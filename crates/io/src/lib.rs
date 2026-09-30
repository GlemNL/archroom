//! `archroom-io`: decoders (LibRaw, image) and metadata (exiv2).

pub mod export_meta;
mod image_rs;
mod libraw;
mod metadata;
mod types;
pub mod xmp;

use std::path::Path;

pub use image_rs::ImageDecoder;
pub use libraw::RawDecoder;
pub use metadata::read_exif_summary;
pub use types::{
    CameraColor, DecodeOptions, DecodedImage, Decoder, ImageF32, ImageMetadata, Priority,
};

/// Picks a `Decoder` for `path` by extension (plan §4.3's decoder registry,
/// kept as a plain function until a second raw or image backend needs real
/// registration/priority arbitration).
pub fn decoder_for(path: &Path) -> Option<Box<dyn Decoder>> {
    if RawDecoder.probe(path, &[]).is_some() {
        Some(Box::new(RawDecoder))
    } else if ImageDecoder.probe(path, &[]).is_some() {
        Some(Box::new(ImageDecoder))
    } else {
        None
    }
}
