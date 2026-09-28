use std::path::Path;

use archroom_color::Mat3;

/// A linear-light f32 image buffer, interleaved RGB(A).
#[derive(Debug, Clone)]
pub struct ImageF32 {
    pub width: u32,
    pub height: u32,
    pub channels: u8,
    pub data: Vec<f32>,
}

/// The camera color model needed to map raw camera RGB to a working space
/// (plan §6.3/§6.4): the DNG-style D65 color matrix and the as-shot white
/// balance multipliers it was built relative to.
#[derive(Debug, Clone)]
pub struct CameraColor {
    pub camera_to_xyz_d65: Mat3,
    pub as_shot_neutral: [f32; 3],
}

#[derive(Debug, Clone)]
pub enum DecodedImage {
    /// Raws: linear camera RGB + camera matrix + as-shot white balance.
    SceneLinear { rgb: ImageF32, camera: CameraColor },
    /// JPEG/TIFF/PNG: display-referred pixels + embedded ICC profile.
    Rendered { rgb: ImageF32, icc: Option<Vec<u8>> },
}

/// Metadata normalized across raw and rendered formats (plan §5.1's `files`
/// columns), independent of which backend (LibRaw, exiv2) produced it.
#[derive(Debug, Clone, Default)]
pub struct ImageMetadata {
    pub width: u32,
    pub height: u32,
    pub orientation: i32,
    pub capture_time: Option<String>,
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub lens: Option<String>,
    pub focal_length: Option<f32>,
    pub aperture: Option<f32>,
    pub shutter: Option<f32>,
    pub iso: Option<u32>,
    pub gps: Option<(f64, f64)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Priority(pub u8);

pub struct DecodeOptions {
    /// `None` decodes at full resolution; `Some(n)` requests an
    /// approximately `n`-px-long-edge proxy where the decoder can produce
    /// one cheaply (plan §6.10: proxy editing).
    pub max_long_edge: Option<u32>,
}

/// A source format backend (plan §4.3/§4.4). LibRaw and the `image`-based
/// backend are M1 work; this is the contract they implement.
pub trait Decoder: Send + Sync {
    fn id(&self) -> &'static str;
    fn probe(&self, path: &Path, header: &[u8]) -> Option<Priority>;
    fn metadata(&self, path: &Path) -> archroom_core::Result<ImageMetadata>;
    fn embedded_preview(&self, path: &Path) -> archroom_core::Result<Option<Vec<u8>>>;
    fn decode(&self, path: &Path, opts: &DecodeOptions) -> archroom_core::Result<DecodedImage>;
}
