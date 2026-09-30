//! The export batch (plan §9): decode → develop pipeline → encode → embed
//! metadata → atomic rename, one photo at a time on a background job.
//! `run_export` is the synchronous core the CLI calls; `ExportJob` wraps it
//! for the scheduler and publishes `CatalogEvent`s.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use archroom_catalog::{Catalog, repo};
use archroom_core::events::{CatalogEvent, EventBus};
use archroom_core::ids::PhotoId;
use archroom_engine::gpu::GpuContext;
use archroom_engine::pipeline::{Pipeline, RenderRequest};
use archroom_engine::{EditParams, Orientation, geometry};
use archroom_io::DecodeOptions;
use archroom_io::export_meta::{ExportMetadata, write_export_metadata};
use archroom_jobs::{Job, JobContext, Priority};
use rusqlite::Connection;

use super::encode::{Pixels, encode_to_file};
use super::{
    Destination, ExportSettings, MetadataMode, NameContext, expand_name, output_size,
    resolve_conflict,
};
use crate::error::{Error, Result};

#[derive(Debug, Clone)]
pub struct ExportRequest {
    pub catalog_path: PathBuf,
    /// Exported in this order (which also sets `{seq}`).
    pub photos: Vec<PhotoId>,
    pub settings: ExportSettings,
}

#[derive(Debug, Clone, Default)]
pub struct ExportSummary {
    pub written: Vec<PathBuf>,
    pub skipped: u32,
    pub failed: Vec<(PhotoId, String)>,
    pub cancelled: bool,
}

/// What `run_export` reports to as it goes.
pub trait ExportSink: Send {
    fn is_cancelled(&self) -> bool {
        false
    }
    fn progress(&self, _done: u32, _total: u32) {}
}

#[derive(Debug)]
pub struct NullExportSink;
impl ExportSink for NullExportSink {}

enum Outcome {
    Written(PathBuf),
    Skipped,
}

/// Exports every photo in `req`; a photo that fails is recorded and the
/// batch carries on. Only setup errors (the catalog won't open) abort it.
pub fn run_export(
    gpu: &GpuContext,
    req: &ExportRequest,
    sink: &dyn ExportSink,
) -> Result<ExportSummary> {
    let catalog = Catalog::create_or_open(&req.catalog_path)?;
    let conn = catalog.connection();
    let total = req.photos.len() as u32;
    let mut summary = ExportSummary::default();
    for (i, &photo) in req.photos.iter().enumerate() {
        if sink.is_cancelled() {
            summary.cancelled = true;
            break;
        }
        let seq = req.settings.sequence_start.saturating_add(i as u32);
        match export_one(conn, gpu, photo, &req.settings, seq) {
            Ok(Outcome::Written(p)) => summary.written.push(p),
            Ok(Outcome::Skipped) => summary.skipped += 1,
            Err(e) => {
                tracing::warn!(photo = photo.get(), error = %e, "export failed");
                summary.failed.push((photo, e.to_string()));
            }
        }
        sink.progress(i as u32 + 1, total);
    }
    Ok(summary)
}

fn destination_dir(settings: &ExportSettings, original: &Path) -> PathBuf {
    let parent = original.parent().unwrap_or_else(|| Path::new("."));
    match &settings.destination {
        Destination::Folder(dir) => dir.clone(),
        Destination::SameAsOriginal => parent.to_path_buf(),
        Destination::Subfolder(name) => parent.join(name),
    }
}

