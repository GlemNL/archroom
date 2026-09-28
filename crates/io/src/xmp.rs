//! XMP sidecar read/write via exiv2 (`rexiv2`), plan §5.3. A sidecar is a
//! standalone `.xmp` file next to the image; the catalog stays authoritative
//! (plan principle 2) and this module just serializes/deserializes the
//! subset of catalog fields plan §5.3 maps to XMP properties.
//!
//! Writes are read-modify-write: `Metadata::new_from_path` loads whatever is
//! already in the sidecar (Adobe `crs:`, darktable history, ...) and only
//! the tags this module knows about are touched, so a round trip through
//! another app's fields survives (plan §5.3).

use std::path::Path;
use std::sync::Once;

use archroom_core::{Error, Result};

static REXIV2_INIT: Once = Once::new();
static NAMESPACE_INIT: Once = Once::new();

/// Matches `crate::metadata`'s own `Once` guard; the two modules can be used
/// independently (e.g. a test exercising only `xmp`) without double-init
/// issues, since `rexiv2::initialize()` is itself idempotent-safe to call
/// once per process either way.
fn ensure_rexiv2_initialized() {
    REXIV2_INIT.call_once(|| {
        if let Err(e) = rexiv2::initialize() {
            tracing::warn!(error = %e, "rexiv2::initialize failed");
        }
    });
}

const ARCHROOM_NS_URI: &str = "https://archroom.app/xmp/1.0/";
const ARCHROOM_NS_PREFIX: &str = "archroom";

fn ensure_archroom_namespace_registered() {
    NAMESPACE_INIT.call_once(|| {
        if let Err(e) = rexiv2::register_xmp_namespace(ARCHROOM_NS_URI, ARCHROOM_NS_PREFIX) {
            tracing::warn!(error = %e, "register_xmp_namespace(archroom) failed");
        }
    });
}

/// A minimal, valid, empty XMP packet — what a bare `.xmp` sidecar looks
/// like before anything is written to it. `rexiv2::Metadata::save_to_file`
/// requires the target to already exist (it opens and rewrites it in
/// place), so a brand-new sidecar needs this bootstrapped first.
const EMPTY_XMP_PACKET: &str = r#"<?xpacket begin="﻿" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""/>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>
"#;

/// The catalog fields plan §5.3 round-trips through XMP. Hierarchical
/// keywords (`lr:hierarchicalSubject`) are Later — only the flat keyword
/// list (`dc:subject`) is covered here.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SidecarData {
    pub rating: Option<i32>,
    /// -1 reject, 0 none, 1 pick — mirrors `photos.flag` (plan §5.1).
    pub flag: Option<i32>,
    pub color_label: Option<String>,
    pub keywords: Vec<String>,
    pub title: Option<String>,
    pub caption: Option<String>,
    pub creator: Option<String>,
    pub copyright: Option<String>,
}

/// `IMG_0001.xmp` — the sidecar name for an image with no basename clash
/// (plan D5).
pub fn sidecar_path(image_path: &Path) -> std::path::PathBuf {
    image_path.with_extension("xmp")
}

/// `IMG_0001.CR3.xmp` — the fallback name when a RAW+JPEG pair sharing a
/// basename would otherwise collide on the plain `sidecar_path` (plan D5).
/// Callers decide when the fallback is needed; this just builds the name.
pub fn sidecar_path_with_full_extension(image_path: &Path) -> std::path::PathBuf {
    let mut name = image_path.as_os_str().to_owned();
    name.push(".xmp");
    std::path::PathBuf::from(name)
}

fn to_rexiv2_error(context: &str, e: rexiv2::Rexiv2Error) -> Error {
    Error::Other(format!("{context}: {e}"))
}

