use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use viberoom_catalog::command::{Command as CatalogCommand, Removal, RemovePhotos};
use viberoom_catalog::{Catalog, repo};
use viberoom_color::icc::OutputSpace;
use viberoom_core::ids::PhotoId;
use viberoom_services::import::{ImportMode, ImportOptions, ImportProgressSink, run_import};
use viberoom_services::library::{self, TrashPhotos};

/// `viberoom-cli`: the headless entry point into the engine, catalog and
/// services crates (plan §4.1) — used for tests, batch jobs and benchmarks,
/// and to keep the UI toolkit replaceable.
#[derive(Parser)]
#[command(name = "viberoom-cli", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Catalog maintenance: create/open and integrity-check a `.arcat` file.
    Catalog {
        #[command(subcommand)]
        action: CatalogAction,
    },
    /// Import a folder of photos into a catalog (plan §7.1).
    Import {
        /// The `.arcat` catalog to import into (created if missing).
        catalog: PathBuf,
        /// The folder to scan, recursively.
        source: PathBuf,
        /// Copy files into this folder (`{dest}/{YYYY}/{YYYY-MM-DD}/…`)
        /// instead of referencing them in place.
        #[arg(long)]
        copy_to: Option<PathBuf>,
    },
    /// Render a photo through the develop pipeline to a PNG (plan §11 M3):
    /// the headless twin of the Develop canvas, also the golden-image tool.
    Render {
        /// The raw or image file to render.
        file: PathBuf,
        /// Output PNG.
        #[arg(short, long)]
        output: PathBuf,
        /// Edit params as JSON, or `@path/to/params.json`. Default: unedited.
        #[arg(long)]
        params: Option<String>,
        /// Apply Auto White Balance and Auto Tone on top of `--params`.
        #[arg(long)]
        auto: bool,
        /// Longest edge of the output in pixels.
        #[arg(long, default_value_t = 2048)]
        max_edge: u32,
        /// Ignore the file's EXIF orientation.
        #[arg(long)]
        no_orient: bool,
        /// Draw the clipping overlay.
        #[arg(long)]
        clip: bool,
        /// Tint the mask of the local adjustment with this id red.
        #[arg(long)]
        mask_overlay: Option<String>,
        /// Print the RGB/luma histogram peak bins and timings.
        #[arg(long)]
        stats: bool,
    },
    /// Export photos from a catalog (plan §9): the headless twin of the
    /// Export dialog. Starts from a preset, then applies the flags.
    Export {
        /// The `.arcat` catalog to export from.
        catalog: PathBuf,
        /// Photo ids to export (default: every photo).
        #[arg(long = "photo")]
        photos: Vec<i64>,
        /// A built-in or saved preset name (`--list-presets` shows them).
        #[arg(long, default_value = "JPEG sRGB full size")]
        preset: String,
        /// Print the available presets and exit.
        #[arg(long)]
        list_presets: bool,
        /// Export into this folder (default: the preset's destination).
        #[arg(long)]
        dest: Option<PathBuf>,
        /// jpeg, tiff or png.
        #[arg(long)]
        format: Option<String>,
        /// JPEG quality, 1-100.
        #[arg(long)]
        quality: Option<u8>,
        /// 8 or 16 (TIFF/PNG).
        #[arg(long)]
        depth: Option<u8>,
        /// srgb, p3, adobe or prophoto.
        #[arg(long)]
        space: Option<String>,
        /// Resize so the longest edge is this many pixels.
        #[arg(long)]
        long_edge: Option<u32>,
        /// Naming template, e.g. `{date:%Y%m%d}_{seq:4}_{filename}`.
        #[arg(long)]
        naming: Option<String>,
        /// unique, overwrite or skip.
        #[arg(long)]
        conflict: Option<String>,
        /// all, copyright or none.
        #[arg(long)]
        metadata: Option<String>,
    },
}

#[derive(Subcommand)]
enum CatalogAction {
    /// Create the catalog if missing, then run `PRAGMA integrity_check`.
    Check { path: PathBuf },
    /// Copy the catalog to `dest` via the SQLite online-backup API.
    Backup { path: PathBuf, dest: PathBuf },
    /// List the photos (id, flag, path); `--missing` shows only missing ones.
    List {
        path: PathBuf,
        #[arg(long)]
        missing: bool,
    },
    /// `stat` the originals and flag the ones gone from disk (and clear the
    /// flag on ones that came back). Only the catalog's `missing` flag changes.
    CheckMissing { path: PathBuf },
    /// Remove photos from the catalog. Files on disk are never touched.
    /// Without `--yes` this only prints what it would do.
    Remove {
        path: PathBuf,
        /// Photo ids (see `catalog list`).
        #[arg(required = true)]
        ids: Vec<i64>,
        #[arg(long)]
        yes: bool,
    },
    /// Move photos' original files (and sidecars) to the freedesktop trash,
    /// then remove them from the catalog. Never deletes permanently.
    /// Without `--yes` this only prints the plan.
    Trash {
        path: PathBuf,
        #[arg(required = true)]
        ids: Vec<i64>,
        #[arg(long)]
        yes: bool,
    },
}

