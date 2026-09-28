//! M0 spike only (plan roadmap): the minimal LibRaw call sequence needed to
//! prove decode → GPU → display works end to end. This is deliberately not
//! the M1 `Decoder` implementation — no metadata, no embedded preview, no
//! error-code translation, no camera color matrix. It follows the raw-prep
//! recipe from plan §6.3 (raw camera space, linear gamma, no auto-bright,
//! reference-WB multipliers, AHD demosaic) so the M1 wrapper can reuse the
//! same LibRaw parameters once it replaces this file.

use std::ffi::CString;
use std::path::Path;

use libraw_sys as sys;

use crate::types::ImageF32;

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

fn libraw_err(code: i32) -> String {
    unsafe {
        let msg = sys::libraw_strerror(code);
        if msg.is_null() {
            format!("LibRaw error {code}")
        } else {
            std::ffi::CStr::from_ptr(msg).to_string_lossy().into_owned()
        }
    }
}

/// Decodes `path` to linear camera-RGB `f32`, demosaiced at a fixed
/// reference white balance (plan §6.3's "why demosaic at a fixed D65 white
/// balance").
pub fn decode_to_linear_rgb_f32(path: &Path) -> Result<ImageF32, String> {
    let handle = unsafe { sys::libraw_init(0) };
    if handle.is_null() {
        return Err("libraw_init returned null".to_string());
    }
    let handle = LibRawHandle(handle);

    let c_path = CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|e| format!("invalid path: {e}"))?;

    unsafe {
        let ret = sys::libraw_open_file(handle.0, c_path.as_ptr());
        if ret != 0 {
            return Err(format!("libraw_open_file: {}", libraw_err(ret)));
        }

        let ret = sys::libraw_unpack(handle.0);
        if ret != 0 {
            return Err(format!("libraw_unpack: {}", libraw_err(ret)));
        }

        // Reference-WB multipliers from the raw file itself (plan §6.3):
        // demosaic once at a fixed white, apply the real white balance
        // later as a chromatic adaptation on linear data (M3).
        let pre_mul = (*handle.0).color.pre_mul;

        let params = &mut (*handle.0).params;
        params.output_color = 0; // raw camera space, not sRGB/Adobe
        params.gamm[0] = 1.0; // linear (no gamma curve)
        params.gamm[1] = 1.0;
        params.no_auto_bright = 1;
        params.output_bps = 16;
        params.user_qual = 3; // AHD
        params.highlight = 0; // clip, so blown highlights stay neutral
        params.use_camera_wb = 0;
        params.use_auto_wb = 0;
        params.user_mul = pre_mul;

        let ret = sys::libraw_dcraw_process(handle.0);
        if ret != 0 {
            return Err(format!("libraw_dcraw_process: {}", libraw_err(ret)));
        }

        let mut err = 0i32;
        let image = sys::libraw_dcraw_make_mem_image(handle.0, &raw mut err);
        if image.is_null() || err != 0 {
            return Err(format!("libraw_dcraw_make_mem_image: {}", libraw_err(err)));
        }
        let image = ProcessedImage(image);
        let img = &*image.0;

        if img.bits != 16 {
            return Err(format!("expected 16-bit output, got {}-bit", img.bits));
        }
        let colors = img.colors as usize;
        if colors < 3 {
            return Err(format!("expected at least 3 color channels, got {colors}"));
        }

        let width = img.width as u32;
        let height = img.height as u32;
        let n_pixels = width as usize * height as usize;
        let expected_bytes = n_pixels * colors * 2;
        if (img.data_size as usize) < expected_bytes {
            return Err(format!(
                "data_size {} smaller than expected {expected_bytes}",
                img.data_size
            ));
        }

        let samples =
            std::slice::from_raw_parts(img.data.as_ptr().cast::<u16>(), n_pixels * colors);

        let mut data = vec![0f32; n_pixels * 4];
        for px in 0..n_pixels {
            let src = px * colors;
            let dst = px * 4;
            data[dst] = samples[src] as f32 / 65535.0;
            data[dst + 1] = samples[src + 1] as f32 / 65535.0;
            data[dst + 2] = samples[src + 2] as f32 / 65535.0;
            data[dst + 3] = 1.0;
        }

        Ok(ImageF32 {
            width,
            height,
            channels: 4,
            data,
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "needs ARCHROOM_SPIKE_RAW pointing at a real raw file"]
    fn decodes_a_real_raw_file() {
        let path = std::env::var("ARCHROOM_SPIKE_RAW").expect("set ARCHROOM_SPIKE_RAW");
        let img = decode_to_linear_rgb_f32(std::path::Path::new(&path)).expect("decode");
        eprintln!(
            "decoded {}x{} ({} floats)",
            img.width,
            img.height,
            img.data.len()
        );
        assert!(img.width > 0 && img.height > 0);
        assert_eq!(img.data.len(), img.width as usize * img.height as usize * 4);
    }
}