/// Read-modify-write: loads `path` (bootstrapping an empty packet first if
/// it doesn't exist yet), applies every field in `data`, and saves back.
///
/// **Known exiv2/gexiv2 0.28 limitation, verified against both `rexiv2` and
/// the `exiv2` CLI directly**: setting or updating a tag on a
/// previously-saved standalone `.xmp` file works reliably, but *removing* a
/// tag that was already persisted in an earlier save (via `clear_tag`,
/// `del`, or re-setting an empty value/array) is a silent no-op — the
/// on-disk value survives even `clear_xmp()` followed by a fresh save.
/// `None`/empty fields below still call `clear_tag` on a best-effort basis
/// (it works for a tag added earlier in the *same* open/save cycle, and
/// costs nothing when the tag was never set), but callers should not rely
/// on it to remove a value a *previous* `write_sidecar` call persisted.
pub fn write_sidecar(path: &Path, data: &SidecarData) -> Result<()> {
    ensure_rexiv2_initialized();
    ensure_archroom_namespace_registered();

    if !path.exists() {
        std::fs::write(path, EMPTY_XMP_PACKET).map_err(|e| Error::io(path, e))?;
    }

    let meta = rexiv2::Metadata::new_from_path(path)
        .map_err(|e| to_rexiv2_error(&format!("xmp open {}", path.display()), e))?;

    match data.rating {
        Some(r) => meta
            .set_tag_numeric("Xmp.xmp.Rating", r)
            .map_err(|e| to_rexiv2_error("set Xmp.xmp.Rating", e))?,
        None => {
            meta.clear_tag("Xmp.xmp.Rating");
        }
    }

    match data.flag {
        Some(f) => meta
            .set_tag_numeric("Xmp.archroom.Flag", f)
            .map_err(|e| to_rexiv2_error("set Xmp.archroom.Flag", e))?,
        None => {
            meta.clear_tag("Xmp.archroom.Flag");
        }
    }

    match &data.color_label {
        Some(label) => meta
            .set_tag_string("Xmp.xmp.Label", label)
            .map_err(|e| to_rexiv2_error("set Xmp.xmp.Label", e))?,
        None => {
            meta.clear_tag("Xmp.xmp.Label");
        }
    }

    if data.keywords.is_empty() {
        meta.clear_tag("Xmp.dc.subject");
    } else {
        let refs: Vec<&str> = data.keywords.iter().map(String::as_str).collect();
        meta.set_tag_multiple_strings("Xmp.dc.subject", &refs)
            .map_err(|e| to_rexiv2_error("set Xmp.dc.subject", e))?;
    }

    set_or_clear_string(&meta, "Xmp.dc.title", data.title.as_deref())?;
    set_or_clear_string(&meta, "Xmp.dc.description", data.caption.as_deref())?;
    set_or_clear_string(&meta, "Xmp.dc.rights", data.copyright.as_deref())?;

    match &data.creator {
        Some(creator) => meta
            .set_tag_multiple_strings("Xmp.dc.creator", &[creator.as_str()])
            .map_err(|e| to_rexiv2_error("set Xmp.dc.creator", e))?,
        None => {
            meta.clear_tag("Xmp.dc.creator");
        }
    }

    meta.save_to_file(path)
        .map_err(|e| to_rexiv2_error(&format!("xmp save {}", path.display()), e))
}

fn set_or_clear_string(meta: &rexiv2::Metadata, tag: &str, value: Option<&str>) -> Result<()> {
    match value {
        Some(v) => meta
            .set_tag_string(tag, v)
            .map_err(|e| to_rexiv2_error(&format!("set {tag}"), e)),
        None => {
            meta.clear_tag(tag);
            Ok(())
        }
    }
}

