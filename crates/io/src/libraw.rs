//! The production `Decoder` for raw files (plan roadmap M1: "`libraw-sys`
//! plus a safe wrapper: open, metadata, embedded thumbnail, process").

use rayon::prelude::*;
use std::ffi::CString;
use std::path::Path;

use viberoom_color::Mat3;
use viberoom_core::{Error, Result};
use libraw_sys as sys;

use crate::types::{
    CameraColor, DecodeOptions, DecodedImage, Decoder, ImageF32, ImageMetadata, Priority,
};

const RAW_EXTENSIONS: &[&str] = &[
    "nef", "cr2", "cr3", "arw", "raf", "rw2", "orf", "pef", "srw", "dng", "raw",
];

#[derive(Debug)]
pub struct RawDecoder;

impl Decoder for RawDecoder {
    fn id(&self) -> &'static str {
        "libraw"
    }

    fn probe(&self, path: &Path, _header: &[u8]) -> Option<Priority> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        RAW_EXTENSIONS
            .contains(&ext.as_str())
            .then_some(Priority(100))
    }

    fn metadata(&self, path: &Path) -> Result<ImageMetadata> {
        let handle = open_and_identify(path)?;
        Ok(read_metadata(&handle))
    }

    fn embedded_preview(&self, path: &Path) -> Result<Option<Vec<u8>>> {
        let handle = open_and_identify(path)?;
        unsafe {
            let ret = sys::libraw_unpack_thumb(handle.0);
            if ret != 0 {
                // Not every raw carries an embedded thumbnail; that's not
                // an error the caller needs to see (plan §5.4: preview
                // generation falls back to a decode when there's none).
                return Ok(None);
            }

            let mut err = 0i32;
            let thumb = sys::libraw_dcraw_make_mem_thumb(handle.0, &raw mut err);
            if thumb.is_null() || err != 0 {
                return Ok(None);
            }
            let thumb = ProcessedImage(thumb);
            let img = &*thumb.0;

            if img.type_ != sys::LibRaw_image_formats::LIBRAW_IMAGE_JPEG {
                // A raw (non-JPEG) thumbnail would need re-encoding to be
                // useful as an L0 preview; skip it (M1 falls back to a
                // decode instead, plan §5.4).
                return Ok(None);
            }
            let bytes =
                std::slice::from_raw_parts(img.data.as_ptr(), img.data_size as usize).to_vec();
            Ok(Some(bytes))
        }
    }

    fn decode(&self, path: &Path, _opts: &DecodeOptions) -> Result<DecodedImage> {
        let handle = open_and_identify(path)?;
        unsafe {
            let ret = sys::libraw_unpack(handle.0);
            if ret != 0 {
                return Err(libraw_error("libraw_unpack", ret));
            }

            let camera = read_camera_color(&handle);

            // Plan §6.3: demosaic once at the reference white balance, in raw
            // camera space, linear light. `max_long_edge` proxy sizing is
            // M3 work (the raw-prep/proxy pipeline); Phase A always decodes
            // full resolution.
            let pre_mul = (*handle.0).color.pre_mul;
            let params = &mut (*handle.0).params;
            params.output_color = 0;
            params.gamm[0] = 1.0;
            params.gamm[1] = 1.0;
            params.no_auto_bright = 1;
            params.output_bps = 16;
            params.user_qual = 3;
            params.highlight = 0;
            params.use_camera_wb = 0;
            params.use_auto_wb = 0;
            params.user_mul = pre_mul;
            // Sensor-native pixels, not auto-rotated: `metadata()` reports
            // `sizes.width`/`height` from *before* any flip is applied, and
            // `files.orientation` (plan §5.1) is meant to be a display-time
            // transform, not baked into the decode. Without this, a
            // portrait shot decodes pre-rotated and silently disagrees with
            // its own catalogued width/height.
            params.user_flip = 0;

            let ret = sys::libraw_dcraw_process(handle.0);
            if ret != 0 {
                return Err(libraw_error("libraw_dcraw_process", ret));
            }

            let mut err = 0i32;
            let image = sys::libraw_dcraw_make_mem_image(handle.0, &raw mut err);
            if image.is_null() || err != 0 {
                return Err(libraw_error("libraw_dcraw_make_mem_image", err));
            }
            let image = ProcessedImage(image);
            let img = &*image.0;

            if img.bits != 16 {
                return Err(Error::Other(format!(
                    "expected 16-bit output, got {}-bit",
                    img.bits
                )));
            }
            let colors = img.colors as usize;
            if colors < 3 {
                return Err(Error::Other(format!(
                    "expected at least 3 color channels, got {colors}"
                )));
            }

            let width = img.width as u32;
            let height = img.height as u32;
            let n_pixels = width as usize * height as usize;
            let expected_bytes = n_pixels * colors * 2;
            if (img.data_size as usize) < expected_bytes {
                return Err(Error::Other(format!(
                    "data_size {} smaller than expected {expected_bytes}",
                    img.data_size
                )));
            }

            let samples =
                std::slice::from_raw_parts(img.data.as_ptr().cast::<u16>(), n_pixels * colors);
            let mut data = vec![0f32; n_pixels * 4];
            data.par_chunks_mut(4 * 4096)
                .enumerate()
                .for_each(|(chunk, out)| {
                    for (i, px) in out.chunks_exact_mut(4).enumerate() {
                        let src = (chunk * 4096 + i) * colors;
                        px[0] = samples[src] as f32 / 65535.0;
                        px[1] = samples[src + 1] as f32 / 65535.0;
                        px[2] = samples[src + 2] as f32 / 65535.0;
                        px[3] = 1.0;
                    }
                });

            Ok(DecodedImage::SceneLinear {
                rgb: ImageF32 {
                    width,
                    height,
                    channels: 4,
                    data,
                },
                camera,
            })
        }
    }
}

