#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Query performance on a generated 50,000-photo catalog (plan §13: "query
//! tests on generated 50k-row catalogs to catch performance regressions").
//! The bounds are loose so a slow CI machine passes, tight enough that an
//! accidental full-table scan per photo would not.

use std::time::{Duration, Instant};

use viberoom_catalog::Catalog;
use viberoom_catalog::repo::{
    self, NewFile, NewPhoto, PhotoSort, list_all_photos, list_photos_for_folder,
};

const PHOTOS: usize = 50_000;

fn timed<T>(what: &str, limit: Duration, f: impl FnOnce() -> T) -> T {
    let t = Instant::now();
    let out = f();
    let took = t.elapsed();
    eprintln!("{what}: {took:?}");
    assert!(took < limit, "{what} took {took:?}, limit {limit:?}");
    out
}

#[test]
fn a_50k_catalog_opens_and_lists_quickly() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Big.arcat");
    {
        let catalog = Catalog::create_or_open(&path).unwrap();
        let conn = catalog.connection();
        conn.execute_batch("BEGIN").unwrap();
        let folders: Vec<_> = (0..200)
            .map(|i| {
                repo::upsert_folder_path(
                    conn,
                    std::path::Path::new(&format!("/photos/2026/{i:03}")),
                )
                .unwrap()
            })
            .collect();
        for i in 0..PHOTOS {
            let name = format!("IMG_{i:06}.NEF");
            let time = format!(
                "2026-{:02}-{:02}T{:02}:{:02}:00",
                i % 12 + 1,
                i % 28 + 1,
                i % 24,
                i % 60
            );
            let file = repo::insert_file(
                conn,
                &NewFile {
                    folder_id: folders[i % folders.len()],
                    filename: &name,
                    ext: "nef",
                    kind: "raw",
                    size: Some(30_000_000),
                    width: Some(6000),
                    height: Some(4000),
                    orientation: Some(1),
                    capture_time: Some(&time),
                    camera_model: Some("D780"),
                    ..Default::default()
                },
            )
            .unwrap();
            repo::insert_photo(
                conn,
                &NewPhoto {
                    file_id: file,
                    import_id: None,
                },
            )
            .unwrap();
        }
        conn.execute_batch("COMMIT").unwrap();
    }

    let catalog = timed("reopen", Duration::from_secs(1), || {
        Catalog::create_or_open(&path).unwrap()
    });
    let conn = catalog.connection();
    timed("quick_check", Duration::from_secs(5), || {
        assert!(catalog.quick_check().unwrap())
    });
    let all = timed("list by capture time", Duration::from_secs(2), || {
        list_all_photos(conn, PhotoSort::CaptureTime).unwrap()
    });
    assert_eq!(all.len(), PHOTOS);
    timed("list by filename", Duration::from_secs(2), || {
        list_all_photos(conn, PhotoSort::Filename).unwrap()
    });
    let folders = repo::list_folders(conn).unwrap();
    let leaf = folders.iter().find(|f| f.path.ends_with("/000")).unwrap();
    timed("folder counts", Duration::from_secs(1), || {
        repo::list_folders(conn).unwrap()
    });
    let one = timed("one folder", Duration::from_millis(500), || {
        list_photos_for_folder(conn, leaf.id, PhotoSort::CaptureTime).unwrap()
    });
    assert_eq!(one.len(), PHOTOS / 200);
    timed("count", Duration::from_millis(200), || {
        repo::count_all_photos(conn).unwrap()
    });
}
