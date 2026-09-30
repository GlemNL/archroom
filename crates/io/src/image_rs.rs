//! The `Decoder` for JPEG/PNG/TIFF via the `image` crate (plan roadmap M1).
//! `image` doesn't expose embedded ICC profiles, so this module extracts
//! them itself: JPEG APP2 `ICC_PROFILE` markers, TIFF tag `0x8773`. Both are
//! best-effort — `None` on anything absent or malformed, which
//! `DecodedImage::Rendered.icc: Option<Vec<u8>>` already expects.

use std::path::Path;

use viberoom_core::{Error, Result};

use crate::types::{DecodeOptions, DecodedImage, Decoder, ImageF32, ImageMetadata, Priority};

#[derive(Debug)]
pub struct ImageDecoder;

impl Decoder for ImageDecoder {
    fn id(&self) -> &'static str {
        "image"
    }

    fn probe(&self, path: &Path, _header: &[u8]) -> Option<Priority> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "tif" | "tiff").then_some(Priority(50))
    }

    fn metadata(&self, path: &Path) -> Result<ImageMetadata> {
        crate::metadata::read_exif_summary(path)
    }

    fn embedded_preview(&self, _path: &Path) -> Result<Option<Vec<u8>>> {
        // Nothing "embedded" beyond the image itself for these formats;
        // preview generation decodes the file directly (plan §5.4).
        Ok(None)
    }

    fn decode(&self, path: &Path, _opts: &DecodeOptions) -> Result<DecodedImage> {
        let bytes = std::fs::read(path).map_err(|e| Error::io(path, e))?;
        let icc = extract_icc(path, &bytes);

        let img = image::load_from_memory(&bytes)
            .map_err(|e| Error::Other(format!("decode {}: {e}", path.display())))?;
        // Display-referred samples as-is (not linearized) — the ICC profile
        // is what a later color-management pass (M3) uses to interpret
        // them.
        let rgba = img.to_rgba32f();
        let (width, height) = (rgba.width(), rgba.height());

        Ok(DecodedImage::Rendered {
            rgb: ImageF32 {
                width,
                height,
                channels: 4,
                data: rgba.into_raw(),
            },
            icc,
        })
    }
}

fn extract_icc(path: &Path, bytes: &[u8]) -> Option<Vec<u8>> {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("jpg" | "jpeg") => extract_jpeg_icc(bytes),
        Some("tif" | "tiff") => extract_tiff_icc(bytes),
        _ => None,
    }
}

/// Concatenates JPEG APP2 `ICC_PROFILE` segments in `seq` order (a profile
/// larger than one ~64KB segment is split across several).
fn extract_jpeg_icc(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.len() < 4 || bytes[0..2] != [0xFF, 0xD8] {
        return None;
    }
    let mut chunks: Vec<(u8, Vec<u8>)> = Vec::new();
    let mut pos = 2;
    while pos + 4 <= bytes.len() {
        if bytes[pos] != 0xFF {
            break;
        }
        let marker = bytes[pos + 1];
        // Markers with no payload (RSTn, SOI, EOI have no length field).
        if marker == 0xD8 || marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            pos += 2;
            continue;
        }
        if marker == 0xD9 {
            break; // EOI
        }
        let seg_len = u16::from_be_bytes([bytes[pos + 2], bytes[pos + 3]]) as usize;
        if seg_len < 2 || pos + 2 + seg_len > bytes.len() {
            break;
        }
        let payload = &bytes[pos + 4..pos + 2 + seg_len];
        if marker == 0xE2 && payload.len() > 14 && &payload[0..12] == b"ICC_PROFILE\0" {
            chunks.push((payload[12], payload[14..].to_vec()));
        }
        if marker == 0xDA {
            break; // start of scan: ICC markers always precede this
        }
        pos += 2 + seg_len;
    }
    if chunks.is_empty() {
        return None;
    }
    chunks.sort_by_key(|(seq, _)| *seq);
    Some(chunks.into_iter().flat_map(|(_, data)| data).collect())
}