struct LibRawHandle(*mut sys::libraw_data_t);

impl Drop for LibRawHandle {
    fn drop(&mut self) {
        unsafe { sys::libraw_close(self.0) };
    }
}

struct ProcessedImage(*mut sys::libraw_processed_image_t);

impl Drop for ProcessedImage {
    fn drop(&mut self) {
        unsafe { sys::libraw_dcraw_clear_mem(self.0) };
    }
}

fn libraw_error(call: &str, code: i32) -> Error {
    let msg = unsafe {
        let msg = sys::libraw_strerror(code);
        if msg.is_null() {
            format!("error {code}")
        } else {
            std::ffi::CStr::from_ptr(msg).to_string_lossy().into_owned()
        }
    };
    Error::Other(format!("{call}: {msg}"))
}

/// Opens the file and runs LibRaw's header/EXIF identification pass —
/// cheap, no pixel decode — which is enough for `metadata` and
/// `embedded_preview`.
fn open_and_identify(path: &Path) -> Result<LibRawHandle> {
    let handle = unsafe { sys::libraw_init(0) };
    if handle.is_null() {
        return Err(Error::Other("libraw_init returned null".into()));
    }
    let handle = LibRawHandle(handle);

    let c_path = CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|e| Error::Other(format!("invalid path {}: {e}", path.display())))?;

    let ret = unsafe { sys::libraw_open_file(handle.0, c_path.as_ptr()) };
    if ret != 0 {
        return Err(libraw_error("libraw_open_file", ret));
    }
    Ok(handle)
}

fn c_char_array_to_string(bytes: &[std::os::raw::c_char]) -> Option<String> {
    let bytes: &[u8] = unsafe { std::slice::from_raw_parts(bytes.as_ptr().cast(), bytes.len()) };
    let nul = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    let s = String::from_utf8_lossy(&bytes[..nul]).trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// Maps LibRaw's `sizes.flip` (0/3/5/6 in practice) to the EXIF orientation
/// convention `files.orientation` uses elsewhere in the catalog.
fn flip_to_exif_orientation(flip: i32) -> i32 {
    match flip {
        0 => 1, // normal
        3 => 3, // 180
        5 => 8, // 90 CCW
        6 => 6, // 90 CW
        _ => 1,
    }
}

fn unix_to_iso8601(secs: i64) -> Option<String> {
    if secs <= 0 {
        return None;
    }
    let days = secs.div_euclid(86_400);
    let secs_of_day = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    let (h, mi, s) = (
        secs_of_day / 3600,
        (secs_of_day / 60) % 60,
        secs_of_day % 60,
    );
    Some(format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}"))
}

/// Howard Hinnant's `civil_from_days`: days since the Unix epoch to a
/// proleptic-Gregorian (year, month, day). Self-contained so Phase A
/// doesn't need a date/time crate for the one timestamp LibRaw hands back
/// as a raw `time_t`.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn read_metadata(handle: &LibRawHandle) -> ImageMetadata {
    let data = unsafe { &*handle.0 };

    ImageMetadata {
        width: data.sizes.width as u32,
        height: data.sizes.height as u32,
        orientation: flip_to_exif_orientation(data.sizes.flip),
        capture_time: unix_to_iso8601(data.other.timestamp),
        camera_make: c_char_array_to_string(&data.idata.make),
        camera_model: c_char_array_to_string(&data.idata.model),
        lens: c_char_array_to_string(&data.lens.Lens),
        focal_length: (data.other.focal_len > 0.0).then_some(data.other.focal_len),
        aperture: (data.other.aperture > 0.0).then_some(data.other.aperture),
        shutter: (data.other.shutter > 0.0).then_some(data.other.shutter),
        iso: (data.other.iso_speed > 0.0).then_some(data.other.iso_speed as u32),
        gps: (data.other.parsed_gps.gpsparsed != 0)
            .then_some(())
            .and_then(|()| {
                let dms_to_decimal =
                    |dms: [f32; 3]| dms[0] as f64 + dms[1] as f64 / 60.0 + dms[2] as f64 / 3600.0;
                let mut lat = dms_to_decimal(data.other.parsed_gps.latitude);
                let mut lon = dms_to_decimal(data.other.parsed_gps.longitude);
                if lat == 0.0 && lon == 0.0 {
                    // A camera with no GPS fix still writes an (empty) GPSInfo
                    // IFD on some models, which parses as "parsed" with all
                    // fields zero. (0, 0) is open ocean, never a real photo.
                    return None;
                }
                if data.other.parsed_gps.latref == b'S' as i8 {
                    lat = -lat;
                }
                if data.other.parsed_gps.longref == b'W' as i8 {
                    lon = -lon;
                }
                Some((lat, lon))
            }),
    }
}

