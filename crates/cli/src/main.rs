use std::path::PathBuf;

use anyhow::Result;
use archroom_catalog::Catalog;
use archroom_services::import::{ImportMode, ImportOptions, ImportProgressSink, run_import};
use clap::{Parser, Subcommand};

/// `archroom-cli`: the headless entry point into the engine, catalog and
/// services crates (plan §4.1) — used for tests, batch jobs and benchmarks,
/// and to keep the UI toolkit replaceable.
#[derive(Parser)]
#[command(name = "archroom-cli", version)]
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
        /// Print the RGB/luma histogram peak bins and timings.
        #[arg(long)]
        stats: bool,
    },
}

#[derive(Subcommand)]
enum CatalogAction {
    /// Create the catalog if missing, then run `PRAGMA integrity_check`.
    Check { path: PathBuf },
    /// Copy the catalog to `dest` via the SQLite online-backup API.
    Backup { path: PathBuf, dest: PathBuf },
}

fn main() -> Result<()> {
    archroom_core::tracing_setup::init();
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
        },
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
            stats,
        } => render(&RenderArgs {
            file,
            output,
            params,
            auto,
            max_edge,
            no_orient,
            clip,
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
    stats: bool,
}

fn render(a: &RenderArgs) -> Result<()> {
    use archroom_engine::analysis::{auto_tone, auto_wb, downscale};
    use archroom_engine::gpu::GpuContext;
    use archroom_engine::ops::{
        Exposure, ExposureParams, Profile, Tone, WbMode, WhiteBalance, WhiteBalanceParams,
    };
    use archroom_engine::pipeline::{Pipeline, RenderRequest};
    use archroom_engine::{EditParams, Orientation};
    use archroom_io::DecodedImage;
    use std::time::Instant;

    let decoder = archroom_io::decoder_for(&a.file)
        .ok_or_else(|| anyhow::anyhow!("unsupported file: {}", a.file.display()))?;
    let t = Instant::now();
    let decoded = decoder.decode(&a.file, &archroom_io::DecodeOptions::default())?;
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
        .ok_or_else(|| anyhow::anyhow!("no Vulkan adapter (set ARCHROOM_ADAPTER to pick one)"))?;
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
            && let Some(sc) = archroom_engine::rawprep::SceneColor::new(camera)
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
                WbMode::Custom => archroom_color::temp::temp_tint_to_xy(wb.temp, wb.tint),
            };
            let m = sc.decoded_to_working(illuminant, archroom_color::cat::Cat::Cat16);
            if let Some(at) = auto_tone(&proxy, &m, &edit.get::<Profile>(), false) {
                edit.set::<Exposure>(ExposureParams { ev: at.exposure_ev });
                edit.set::<Tone>(at.tone);
            }
        } else {
            let m = archroom_color::cie::REC2020
                .xyz_to_rgb()
                .mul(&archroom_color::cie::SRGB.rgb_to_xyz());
            if let Some(at) = auto_tone(&proxy, &m, &edit.get::<Profile>(), true) {
                edit.set::<Exposure>(ExposureParams { ev: at.exposure_ev });
                edit.set::<Tone>(at.tone);
            }
        }
    }

    let mut req = RenderRequest::new(edit.clone(), a.max_edge);
    req.orientation = orientation;
    req.clip_overlay = a.clip;
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
