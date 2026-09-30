//! File encoders for export (plan §9): JPEG, TIFF (8/16-bit, none/LZW/ZIP)
//! and PNG (8/16-bit), each embedding the delivery space's ICC profile.

use std::fs::File;
use std::io::{BufWriter, Seek, Write};
use std::path::Path;

use image::ExtendedColorType;
use image::ImageEncoder;
use image::codecs::jpeg::{JpegEncoder, PixelDensity};
use image::codecs::png::PngEncoder;
use tiff::encoder::colortype::{RGB8, RGB16};
use tiff::encoder::compression::DeflateLevel;
use tiff::encoder::{Compression, TiffEncoder};
use tiff::tags::{ResolutionUnit, Tag};

use super::{ExportSettings, Format, TiffCompression};
use crate::error::{Error, Result};

/// The rendered pixels, RGBA with an opaque alpha the encoders drop.
#[derive(Debug)]
pub enum Pixels {
    Rgba8(Vec<u8>),
    Rgba16(Vec<u16>),
}

/// TIFF tag 34675, `InterColorProfile`.
const TAG_ICC: u16 = 34675;

fn other(e: impl std::fmt::Display) -> Error {
    Error::Other(e.to_string())
}

fn rgb8(rgba: &[u8]) -> Vec<u8> {
    rgba.chunks_exact(4)
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect()
}

fn rgb16(rgba: &[u16]) -> Vec<u16> {
    rgba.chunks_exact(4)
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect()
}

/// Encodes `pixels` (`w`×`h`) to `path`, embedding `icc`.
pub fn encode_to_file(
    path: &Path,
    w: u32,
    h: u32,
    pixels: &Pixels,
    settings: &ExportSettings,
    icc: &[u8],
) -> Result<()> {
    let file = File::create(path).map_err(|e| Error::io(path, e))?;
    let mut out = BufWriter::new(file);
    match settings.format {
        Format::Jpeg => {
            let Pixels::Rgba8(rgba) = pixels else {
                return Err(other("JPEG export needs 8-bit pixels"));
            };
            let mut enc =
                JpegEncoder::new_with_quality(&mut out, settings.jpeg_quality.clamp(1, 100));
            enc.set_icc_profile(icc.to_vec()).map_err(other)?;
            if settings.ppi > 0 {
                enc.set_pixel_density(PixelDensity::dpi(settings.ppi.min(u16::MAX as u32) as u16));
            }
            enc.write_image(&rgb8(rgba), w, h, ExtendedColorType::Rgb8)
                .map_err(other)?;
        }
        Format::Png => {
            let mut enc = PngEncoder::new(&mut out);
            enc.set_icc_profile(icc.to_vec()).map_err(other)?;
            match pixels {
                Pixels::Rgba8(rgba) => enc.write_image(&rgb8(rgba), w, h, ExtendedColorType::Rgb8),
                Pixels::Rgba16(rgba) => {
                    // The image crate takes 16-bit samples as native-endian bytes.
                    let bytes: Vec<u8> = rgb16(rgba).iter().flat_map(|v| v.to_ne_bytes()).collect();
                    enc.write_image(&bytes, w, h, ExtendedColorType::Rgb16)
                }
            }
            .map_err(other)?;
        }
        Format::Tiff => encode_tiff(&mut out, w, h, pixels, settings, icc)?,
    }
    out.flush().map_err(|e| Error::io(path, e))?;
    out.get_ref().sync_all().map_err(|e| Error::io(path, e))
}

