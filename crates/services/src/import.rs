//! Import (plan §7.1, M1 roadmap): scan a folder, dedupe, read metadata,
//! write catalog rows and an L1 preview per photo. `run_import` is the
//! synchronous core — the CLI calls it directly; `ImportJob` wraps it for
//! the background scheduler (plan §4.6) and turns its progress into
//! `CatalogEvent`s for the UI.

use std::cell::Cell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use archroom_catalog::Catalog;
use archroom_catalog::repo;
use archroom_core::events::{CatalogEvent, EventBus};
use archroom_core::ids::{FolderId, ImportId, PhotoId};
use archroom_jobs::{Job, JobContext, Priority as JobPriority};
use archroom_preview::{L1_BUDGET_PX, LEVEL_L1, PreviewCache};
use rusqlite::Connection;

use crate::error::Result;

const SUPPORTED_EXTENSIONS: &[&str] = &[
    "nef", "cr2", "cr3", "arw", "raf", "rw2", "orf", "pef", "srw", "dng", "raw", "jpg", "jpeg",
    "png", "tif", "tiff",
];

#[derive(Debug, Clone)]
pub enum ImportMode {
    /// Reference files in place.
    Add,
    /// Copy into `dest/{YYYY}/{YYYY-MM-DD}/{filename}` before importing.
    Copy { dest: PathBuf },
}

#[derive(Debug, Clone)]
pub struct ImportOptions {
    pub catalog_path: PathBuf,
    pub source: PathBuf,
    pub mode: ImportMode,
}

#[derive(Debug, Clone, Default)]
pub struct ImportSummary {
    pub scanned: u32,
    pub imported: u32,
    pub skipped_duplicates: u32,
    pub photo_ids: Vec<PhotoId>,
}

/// What `run_import` reports to as it goes — a CLI printing to stdout
/// synchronously, or a background `Job` publishing catalog events.
pub trait ImportProgressSink: Send {
    fn is_cancelled(&self) -> bool {
        false
    }
    fn import_started(&self, _import_id: ImportId) {}
    fn progress(&self, _scanned: u32, _imported: u32, _total: u32) {}
    fn finished(&self, _summary: &ImportSummary) {}
}

#[derive(Debug)]
pub struct NullProgressSink;
impl ImportProgressSink for NullProgressSink {}

fn is_supported(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| SUPPORTED_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

fn kind_for_extension(ext: &str) -> &'static str {
    match ext {
        "jpg" | "jpeg" => "jpeg",
        "tif" | "tiff" => "tiff",
        "png" => "png",
        _ => "raw",
    }
}

pub fn run_import(opts: &ImportOptions, sink: &dyn ImportProgressSink) -> Result<ImportSummary> {
    let catalog = Catalog::create_or_open(&opts.catalog_path)?;
    let conn = catalog.connection();

    let mode_label = match &opts.mode {
        ImportMode::Add => "add",
        ImportMode::Copy { .. } => "copy",
    };
    let import_id = repo::create_import(conn, &opts.source.to_string_lossy(), mode_label)?;
    sink.import_started(import_id);

    let previews = PreviewCache::open_for_catalog(&opts.catalog_path)?;

    let entries: Vec<PathBuf> = walkdir::WalkDir::new(&opts.source)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file() && is_supported(e.path()))
        .map(|e| e.into_path())
        .collect();
    let total = entries.len() as u32;

    let mut summary = ImportSummary::default();
    let mut folder_cache: HashMap<PathBuf, FolderId> = HashMap::new();

    for src_path in entries {
        if sink.is_cancelled() {
            break;
        }
        summary.scanned += 1;

        let imported_path = match &opts.mode {
            ImportMode::Add => src_path.clone(),
            ImportMode::Copy { dest } => match copy_into(&src_path, dest) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(path = %src_path.display(), error = %e, "import: copy failed, skipping");
                    sink.progress(summary.scanned, summary.imported, total);
                    continue;
                }
            },
        };

        match import_one(
            conn,
            &previews,
            &imported_path,
            import_id,
            &mut folder_cache,
        ) {
            Ok(ImportOutcome::Imported(photo_id)) => {
                summary.imported += 1;
                summary.photo_ids.push(photo_id);
            }
            Ok(ImportOutcome::Duplicate) => summary.skipped_duplicates += 1,
            Err(e) => {
                tracing::warn!(path = %imported_path.display(), error = %e, "import: failed, skipping");
            }
        }
        sink.progress(summary.scanned, summary.imported, total);
    }

    repo::finish_import(conn, import_id, summary.imported)?;
    sink.finished(&summary);
    Ok(summary)
}