fn extract_tiff_icc(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.len() < 8 {
        return None;
    }
    let le = match &bytes[0..2] {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let u16_at = |o: usize| -> Option<u16> {
        let b = bytes.get(o..o + 2)?;
        Some(if le {
            u16::from_le_bytes([b[0], b[1]])
        } else {
            u16::from_be_bytes([b[0], b[1]])
        })
    };
    let u32_at = |o: usize| -> Option<u32> {
        let b = bytes.get(o..o + 4)?;
        Some(if le {
            u32::from_le_bytes([b[0], b[1], b[2], b[3]])
        } else {
            u32::from_be_bytes([b[0], b[1], b[2], b[3]])
        })
    };

    let ifd_offset = u32_at(4)? as usize;
    let count = u16_at(ifd_offset)? as usize;
    let entries_start = ifd_offset + 2;

    for i in 0..count {
        let entry = entries_start + i * 12;
        if bytes.len() < entry + 12 {
            break;
        }
        const ICC_TAG: u16 = 0x8773;
        if u16_at(entry)? != ICC_TAG {
            continue;
        }
        let field_type = u16_at(entry + 2)?;
        let field_count = u32_at(entry + 4)? as usize;
        let type_size = match field_type {
            1 | 2 | 6 | 7 => 1,
            3 | 8 => 2,
            4 | 9 | 11 => 4,
            5 | 10 | 12 => 8,
            _ => 1,
        };
        let total = field_count * type_size;
        let value_offset = if total <= 4 {
            entry + 8
        } else {
            u32_at(entry + 8)? as usize
        };
        return bytes
            .get(value_offset..value_offset + total)
            .map(<[u8]>::to_vec);
    }
    None
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn fake_jpeg_with_icc(profile: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0xFF, 0xD8]; // SOI
        let mut payload = b"ICC_PROFILE\0".to_vec();
        payload.push(1); // seq
        payload.push(1); // total
        payload.extend_from_slice(profile);
        let seg_len = (payload.len() + 2) as u16;
        bytes.push(0xFF);
        bytes.push(0xE2); // APP2
        bytes.extend_from_slice(&seg_len.to_be_bytes());
        bytes.extend_from_slice(&payload);
        bytes.extend_from_slice(&[0xFF, 0xD9]); // EOI
        bytes
    }

    #[test]
    fn extracts_icc_from_a_single_app2_segment() {
        let profile = b"fake-icc-bytes";
        let jpeg = fake_jpeg_with_icc(profile);
        assert_eq!(extract_jpeg_icc(&jpeg), Some(profile.to_vec()));
    }

    #[test]
    fn missing_icc_segment_yields_none() {
        let jpeg = vec![0xFF, 0xD8, 0xFF, 0xD9];
        assert_eq!(extract_jpeg_icc(&jpeg), None);
    }

    #[test]
    fn probe_matches_rendered_extensions() {
        let d = ImageDecoder;
        assert!(d.probe(Path::new("a.JPG"), &[]).is_some());
        assert!(d.probe(Path::new("a.png"), &[]).is_some());
        assert!(d.probe(Path::new("a.tiff"), &[]).is_some());
        assert!(d.probe(Path::new("a.nef"), &[]).is_none());
    }

    #[test]
    fn decodes_a_synthetic_png_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.png");
        let img = image::RgbImage::from_fn(4, 3, |x, _y| image::Rgb([x as u8 * 50, 10, 20]));
        img.save(&path).unwrap();

        let d = ImageDecoder;
        let decoded = d.decode(&path, &DecodeOptions::default()).unwrap();
        match decoded {
            DecodedImage::Rendered { rgb, icc } => {
                assert_eq!((rgb.width, rgb.height), (4, 3));
                assert!(icc.is_none());
            }
            DecodedImage::SceneLinear { .. } => panic!("PNG must decode to Rendered"),
        }
    }
}