fn encode_tiff<W: Write + Seek>(
    out: &mut W,
    w: u32,
    h: u32,
    pixels: &Pixels,
    settings: &ExportSettings,
    icc: &[u8],
) -> Result<()> {
    let compression = match settings.tiff_compression {
        TiffCompression::None => Compression::Uncompressed,
        TiffCompression::Lzw => Compression::Lzw,
        TiffCompression::Zip => Compression::Deflate(DeflateLevel::Balanced),
    };
    let mut enc = TiffEncoder::new(out)
        .map_err(other)?
        .with_compression(compression);
    macro_rules! write {
        ($ct:ty, $data:expr) => {{
            let mut img = enc.new_image::<$ct>(w, h).map_err(other)?;
            if settings.ppi > 0 {
                img.resolution(
                    ResolutionUnit::Inch,
                    tiff::encoder::Rational {
                        n: settings.ppi,
                        d: 1,
                    },
                );
            }
            img.encoder()
                .write_tag(Tag::Unknown(TAG_ICC), icc)
                .map_err(other)?;
            img.write_data($data).map_err(other)?;
        }};
    }
    match pixels {
        Pixels::Rgba8(rgba) => write!(RGB8, &rgb8(rgba)),
        Pixels::Rgba16(rgba) => write!(RGB16, &rgb16(rgba)),
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use archroom_color::icc::OutputSpace;

    fn gradient16(w: u32, h: u32) -> Vec<u16> {
        (0..w * h)
            .flat_map(|i| {
                let x = (i % w) as f32 / (w - 1) as f32;
                let y = (i / w) as f32 / (h - 1) as f32;
                [(x * 65535.0) as u16, (y * 65535.0) as u16, 32768, 65535]
            })
            .collect()
    }

    fn contains(hay: &[u8], needle: &[u8]) -> bool {
        hay.windows(needle.len()).any(|w| w == needle)
    }

    #[test]
    fn jpeg_embeds_the_profile_and_decodes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.jpg");
        let rgba: Vec<u8> = (0..16 * 8).flat_map(|i| [i as u8, 100, 50, 255]).collect();
        let icc = OutputSpace::DisplayP3.icc_bytes().unwrap();
        let s = ExportSettings::default();
        encode_to_file(&path, 16, 8, &Pixels::Rgba8(rgba), &s, &icc).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(contains(&bytes, b"ICC_PROFILE"));
        let img = image::open(&path).unwrap();
        assert_eq!((img.width(), img.height()), (16, 8));
    }

    #[test]
    fn png16_round_trips_exactly_with_a_profile() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.png");
        let px = gradient16(16, 8);
        let icc = OutputSpace::AdobeRgb.icc_bytes().unwrap();
        let s = ExportSettings {
            format: Format::Png,
            bit_depth: 16,
            ..Default::default()
        };
        encode_to_file(&path, 16, 8, &Pixels::Rgba16(px.clone()), &s, &icc).unwrap();
        assert!(contains(&std::fs::read(&path).unwrap(), b"iCCP"));
        let back = image::open(&path).unwrap().into_rgb16();
        for (i, p) in back.pixels().enumerate() {
            assert_eq!(p.0, [px[i * 4], px[i * 4 + 1], px[i * 4 + 2]]);
        }
    }

    #[test]
    fn tiff16_round_trips_for_every_compression_and_keeps_the_profile() {
        let px = gradient16(32, 16);
        let icc = OutputSpace::ProPhoto.icc_bytes().unwrap();
        for comp in [
            TiffCompression::None,
            TiffCompression::Lzw,
            TiffCompression::Zip,
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("a.tif");
            let s = ExportSettings {
                format: Format::Tiff,
                bit_depth: 16,
                tiff_compression: comp,
                ..Default::default()
            };
            encode_to_file(&path, 32, 16, &Pixels::Rgba16(px.clone()), &s, &icc).unwrap();
            let mut dec = tiff::decoder::Decoder::new(File::open(&path).unwrap()).unwrap();
            assert_eq!(dec.dimensions().unwrap(), (32, 16));
            assert_eq!(
                dec.get_tag_u8_vec(Tag::Unknown(TAG_ICC)).unwrap(),
                icc,
                "{comp:?}"
            );
            let tiff::decoder::DecodingResult::U16(data) = dec.read_image().unwrap() else {
                panic!("not 16-bit");
            };
            for (i, p) in data.chunks_exact(3).enumerate() {
                assert_eq!(
                    p,
                    [px[i * 4], px[i * 4 + 1], px[i * 4 + 2]],
                    "{comp:?} px {i}"
                );
            }
        }
    }
}