fn export_one(
    conn: &Connection,
    gpu: &GpuContext,
    photo: PhotoId,
    settings: &ExportSettings,
    seq: u32,
) -> Result<Outcome> {
    let info = repo::photo_file_info(conn, photo)?
        .ok_or_else(|| Error::Other("the photo no longer exists".into()))?;
    if !info.path.is_file() {
        return Err(Error::Other(format!("{} is missing", info.path.display())));
    }
    let exif = repo::photo_exif(conn, photo)?;
    let iptc = repo::photo_iptc(conn, photo)?;

    // Pick the output file before the (slow) render, so a skip is cheap.
    let dir = destination_dir(settings, &info.path);
    let stem = info
        .path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("export");
    let name = expand_name(
        &settings.naming,
        &NameContext {
            stem,
            seq,
            capture_time: exif.capture_time.as_deref(),
            title: iptc.title.as_deref(),
            custom: &settings.custom_text,
        },
    );
    std::fs::create_dir_all(&dir).map_err(|e| Error::io(&dir, e))?;
    let ext = settings.format.extension();
    let Some(target) = resolve_conflict(&dir, &name, ext, settings.conflict) else {
        return Ok(Outcome::Skipped);
    };

    let params = crate::develop::load_params(conn, photo)?;
    let t_start = std::time::Instant::now();
    let (w, h, pixels) = render(gpu, &info, &params, settings)?;
    let t_rendered = t_start.elapsed();

    // Write beside the target and rename, so a crash or cancel never leaves a
    // truncated file under the final name.
    let part = dir.join(format!(".{name}.part.{ext}"));
    let result = (|| {
        let icc = settings
            .space
            .icc_bytes()
            .map_err(|e| Error::Other(e.to_string()))?;
        let t = std::time::Instant::now();
        encode_to_file(&part, w, h, &pixels, settings, &icc)?;
        let t_encoded = t.elapsed();
        if let Some(meta) = export_metadata(conn, photo, settings)? {
            write_export_metadata(&part, &meta)?;
        }
        let t_meta = t.elapsed() - t_encoded;
        tracing::debug!(
            render_and_readback_ms = t_rendered.as_millis() as u64,
            encode_ms = t_encoded.as_millis() as u64,
            metadata_ms = t_meta.as_millis() as u64,
            "export stages"
        );
        std::fs::rename(&part, &target).map_err(|e| Error::io(&target, e))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    result?;
    Ok(Outcome::Written(target))
}

fn render(
    gpu: &GpuContext,
    info: &repo::PhotoFileInfo,
    params: &EditParams,
    settings: &ExportSettings,
) -> Result<(u32, u32, Pixels)> {
    let t0 = std::time::Instant::now();
    let decoder = archroom_io::decoder_for(&info.path)
        .ok_or_else(|| Error::Other(format!("no decoder for {}", info.path.display())))?;
    let decoded = decoder.decode(&info.path, &DecodeOptions::default())?;
    let t_decode = t0.elapsed();
    let mut pipeline = Pipeline::new(gpu, &decoded).map_err(|e| Error::Other(e.to_string()))?;
    let t_upload = t0.elapsed();

    let orientation = Orientation::from_exif(info.exif_orientation).rotated(info.user_orientation);
    let (sw, sh) = pipeline.source_size();
    let (cw, ch) = geometry::resolve((sw, sh), orientation, params, false).crop_px();
    let (tw, th) = output_size(settings.resize, (cw.round() as u32, ch.round() as u32));

    let mut req = RenderRequest::new(params.clone(), tw.max(th));
    req.orientation = orientation;
    req.space = settings.space;
    req.depth16 = settings.effective_depth() == 16;
    pipeline
        .render(&req)
        .map_err(|e| Error::Other(e.to_string()))?;
    let t_render = t0.elapsed();
    tracing::debug!(
        decode_ms = t_decode.as_millis() as u64,
        upload_ms = (t_upload - t_decode).as_millis() as u64,
        render_ms = (t_render - t_upload).as_millis() as u64,
        "export render"
    );
    if req.depth16 {
        let (w, h, px) = pipeline
            .read_output_rgba16()
            .map_err(|e| Error::Other(e.to_string()))?;
        Ok((w, h, Pixels::Rgba16(px)))
    } else {
        let (w, h, px) = pipeline
            .read_output_rgba8()
            .map_err(|e| Error::Other(e.to_string()))?;
        Ok((w, h, Pixels::Rgba8(px)))
    }
}

/// The metadata to embed, filtered by the settings; `None` writes nothing.
fn export_metadata(
    conn: &Connection,
    photo: PhotoId,
    settings: &ExportSettings,
) -> Result<Option<ExportMetadata>> {
    if settings.metadata == MetadataMode::None {
        return Ok(None);
    }
    let iptc = repo::photo_iptc(conn, photo)?;
    let mut meta = ExportMetadata {
        creator: iptc.creator,
        copyright: iptc.copyright,
        ..Default::default()
    };
    if settings.metadata == MetadataMode::All {
        let exif = repo::photo_exif(conn, photo)?;
        meta.camera_make = exif.camera_make;
        meta.camera_model = exif.camera_model;
        meta.lens = exif.lens;
        meta.capture_time = exif.capture_time;
        meta.focal_length = exif.focal_length;
        meta.aperture = exif.aperture;
        meta.shutter = exif.shutter;
        meta.iso = exif.iso;
        if !settings.remove_location
            && let (Some(lat), Some(lon)) = (exif.gps_lat, exif.gps_lon)
        {
            meta.gps = Some((lat, lon));
        }
        meta.title = iptc.title;
        meta.caption = iptc.caption;
        let (flat, paths) = exportable_keywords(conn, photo)?;
        meta.keywords = flat;
        if settings.hierarchical_keywords {
            meta.hierarchical_keywords = paths;
        }
    }
    Ok(Some(meta))
}

/// The photo's keywords whose `include_on_export` flag is on: leaf names,
/// and `a|b|c` paths for the hierarchy.
fn exportable_keywords(conn: &Connection, photo: PhotoId) -> Result<(Vec<String>, Vec<String>)> {
    let all = repo::list_keywords(conn)?;
    let by_id: std::collections::HashMap<_, _> = all.iter().map(|k| (k.id, k)).collect();
    let (mut flat, mut paths) = (Vec::new(), Vec::new());
    for id in repo::list_keywords_for_photo(conn, photo)? {
        let Some(k) = by_id.get(&id) else { continue };
        if !k.include_on_export {
            continue;
        }
        let mut segments = vec![k.name.clone()];
        let mut cur = k.parent_id;
        while let Some(pid) = cur
            && let Some(p) = by_id.get(&pid)
        {
            segments.push(p.name.clone());
            cur = p.parent_id;
        }
        segments.reverse();
        flat.push(k.name.clone());
        paths.push(segments.join("|"));
    }
    flat.sort();
    flat.dedup();
    paths.sort();
    Ok((flat, paths))
}

/// Runs [`run_export`] on the scheduler and publishes progress events.
#[derive(Debug)]
pub struct ExportJob {
    req: ExportRequest,
    gpu: GpuContext,
    events: Arc<EventBus<CatalogEvent>>,
    export_id: u64,
}

impl ExportJob {
    pub fn new(
        req: ExportRequest,
        gpu: GpuContext,
        events: Arc<EventBus<CatalogEvent>>,
        export_id: u64,
    ) -> Self {
        Self {
            req,
            gpu,
            events,
            export_id,
        }
    }
}

struct EventSink<'a> {
    cx: &'a JobContext,
    events: &'a EventBus<CatalogEvent>,
    export_id: u64,
}

impl ExportSink for EventSink<'_> {
    fn is_cancelled(&self) -> bool {
        self.cx.is_cancelled()
    }

    fn progress(&self, done: u32, total: u32) {
        self.cx.report_progress(done as f32 / total.max(1) as f32);
        self.events.publish(CatalogEvent::ExportProgress {
            export_id: self.export_id,
            done,
            total,
        });
    }
}