enum ImportOutcome {
    Imported(PhotoId),
    Duplicate,
}

fn import_one(
    conn: &Connection,
    previews: &PreviewCache,
    path: &Path,
    import_id: ImportId,
    folder_cache: &mut HashMap<PathBuf, FolderId>,
) -> Result<ImportOutcome> {
    let folder_path = path.parent().ok_or_else(|| {
        crate::error::Error::Other(format!("no parent directory: {}", path.display()))
    })?;
    let folder_id = match folder_cache.get(folder_path) {
        Some(id) => *id,
        None => {
            let id = repo::upsert_folder_path(conn, folder_path)?;
            folder_cache.insert(folder_path.to_path_buf(), id);
            id
        }
    };

    let filename = path.file_name().and_then(|f| f.to_str()).ok_or_else(|| {
        crate::error::Error::Other(format!("non-UTF-8 filename: {}", path.display()))
    })?;

    // Re-importing the same path is always a no-op: `(folder_id, filename)`
    // already identifies "this exact file."
    if repo::find_file(conn, folder_id, filename)?.is_some() {
        return Ok(ImportOutcome::Duplicate);
    }

    let decoder = archroom_io::decoder_for(path)
        .ok_or_else(|| crate::error::Error::Other(format!("no decoder for {}", path.display())))?;
    let metadata = decoder.metadata(path)?;

    let fs_meta = std::fs::metadata(path).map_err(|e| crate::error::Error::io(path, e))?;
    let size = fs_meta.len() as i64;
    let mtime = fs_meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64);
    let quick_hash = quick_hash_of(path)?;

    // Cross-folder duplicate (plan §7.1): filename + size + capture time
    // match an existing file elsewhere, confirmed by a quick hash before
    // treating it as the same photo re-imported from a different path.
    if let Some(candidate) = repo::find_duplicate_candidate(
        conn,
        filename,
        Some(size),
        metadata.capture_time.as_deref(),
    )? && candidate.quick_hash.as_deref() == Some(quick_hash.as_slice())
    {
        return Ok(ImportOutcome::Duplicate);
    }

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let kind = kind_for_extension(&ext);

    let new_file = repo::NewFile {
        folder_id,
        filename,
        ext: &ext,
        kind,
        size: Some(size),
        mtime,
        quick_hash: Some(&quick_hash),
        width: Some(metadata.width),
        height: Some(metadata.height),
        orientation: Some(metadata.orientation),
        capture_time: metadata.capture_time.as_deref(),
        camera_make: metadata.camera_make.as_deref(),
        camera_model: metadata.camera_model.as_deref(),
        lens: metadata.lens.as_deref(),
        focal_length: metadata.focal_length,
        aperture: metadata.aperture,
        shutter: metadata.shutter,
        iso: metadata.iso,
        gps_lat: metadata.gps.map(|g| g.0),
        gps_lon: metadata.gps.map(|g| g.1),
    };
    let file_id = repo::insert_file(conn, &new_file)?;
    let photo_id = repo::insert_photo(
        conn,
        &repo::NewPhoto {
            file_id,
            import_id: Some(import_id),
        },
    )?;

    if let Err(e) =
        crate::preview::generate_and_store(previews, path, photo_id, LEVEL_L1, L1_BUDGET_PX)
    {
        tracing::warn!(path = %path.display(), error = %e, "import: L1 preview generation failed");
    }

    Ok(ImportOutcome::Imported(photo_id))
}