fn main() -> Result<()> {
    viberoom_core::tracing_setup::init();
    let cli = Cli::parse();

    match cli.command {
        Command::Catalog { action } => match action {
            CatalogAction::Check { path } => {
                let catalog = Catalog::create_or_open(&path)?;
                if catalog.integrity_check()? {
                    println!("ok: {}", path.display());
                } else {
                    anyhow::bail!("integrity check failed for {}", path.display());
                }
            }
            CatalogAction::Backup { path, dest } => {
                let catalog = Catalog::create_or_open(&path)?;
                catalog.backup_to(&dest)?;
                println!("backed up {} -> {}", path.display(), dest.display());
            }
            CatalogAction::List { path, missing } => {
                let catalog = Catalog::create_or_open(&path)?;
                let conn = catalog.connection();
                let folders: std::collections::HashMap<_, _> = repo::list_folders(conn)?
                    .into_iter()
                    .map(|f| (f.id, f.path))
                    .collect();
                for p in repo::list_all_photos(conn, repo::PhotoSort::ImportOrder)? {
                    if missing && !p.missing {
                        continue;
                    }
                    let dir = folders.get(&p.folder_id).map_or("?", String::as_str);
                    println!(
                        "{}\t{}\t{}/{}",
                        p.photo_id.get(),
                        if p.missing { "MISSING" } else { "ok" },
                        dir,
                        p.filename
                    );
                }
            }
            CatalogAction::CheckMissing { path } => {
                let catalog = Catalog::create_or_open(&path)?;
                let report = library::check_missing(
                    catalog.connection(),
                    library::MissingScope::All,
                    &|| false,
                )?;
                println!(
                    "checked {}: {} newly missing, {} found again",
                    report.checked,
                    report.now_missing.len(),
                    report.restored.len()
                );
            }
            CatalogAction::Remove { path, ids, yes } => {
                let catalog = Catalog::create_or_open(&path)?;
                let ids: Vec<PhotoId> = ids.into_iter().map(PhotoId::new).collect();
                println!(
                    "{} {} photo(s) from the catalog; files on disk are not touched",
                    if yes { "removing" } else { "would remove" },
                    ids.len()
                );
                if yes {
                    RemovePhotos::new(ids, Removal::Catalog).apply(catalog.connection())?;
                    println!("done (the rows are purged for good the next time the app starts)");
                } else {
                    println!("dry run: pass --yes to do it");
                }
            }
            CatalogAction::Trash { path, ids, yes } => {
                let catalog = Catalog::create_or_open(&path)?;
                let ids: Vec<PhotoId> = ids.into_iter().map(PhotoId::new).collect();
                let plan = library::plan_trash(catalog.connection(), &ids)?;
                for f in &plan.files {
                    println!("  trash: {}", f.display());
                }
                println!(
                    "{} photo(s); {} file(s) to trash, {} kept (shared with another photo), {} already missing",
                    ids.len(),
                    plan.files.len(),
                    plan.kept_shared,
                    plan.already_missing
                );
                if yes {
                    TrashPhotos::new(plan, Box::new(library::SystemTrash))
                        .apply(catalog.connection())?;
                    println!("done; restore the files from your file manager's trash if needed");
                } else {
                    println!("dry run: pass --yes to do it");
                }
            }
        },
        Command::Export {
            catalog,
            photos,
            preset,
            list_presets,
            dest,
            format,
            quality,
            depth,
            space,
            long_edge,
            naming,
            conflict,
            metadata,
        } => {
            use viberoom_catalog::repo::{PhotoSort, list_all_photos};
            use viberoom_core::ids::PhotoId;
            use viberoom_services::export::job::{ExportRequest, ExportSink, run_export};
            use viberoom_services::export::{
                Conflict, Destination, Format, MetadataMode, Resize, all_presets,
            };
            let cat = Catalog::create_or_open(&catalog)?;
            let presets = all_presets(cat.connection())?;
            if list_presets {
                for p in &presets {
                    println!("{}{}", p.name, if p.builtin { "  (built in)" } else { "" });
                }
                return Ok(());
            }
            let mut settings = presets
                .into_iter()
                .find(|p| p.name == preset)
                .ok_or_else(|| anyhow::anyhow!("no export preset named \"{preset}\""))?
                .settings;
            if let Some(dir) = dest {
                settings.destination = Destination::Folder(dir);
            }
            if let Some(f) = format {
                settings.format = match f.to_ascii_lowercase().as_str() {
                    "jpeg" | "jpg" => Format::Jpeg,
                    "tiff" | "tif" => Format::Tiff,
                    "png" => Format::Png,
                    other => anyhow::bail!("unknown format {other}"),
                };
            }
            if let Some(q) = quality {
                settings.jpeg_quality = q;
            }
            if let Some(d) = depth {
                anyhow::ensure!(d == 8 || d == 16, "depth must be 8 or 16");
                settings.bit_depth = d;
            }
            if let Some(s) = space {
                settings.space = match s.to_ascii_lowercase().as_str() {
                    "srgb" => OutputSpace::Srgb,
                    "p3" => OutputSpace::DisplayP3,
                    "adobe" => OutputSpace::AdobeRgb,
                    "prophoto" => OutputSpace::ProPhoto,
                    other => anyhow::bail!("unknown color space {other}"),
                };
            }
            if let Some(n) = long_edge {
                settings.resize = Resize::LongEdge(n);
            }
            if let Some(n) = naming {
                settings.naming = n;
            }
            if let Some(c) = conflict {
                settings.conflict = match c.to_ascii_lowercase().as_str() {
                    "unique" => Conflict::Unique,
                    "overwrite" => Conflict::Overwrite,
                    "skip" => Conflict::Skip,
                    other => anyhow::bail!("unknown conflict policy {other}"),
                };
            }
            if let Some(m) = metadata {
                settings.metadata = match m.to_ascii_lowercase().as_str() {
                    "all" => MetadataMode::All,
                    "copyright" => MetadataMode::CopyrightOnly,
                    "none" => MetadataMode::None,
                    other => anyhow::bail!("unknown metadata mode {other}"),
                };
            }
            let photos: Vec<PhotoId> = if photos.is_empty() {
                list_all_photos(cat.connection(), PhotoSort::ImportOrder)?
                    .into_iter()
                    .map(|p| p.photo_id)
                    .collect()
            } else {
                photos.into_iter().map(PhotoId::new).collect()
            };
            drop(cat);
            let gpu = viberoom_engine::gpu::GpuContext::headless()
                .ok_or_else(|| anyhow::anyhow!("no usable Vulkan adapter"))?;
            struct Stdout;
            impl ExportSink for Stdout {
                fn progress(&self, done: u32, total: u32) {
                    println!("exported {done}/{total}");
                }
            }
            let req = ExportRequest {
                catalog_path: catalog,
                photos,
                settings,
            };
            let summary = run_export(&gpu, &req, &Stdout)?;
            for p in &summary.written {
                println!("wrote {}", p.display());
            }
            for (id, why) in &summary.failed {
                eprintln!("photo {} failed: {why}", id.get());
            }
            println!(
                "{} written, {} skipped, {} failed",
                summary.written.len(),
                summary.skipped,
                summary.failed.len()
            );
            if !summary.failed.is_empty() {
                std::process::exit(1);
            }
        }
        Command::Import {
            catalog,
            source,
            copy_to,
        } => {
            let mode = match copy_to {
                Some(dest) => ImportMode::Copy { dest },
                None => ImportMode::Add,
            };
            let opts = ImportOptions {
                catalog_path: catalog,
                source,
                mode,
            };
            let summary = run_import(&opts, &StdoutProgressSink)?;
            println!(
                "imported {} of {} scanned ({} duplicate{} skipped)",
                summary.imported,
                summary.scanned,
                summary.skipped_duplicates,
                if summary.skipped_duplicates == 1 {
                    ""
                } else {
                    "s"
                }
            );
        }
        Command::Render {
            file,
            output,
            params,
            auto,
            max_edge,
            no_orient,
            clip,
            mask_overlay,
            stats,
        } => render(&RenderArgs {
            file,
            output,
            params,
            auto,
            max_edge,
            no_orient,
            clip,
            mask_overlay,
            stats,
        })?,
    }

    Ok(())
}

