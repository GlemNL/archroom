//! `archroom-io`: decoders (LibRaw, image) and metadata (exiv2). The
//! `Decoder` contract lands now; the LibRaw and exiv2-backed implementations
//! are M1 work (plan roadmap).

mod types;

pub use types::{
    CameraColor, DecodeOptions, DecodedImage, Decoder, ImageF32, ImageMetadata, Priority,
};

// mod libraw;   // M1: the safe LibRaw wrapper (plan §6.3)
// mod image_rs; // M1: JPEG/PNG/TIFF via `image`
// mod metadata; // M1: exiv2-backed MetadataReader/MetadataWriter (plan §5.3)