/// Camera RGB → XYZ D65 plus the as-shot-neutral multipliers it was built
/// relative to (plan §6.3/§6.4). Both come from the same reference-WB frame
/// `decode` demosaics into, so a later chromatic-adaptation step (M3) has a
/// consistent basis. Falls back to identity if LibRaw couldn't derive a
/// matrix for this camera (rare, but `cam_xyz` is then all zero).
fn read_camera_color(handle: &LibRawHandle) -> CameraColor {
    let data = unsafe { &*handle.0 };
    let m = data.color.cam_xyz;
    let is_zero = m[0..3].iter().all(|row| row.iter().all(|&v| v == 0.0));
    let xyz_to_camera = if is_zero {
        Mat3::IDENTITY
    } else {
        Mat3([m[0], m[1], m[2]])
    };
    let p = data.color.pre_mul;
    let c = data.color.cam_mul;
    let as_shot_mul =
        (c[0] > 0.0 && c[1] > 0.0 && c[2] > 0.0).then(|| [c[0] / c[1], 1.0, c[2] / c[1]]);
    CameraColor {
        xyz_to_camera,
        d65_mul: [p[0], p[1], p[2]],
        as_shot_mul,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn flip_maps_to_known_exif_orientations() {
        assert_eq!(flip_to_exif_orientation(0), 1);
        assert_eq!(flip_to_exif_orientation(3), 3);
        assert_eq!(flip_to_exif_orientation(6), 6);
    }

    #[test]
    fn unix_epoch_converts_to_iso8601() {
        // 2024-01-15T10:30:00Z
        assert_eq!(
            unix_to_iso8601(1_705_314_600),
            Some("2024-01-15T10:30:00".to_string())
        );
    }

    #[test]
    fn probe_matches_known_raw_extensions_case_insensitively() {
        let d = RawDecoder;
        assert!(d.probe(Path::new("IMG_0001.NEF"), &[]).is_some());
        assert!(d.probe(Path::new("img.jpg"), &[]).is_none());
    }

    #[test]
    #[ignore = "needs VIBEROOM_TEST_RAW pointing at a real raw file"]
    fn decodes_metadata_and_pixels_from_a_real_raw_file() {
        let path = std::env::var("VIBEROOM_TEST_RAW").expect("set VIBEROOM_TEST_RAW");
        let path = Path::new(&path);
        let d = RawDecoder;

        let meta = d.metadata(path).expect("metadata");
        assert!(meta.width > 0 && meta.height > 0);
        eprintln!("{meta:?}");

        let preview = d.embedded_preview(path).expect("embedded preview");
        eprintln!("embedded preview: {} bytes", preview.map_or(0, |p| p.len()));

        let decoded = d.decode(path, &DecodeOptions::default()).expect("decode");
        match decoded {
            DecodedImage::SceneLinear { rgb, camera } => {
                assert_eq!(rgb.width, meta.width);
                eprintln!("camera color: {camera:?}");
            }
            DecodedImage::Rendered { .. } => panic!("a raw must decode to SceneLinear"),
        }
    }
}