struct RenderArgs {
    file: PathBuf,
    output: PathBuf,
    params: Option<String>,
    auto: bool,
    max_edge: u32,
    no_orient: bool,
    clip: bool,
    mask_overlay: Option<String>,
    stats: bool,
}

fn render(a: &RenderArgs) -> Result<()> {
    use std::time::Instant;
    use viberoom_engine::analysis::{auto_tone, auto_wb, downscale};
    use viberoom_engine::gpu::GpuContext;
    use viberoom_engine::ops::{
        Exposure, ExposureParams, Profile, Tone, WbMode, WhiteBalance, WhiteBalanceParams,
    };
    use viberoom_engine::pipeline::{Pipeline, RenderRequest};
    use viberoom_engine::{EditParams, Orientation};
    use viberoom_io::DecodedImage;

    let decoder = viberoom_io::decoder_for(&a.file)
        .ok_or_else(|| anyhow::anyhow!("unsupported file: {}", a.file.display()))?;
    let t = Instant::now();
    let decoded = decoder.decode(&a.file, &viberoom_io::DecodeOptions::default())?;
    let decode_ms = t.elapsed().as_millis();
    let orientation = if a.no_orient {
        Orientation::IDENTITY
    } else {
        Orientation::from_exif(decoder.metadata(&a.file)?.orientation)
    };

    let mut edit = match &a.params {
        None => EditParams::default(),
        Some(p) => {
            let json = match p.strip_prefix('@') {
                Some(path) => std::fs::read_to_string(path)?,
                None => p.clone(),
            };
            EditParams::from_json(&json)?
        }
    };

    let gpu = GpuContext::headless()
        .ok_or_else(|| anyhow::anyhow!("no Vulkan adapter (set VIBEROOM_ADAPTER to pick one)"))?;
    let t = Instant::now();
    let mut pipeline = Pipeline::new(&gpu, &decoded)?;
    let upload_ms = t.elapsed().as_millis();

    if a.auto {
        let (rgb, rendered) = match &decoded {
            DecodedImage::SceneLinear { rgb, .. } => (rgb, false),
            DecodedImage::Rendered { rgb, .. } => (rgb, true),
        };
        let proxy = downscale(rgb, 512, rendered);
        if let DecodedImage::SceneLinear { camera, .. } = &decoded
            && let Some(sc) = viberoom_engine::rawprep::SceneColor::new(camera)
        {
            if let Some((temp, tint)) = auto_wb(&proxy, &sc) {
                edit.set::<WhiteBalance>(WhiteBalanceParams {
                    mode: WbMode::Custom,
                    temp,
                    tint,
                });
            }
            let wb = edit.get::<WhiteBalance>();
            let illuminant = match wb.mode {
                WbMode::AsShot => sc.as_shot_white(),
                WbMode::Custom => viberoom_color::temp::temp_tint_to_xy(wb.temp, wb.tint),
            };
            let m = sc.decoded_to_working(illuminant, viberoom_color::cat::Cat::Cat16);
            if let Some(at) = auto_tone(&proxy, &m, &edit.get::<Profile>(), false) {
                edit.set::<Exposure>(ExposureParams { ev: at.exposure_ev });
                edit.set::<Tone>(at.tone);
            }
        } else {
            let m = viberoom_color::cie::REC2020
                .xyz_to_rgb()
                .mul(&viberoom_color::cie::SRGB.rgb_to_xyz());
            if let Some(at) = auto_tone(&proxy, &m, &edit.get::<Profile>(), true) {
                edit.set::<Exposure>(ExposureParams { ev: at.exposure_ev });
                edit.set::<Tone>(at.tone);
            }
        }
    }

    let mut req = RenderRequest::new(edit.clone(), a.max_edge);
    req.orientation = orientation;
    req.clip_overlay = a.clip;
    req.mask_overlay.clone_from(&a.mask_overlay);
    req.want_histogram = a.stats;
    let first = pipeline.render(&req)?;
    let (w, h, rgba) = pipeline.read_output_rgba8()?;
    image::save_buffer(&a.output, &rgba, w, h, image::ExtendedColorType::Rgba8)?;

    println!("params: {}", edit.to_json());
    println!(
        "wrote {} ({w}×{h}) on {}",
        a.output.display(),
        gpu.adapter_name
    );
    if a.stats {
        // Slider latency: nudge exposure (scene..output re-run) and presence-like
        // downstream changes, timing the warm pipeline.
        let mut nudged = edit.clone();
        let ev = nudged.get::<Exposure>().ev + 0.1;
        nudged.set::<Exposure>(ExposureParams { ev });
        let mut r2 = req.clone();
        r2.params = nudged;
        let warm = pipeline.render(&r2)?;
        println!(
            "decode {decode_ms} ms, upload {upload_ms} ms, first render {:.1} ms, exposure-slider render {:.1} ms",
            first.elapsed.as_secs_f64() * 1e3,
            warm.elapsed.as_secs_f64() * 1e3
        );
        if let Some(hist) = pipeline.histogram() {
            let peak = |c: usize| {
                hist.bins[c]
                    .iter()
                    .enumerate()
                    .max_by_key(|(_, v)| **v)
                    .map_or(0, |(i, _)| i)
            };
            println!(
                "histogram peaks: R {} G {} B {} L {}",
                peak(0),
                peak(1),
                peak(2),
                peak(3)
            );
        }
    }
    Ok(())
}

struct StdoutProgressSink;

impl ImportProgressSink for StdoutProgressSink {
    fn progress(&self, scanned: u32, imported: u32, total: u32) {
        println!("  {scanned}/{total} scanned, {imported} imported");
    }
}