impl Job for ExportJob {
    fn label(&self) -> String {
        format!("Exporting {} photos", self.req.photos.len())
    }

    fn priority(&self) -> Priority {
        Priority::UserBatch
    }

    fn run(self: Box<Self>, cx: &JobContext) {
        let sink = EventSink {
            cx,
            events: &self.events,
            export_id: self.export_id,
        };
        let summary = run_export(&self.gpu, &self.req, &sink).unwrap_or_else(|e| {
            tracing::error!(error = %e, "export job failed");
            ExportSummary {
                failed: self
                    .req
                    .photos
                    .iter()
                    .map(|&p| (p, e.to_string()))
                    .collect(),
                ..Default::default()
            }
        });
        if self.req.settings.show_in_file_manager
            && let Some(first) = summary.written.first()
        {
            let _ = open::that(first.parent().unwrap_or(first));
        }
        self.events.publish(CatalogEvent::ExportFinished {
            export_id: self.export_id,
            exported: summary.written.len() as u32,
            skipped: summary.skipped,
            failed: summary.failed.len() as u32,
            cancelled: summary.cancelled,
        });
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;
    use crate::export::{Conflict, Format, Resize};
    use crate::import::{ImportMode, ImportOptions, NullProgressSink, run_import};

    fn gpu() -> Option<GpuContext> {
        let g = GpuContext::headless();
        if g.is_none() {
            eprintln!("no Vulkan adapter; skipping GPU test");
        }
        g
    }

    fn hash(path: &Path) -> u64 {
        xxhash_rust::xxh3::xxh3_64(&std::fs::read(path).unwrap())
    }

    /// A catalog with two PNG originals in a temp folder.
    fn fixture() -> (tempfile::TempDir, ExportRequest, Vec<PathBuf>) {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir(&src).unwrap();
        let mut originals = Vec::new();
        for name in ["a.png", "b.png"] {
            let img = image::RgbImage::from_fn(64, 48, |x, y| {
                image::Rgb([(x * 4) as u8, (y * 5) as u8, 90])
            });
            let p = src.join(name);
            img.save(&p).unwrap();
            originals.push(p);
        }
        let opts = ImportOptions {
            catalog_path: dir.path().join("t.arcat"),
            source: src,
            mode: ImportMode::Add,
        };
        let summary = run_import(&opts, &NullProgressSink).unwrap();
        let req = ExportRequest {
            catalog_path: opts.catalog_path,
            photos: summary.photo_ids,
            settings: ExportSettings {
                destination: Destination::Folder(dir.path().join("out")),
                ..Default::default()
            },
        };
        (dir, req, originals)
    }

    #[test]
    fn exports_every_photo_without_touching_the_originals() {
        let Some(g) = gpu() else { return };
        let (dir, mut req, originals) = fixture();
        let before: Vec<u64> = originals.iter().map(|p| hash(p)).collect();
        req.settings.naming = "{seq:3}_{filename}".into();
        let s = run_export(&g, &req, &NullExportSink).unwrap();
        assert_eq!(
            (s.written.len(), s.skipped, s.failed.len()),
            (2, 0, 0),
            "{s:?}"
        );
        let names: Vec<_> = s
            .written
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap().to_string())
            .collect();
        // Import order decides which original gets which number.
        assert!(
            names[0].starts_with("001_") && names[1].starts_with("002_"),
            "{names:?}"
        );
        assert_eq!(
            names
                .iter()
                .filter(|n| n.ends_with("a.jpg") || n.ends_with("b.jpg"))
                .count(),
            2
        );
        assert_eq!(image::open(&s.written[0]).unwrap().width(), 64);
        // No temp files left behind.
        let leftovers = std::fs::read_dir(dir.path().join("out"))
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_str()
                    .unwrap()
                    .starts_with('.')
            })
            .count();
        assert_eq!(leftovers, 0);
        let after: Vec<u64> = originals.iter().map(|p| hash(p)).collect();
        assert_eq!(before, after, "an original was modified");
    }

    #[test]
    fn resize_format_depth_and_conflicts() {
        let Some(g) = gpu() else { return };
        let (_dir, mut req, _) = fixture();
        req.photos.truncate(1);
        req.settings.format = Format::Tiff;
        req.settings.bit_depth = 16;
        req.settings.resize = Resize::LongEdge(32);
        let first = run_export(&g, &req, &NullExportSink).unwrap();
        let path = first.written[0].clone();
        let mut dec = tiff::decoder::Decoder::new(std::fs::File::open(&path).unwrap()).unwrap();
        assert_eq!(dec.dimensions().unwrap(), (32, 24));
        assert!(matches!(
            dec.read_image().unwrap(),
            tiff::decoder::DecodingResult::U16(_)
        ));

        // Same name again: unique, then skip, then overwrite.
        let again = run_export(&g, &req, &NullExportSink).unwrap();
        let stem = path.file_stem().unwrap().to_str().unwrap();
        assert_eq!(
            again.written[0].file_name().unwrap().to_str().unwrap(),
            format!("{stem}-1.tif")
        );
        req.settings.conflict = Conflict::Skip;
        let skipped = run_export(&g, &req, &NullExportSink).unwrap();
        assert_eq!((skipped.written.len(), skipped.skipped), (0, 1));
        req.settings.conflict = Conflict::Overwrite;
        let over = run_export(&g, &req, &NullExportSink).unwrap();
        assert_eq!(over.written[0], path);
    }

    #[test]
    fn a_missing_original_fails_that_photo_only() {
        let Some(g) = gpu() else { return };
        let (_dir, req, originals) = fixture();
        std::fs::remove_file(&originals[0]).unwrap();
        let s = run_export(&g, &req, &NullExportSink).unwrap();
        assert_eq!((s.written.len(), s.failed.len()), (1, 1), "{s:?}");
        assert!(s.failed[0].1.contains("missing"));
    }

    #[test]
    fn cancelling_stops_the_batch_and_leaves_no_partial_files() {
        let Some(g) = gpu() else { return };
        let (dir, req, _) = fixture();
        struct CancelAfterOne(AtomicU32);
        impl ExportSink for CancelAfterOne {
            fn is_cancelled(&self) -> bool {
                self.0.load(Ordering::SeqCst) >= 1
            }
            fn progress(&self, done: u32, _total: u32) {
                self.0.store(done, Ordering::SeqCst);
            }
        }
        let s = run_export(&g, &req, &CancelAfterOne(AtomicU32::new(0))).unwrap();
        assert!(s.cancelled);
        assert_eq!(s.written.len(), 1);
        let files: Vec<_> = std::fs::read_dir(dir.path().join("out"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(files.len(), 1, "{files:?}");
    }

    #[test]
    fn metadata_modes_and_keyword_filtering() {
        let Some(g) = gpu() else { return };
        let (dir, mut req, _) = fixture();
        req.photos.truncate(1);
        let photo = req.photos[0];
        {
            let cat = Catalog::create_or_open(&req.catalog_path).unwrap();
            let conn = cat.connection();
            conn.execute(
                "UPDATE photos SET title = 'Dawn', creator = 'Me', copyright = '(c) Me' WHERE id = ?1",
                [photo.get()],
            )
            .unwrap();
            let public = repo::upsert_keyword_path(conn, &["Places", "Paris"]).unwrap();
            let private = repo::upsert_keyword_path(conn, &["Family"]).unwrap();
            repo::add_photo_keyword(conn, photo, public).unwrap();
            repo::add_photo_keyword(conn, photo, private).unwrap();
            repo::set_keyword_include_on_export(conn, private, false).unwrap();
        }
        let read = |p: &Path| {
            let m = rexiv2::Metadata::new_from_path(p).unwrap();
            (
                m.get_tag_string("Exif.Image.Copyright").ok(),
                m.get_tag_string("Xmp.dc.title").ok(),
                m.get_tag_multiple_strings("Iptc.Application2.Keywords")
                    .unwrap(),
                m.get_tag_multiple_strings("Xmp.lr.hierarchicalSubject")
                    .unwrap(),
            )
        };
        let all = run_export(&g, &req, &NullExportSink)
            .unwrap()
            .written
            .remove(0);
        let (c, t, kw, h) = read(&all);
        assert_eq!(c.as_deref(), Some("(c) Me"));
        assert!(t.unwrap().ends_with("Dawn"));
        assert_eq!(kw, ["Paris"]);
        assert_eq!(h, ["Places|Paris"]);

        req.settings.metadata = MetadataMode::CopyrightOnly;
        req.settings.destination = Destination::Folder(dir.path().join("c"));
        let copy = run_export(&g, &req, &NullExportSink)
            .unwrap()
            .written
            .remove(0);
        let (c, t, kw, _) = read(&copy);
        assert_eq!(c.as_deref(), Some("(c) Me"));
        assert!((t, kw.len()) == (None, 0));

        req.settings.metadata = MetadataMode::None;
        req.settings.destination = Destination::Folder(dir.path().join("n"));
        let none = run_export(&g, &req, &NullExportSink)
            .unwrap()
            .written
            .remove(0);
        assert_eq!(read(&none).0, None);
    }
}