fn quick_hash_of(path: &Path) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|e| crate::error::Error::io(path, e))?;
    let mut buf = vec![0u8; 64 * 1024];
    let n = file
        .read(&mut buf)
        .map_err(|e| crate::error::Error::io(path, e))?;
    buf.truncate(n);
    Ok(xxhash_rust::xxh3::xxh3_64(&buf).to_le_bytes().to_vec())
}

/// Copy mode: `{dest}/{YYYY}/{YYYY-MM-DD}/{filename}`, hash-verified before
/// the copy is treated as a source for the normal Add path (plan §7.1).
fn copy_into(src: &Path, dest: &Path) -> Result<PathBuf> {
    let metadata = archroom_io::decoder_for(src)
        .and_then(|d| d.metadata(src).ok())
        .and_then(|m| m.capture_time)
        .unwrap_or_default();
    let date = &metadata[..10.min(metadata.len())]; // "YYYY-MM-DD" prefix, or "" if unknown
    let (year, day_dir) = if date.len() == 10 {
        (&date[0..4], date.to_string())
    } else {
        ("Unknown Date", "Unknown Date".to_string())
    };

    let dir = dest.join(year).join(&day_dir);
    std::fs::create_dir_all(&dir).map_err(|e| crate::error::Error::io(&dir, e))?;
    let filename = src
        .file_name()
        .ok_or_else(|| crate::error::Error::Other(format!("no filename: {}", src.display())))?;
    let dst = dir.join(filename);

    std::fs::copy(src, &dst).map_err(|e| crate::error::Error::io(&dst, e))?;

    let src_hash = xxhash_rust::xxh3::xxh3_64(
        &std::fs::read(src).map_err(|e| crate::error::Error::io(src, e))?,
    );
    let dst_hash = xxhash_rust::xxh3::xxh3_64(
        &std::fs::read(&dst).map_err(|e| crate::error::Error::io(&dst, e))?,
    );
    if src_hash != dst_hash {
        return Err(crate::error::Error::Other(format!(
            "copy verification failed: {} -> {}",
            src.display(),
            dst.display()
        )));
    }
    Ok(dst)
}

/// Runs [`run_import`] as an `archroom-jobs` background job, publishing
/// `CatalogEvent`s so the UI's view models can invalidate (plan §4.5/§4.6).
#[derive(Debug)]
pub struct ImportJob {
    opts: ImportOptions,
    events: Arc<EventBus<CatalogEvent>>,
}

impl ImportJob {
    pub fn new(opts: ImportOptions, events: Arc<EventBus<CatalogEvent>>) -> Self {
        Self { opts, events }
    }
}

impl Job for ImportJob {
    fn label(&self) -> String {
        format!("Importing from {}", self.opts.source.display())
    }

    fn priority(&self) -> JobPriority {
        JobPriority::UserBatch
    }

    fn run(self: Box<Self>, cx: &JobContext) {
        let sink = EventSink {
            cx,
            events: &self.events,
            import_id: Cell::new(None),
            total: Cell::new(0),
        };
        if let Err(e) = run_import(&self.opts, &sink) {
            tracing::error!(error = %e, "import job failed");
        }
    }
}

struct EventSink<'a> {
    cx: &'a JobContext,
    events: &'a EventBus<CatalogEvent>,
    import_id: Cell<Option<ImportId>>,
    total: Cell<u32>,
}