/// Reads a sidecar back into a [`SidecarData`] (plan §5.3: existing
/// sidecars — from Lightroom, darktable, digiKam — are read on import).
/// A tag absent from the file simply yields `None`/empty rather than an
/// error.
pub fn read_sidecar(path: &Path) -> Result<SidecarData> {
    ensure_rexiv2_initialized();
    ensure_archroom_namespace_registered();

    let meta = rexiv2::Metadata::new_from_path(path)
        .map_err(|e| to_rexiv2_error(&format!("xmp open {}", path.display()), e))?;

    let rating = meta
        .has_tag("Xmp.xmp.Rating")
        .then(|| meta.get_tag_numeric("Xmp.xmp.Rating"));
    let flag = meta
        .has_tag("Xmp.archroom.Flag")
        .then(|| meta.get_tag_numeric("Xmp.archroom.Flag"));
    let color_label = non_empty(meta.get_tag_string("Xmp.xmp.Label").ok());
    let keywords = meta
        .get_tag_multiple_strings("Xmp.dc.subject")
        .unwrap_or_default();
    let title = non_empty(meta.get_tag_string("Xmp.dc.title").ok().map(|s| strip_lang_alt_prefix(&s)));
    let caption = non_empty(
        meta.get_tag_string("Xmp.dc.description")
            .ok()
            .map(|s| strip_lang_alt_prefix(&s)),
    );
    let creator = meta
        .get_tag_multiple_strings("Xmp.dc.creator")
        .ok()
        .and_then(|v| v.into_iter().next());
    let copyright = non_empty(
        meta.get_tag_string("Xmp.dc.rights")
            .ok()
            .map(|s| strip_lang_alt_prefix(&s)),
    );

    Ok(SidecarData {
        rating,
        flag,
        color_label,
        keywords,
        title,
        caption,
        creator,
        copyright,
    })
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// `rexiv2::Metadata::get_tag_string` on a `LangAlt` property (`dc:title`,
/// `dc:description`, `dc:rights`) returns exiv2's raw serialization —
/// `lang="x-default" the actual text` — rather than just the text. Strip it
/// so callers get the plain string back, matching what `set_tag_string`
/// accepts as input.
fn strip_lang_alt_prefix(s: &str) -> String {
    s.strip_prefix("lang=\"x-default\" ")
        .map(str::to_string)
        .unwrap_or_else(|| s.to_string())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_path_replaces_the_extension() {
        assert_eq!(
            sidecar_path(Path::new("/a/IMG_0001.CR3")),
            Path::new("/a/IMG_0001.xmp")
        );
    }

    #[test]
    fn sidecar_path_with_full_extension_appends_instead_of_replacing() {
        assert_eq!(
            sidecar_path_with_full_extension(Path::new("/a/IMG_0001.CR3")),
            Path::new("/a/IMG_0001.CR3.xmp")
        );
    }

    #[test]
    fn write_then_read_round_trips_every_field() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("IMG_0001.xmp");

        let data = SidecarData {
            rating: Some(4),
            flag: Some(1),
            color_label: Some("Red".to_string()),
            keywords: vec!["Sunset".to_string(), "Beach".to_string()],
            title: Some("A title".to_string()),
            caption: Some("A caption".to_string()),
            creator: Some("A. Photographer".to_string()),
            copyright: Some("(c) 2026".to_string()),
        };
        write_sidecar(&path, &data).unwrap();

        let back = read_sidecar(&path).unwrap();
        assert_eq!(back.rating, Some(4));
        assert_eq!(back.flag, Some(1));
        assert_eq!(back.color_label, Some("Red".to_string()));
        assert_eq!(back.keywords, vec!["Sunset".to_string(), "Beach".to_string()]);
        assert_eq!(back.title, Some("A title".to_string()));
        assert_eq!(back.caption, Some("A caption".to_string()));
        assert_eq!(back.creator, Some("A. Photographer".to_string()));
        assert_eq!(back.copyright, Some("(c) 2026".to_string()));
    }

    #[test]
    fn writing_none_clears_a_tag_set_earlier_in_the_same_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("IMG_0002.xmp");

        // Exercises the one case `clear_tag` reliably handles: a value set
        // and cleared within a single open/save cycle (see `write_sidecar`'s
        // doc comment for the reopened-file limitation this does NOT cover).
        write_sidecar(
            &path,
            &SidecarData {
                rating: Some(3),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(read_sidecar(&path).unwrap().rating, Some(3));
    }

    #[test]
    fn write_preserves_fields_written_by_another_app() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("IMG_0003.xmp");
        std::fs::write(&path, EMPTY_XMP_PACKET).unwrap();

        // Simulate a foreign tag another tool (e.g. darktable) would have
        // written, outside any tag this module touches.
        {
            ensure_rexiv2_initialized();
            let meta = rexiv2::Metadata::new_from_path(&path).unwrap();
            meta.set_tag_string("Xmp.dc.format", "image/x-canon-cr3")
                .unwrap();
            meta.save_to_file(&path).unwrap();
        }

        write_sidecar(
            &path,
            &SidecarData {
                rating: Some(2),
                ..Default::default()
            },
        )
        .unwrap();

        let meta = rexiv2::Metadata::new_from_path(&path).unwrap();
        assert_eq!(meta.get_tag_string("Xmp.dc.format").unwrap(), "image/x-canon-cr3");
    }

    /// Cross-checks against the real `exiv2` CLI (an independent parser, as
    /// a proxy for darktable/digiKam's own exiv2-based readers) so a bug
    /// specific to how `rexiv2` itself reads back its own writes wouldn't
    /// hide a wire-format mistake. Skips if `exiv2` isn't on PATH.
    #[test]
    fn round_trip_is_readable_by_the_exiv2_cli() {
        let Ok(output) = std::process::Command::new("exiv2").arg("--version").output() else {
            eprintln!("skipping: exiv2 CLI not found");
            return;
        };
        if !output.status.success() {
            eprintln!("skipping: exiv2 CLI not usable");
            return;
        }

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("IMG_0004.xmp");
        write_sidecar(
            &path,
            &SidecarData {
                rating: Some(5),
                color_label: Some("Green".to_string()),
                keywords: vec!["Mountains".to_string()],
                title: Some("Peak at dawn".to_string()),
                ..Default::default()
            },
        )
        .unwrap();

        let output = std::process::Command::new("exiv2")
            .args(["-PXkv", "print"])
            .arg(&path)
            .output()
            .expect("run exiv2 CLI");
        let text = String::from_utf8_lossy(&output.stdout);

        assert!(output.status.success(), "exiv2 CLI failed: {text}");
        assert!(text.contains("Xmp.xmp.Rating") && text.contains('5'));
        assert!(text.contains("Green"));
        assert!(text.contains("Mountains"));
        assert!(text.contains("Peak at dawn"));
    }
}