impl ImportProgressSink for EventSink<'_> {
    fn is_cancelled(&self) -> bool {
        self.cx.is_cancelled()
    }

    fn import_started(&self, import_id: ImportId) {
        self.import_id.set(Some(import_id));
    }

    fn progress(&self, scanned: u32, imported: u32, total: u32) {
        self.total.set(total);
        if let Some(import_id) = self.import_id.get() {
            self.events.publish(CatalogEvent::ImportProgress {
                import_id,
                scanned,
                imported,
                total_hint: Some(total),
            });
        }
        if total > 0 {
            self.cx.report_progress(scanned as f32 / total as f32);
        }
    }

    fn finished(&self, summary: &ImportSummary) {
        let Some(import_id) = self.import_id.get() else {
            return;
        };
        if !summary.photo_ids.is_empty() {
            self.events.publish(CatalogEvent::PhotosAdded {
                ids: summary.photo_ids.clone(),
                import_id: Some(import_id),
            });
        }
        self.events.publish(CatalogEvent::ImportFinished {
            import_id,
            imported: summary.imported,
        });
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn make_test_png(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        let img =
            image::RgbImage::from_fn(8, 6, |x, y| image::Rgb([x as u8 * 10, y as u8 * 10, 128]));
        img.save(&path).unwrap();
        path
    }

    #[test]
    fn imports_a_folder_of_images_and_skips_a_second_pass() {
        let src_dir = tempfile::tempdir().unwrap();
        let catalog_dir = tempfile::tempdir().unwrap();
        make_test_png(src_dir.path(), "a.png");
        make_test_png(src_dir.path(), "b.png");

        let opts = ImportOptions {
            catalog_path: catalog_dir.path().join("Test.arcat"),
            source: src_dir.path().to_path_buf(),
            mode: ImportMode::Add,
        };

        let summary = run_import(&opts, &NullProgressSink).unwrap();
        assert_eq!(summary.scanned, 2);
        assert_eq!(summary.imported, 2);
        assert_eq!(summary.skipped_duplicates, 0);
        assert_eq!(summary.photo_ids.len(), 2);

        let catalog = Catalog::create_or_open(&opts.catalog_path).unwrap();
        let count: i64 = catalog
            .connection()
            .query_row("SELECT count(*) FROM photos", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);

        // Re-importing the same folder is a no-op.
        let second = run_import(&opts, &NullProgressSink).unwrap();
        assert_eq!(second.imported, 0);
        assert_eq!(second.skipped_duplicates, 2);
    }

    #[test]
    fn generates_an_l1_preview_for_each_imported_photo() {
        let src_dir = tempfile::tempdir().unwrap();
        let catalog_dir = tempfile::tempdir().unwrap();
        make_test_png(src_dir.path(), "a.png");

        let opts = ImportOptions {
            catalog_path: catalog_dir.path().join("Test.arcat"),
            source: src_dir.path().to_path_buf(),
            mode: ImportMode::Add,
        };
        let summary = run_import(&opts, &NullProgressSink).unwrap();

        let previews = PreviewCache::open_for_catalog(&opts.catalog_path).unwrap();
        let path = previews
            .lookup(summary.photo_ids[0], LEVEL_L1)
            .unwrap()
            .expect("an L1 preview should exist");
        assert!(path.exists());
    }

    #[test]
    fn copy_mode_places_files_under_a_date_template_and_verifies_the_hash() {
        let src_dir = tempfile::tempdir().unwrap();
        let catalog_dir = tempfile::tempdir().unwrap();
        let dest_dir = tempfile::tempdir().unwrap();
        make_test_png(src_dir.path(), "a.png");

        let opts = ImportOptions {
            catalog_path: catalog_dir.path().join("Test.arcat"),
            source: src_dir.path().to_path_buf(),
            mode: ImportMode::Copy {
                dest: dest_dir.path().to_path_buf(),
            },
        };
        let summary = run_import(&opts, &NullProgressSink).unwrap();
        assert_eq!(summary.imported, 1);

        // The file must have landed somewhere under dest_dir, not stayed in src_dir.
        let copied = walkdir::WalkDir::new(dest_dir.path())
            .into_iter()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name() == "a.png");
        assert!(
            copied,
            "expected a.png to be copied under {}",
            dest_dir.path().display()
        );
    }
}
