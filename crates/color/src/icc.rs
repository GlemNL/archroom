//! `lcms2` wrapper (plan §6.9): the display transform is built as a 3D LUT
//! so a monitor ICC profile later is just a different LUT.

use lcms2::{
    CIExyY, CIExyYTRIPLE, Intent, Locale, MLU, PixelFormat, Profile, TagSignature, ToneCurve,
    Transform,
};

use crate::cie::{ADOBE_RGB, DISPLAY_P3, PROPHOTO, Primaries, SRGB, Xy};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("lcms2: {0}")]
    Lcms(#[from] lcms2::Error),
}

fn xyy(xy: Xy) -> CIExyY {
    CIExyY {
        x: xy.x,
        y: xy.y,
        Y: 1.0,
    }
}

/// A linear-gamma RGB profile for `p` (our working spaces are linear).
pub fn linear_profile(p: &Primaries) -> Result<Profile, Error> {
    let curve = ToneCurve::new(1.0);
    let prim = CIExyYTRIPLE {
        Red: xyy(p.r),
        Green: xyy(p.g),
        Blue: xyy(p.b),
    };
    Ok(Profile::new_rgb(
        &xyy(p.white),
        &prim,
        &[&curve, &curve, &curve],
    )?)
}

/// An `n`³ LUT mapping display-referred *linear working RGB* (input grid is
/// sRGB-encoded so samples are perceptually spaced) to the destination
/// profile's encoded values. Layout: red fastest, then green, then blue;
/// `n * n * n * 3` floats.
pub fn build_display_lut(working: &Primaries, dest: &Profile, n: usize) -> Result<Vec<f32>, Error> {
    let src = linear_profile(working)?;
    let xf: Transform<[f32; 3], [f32; 3]> = Transform::new(
        &src,
        PixelFormat::RGB_FLT,
        dest,
        PixelFormat::RGB_FLT,
        Intent::RelativeColorimetric,
    )?;
    let denom = (n - 1).max(1) as f32;
    let mut input = Vec::with_capacity(n * n * n);
    for b in 0..n {
        for g in 0..n {
            for r in 0..n {
                input.push([r, g, b].map(|i| crate::srgb_eotf(i as f32 / denom)));
            }
        }
    }
    let mut out = vec![[0.0f32; 3]; input.len()];
    xf.transform_pixels(&input, &mut out);
    Ok(out.into_iter().flatten().collect())
}

/// The built-in sRGB destination.
pub fn srgb_profile() -> Profile {
    Profile::new_srgb()
}

/// A delivery color space for exported files (plan §9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum OutputSpace {
    Srgb,
    DisplayP3,
    /// Adobe RGB (1998)-compatible (gamma 2.2, D65).
    AdobeRgb,
    ProPhoto,
}

impl OutputSpace {
    pub const ALL: [OutputSpace; 4] = [Self::Srgb, Self::DisplayP3, Self::AdobeRgb, Self::ProPhoto];

    pub fn label(self) -> &'static str {
        match self {
            Self::Srgb => "sRGB",
            Self::DisplayP3 => "Display P3",
            Self::AdobeRgb => "Adobe RGB (1998)",
            Self::ProPhoto => "ProPhoto RGB",
        }
    }

    pub fn primaries(self) -> Primaries {
        match self {
            Self::Srgb => SRGB,
            Self::DisplayP3 => DISPLAY_P3,
            Self::AdobeRgb => ADOBE_RGB,
            Self::ProPhoto => PROPHOTO,
        }
    }

    /// The encoded profile for this space, with a readable description so
    /// viewers show its name.
    pub fn profile(self) -> Result<Profile, Error> {
        let mut profile = match self {
            Self::Srgb => Profile::new_srgb(),
            Self::DisplayP3 | Self::AdobeRgb | Self::ProPhoto => {
                let curve = match self {
                    // The sRGB parametric curve (IEC 61966-2-1).
                    Self::DisplayP3 => ToneCurve::new_parametric(
                        4,
                        &[2.4, 1.0 / 1.055, 0.055 / 1.055, 1.0 / 12.92, 0.04045],
                    )?,
                    Self::AdobeRgb => ToneCurve::new(563.0 / 256.0),
                    _ => ToneCurve::new(1.8),
                };
                let p = self.primaries();
                let prim = CIExyYTRIPLE {
                    Red: xyy(p.r),
                    Green: xyy(p.g),
                    Blue: xyy(p.b),
                };
                Profile::new_rgb(&xyy(p.white), &prim, &[&curve, &curve, &curve])?
            }
        };
        let mut mlu = MLU::new(1);
        if mlu.set_text_ascii(self.label(), Locale::none()) {
            profile.write_tag(TagSignature::ProfileDescriptionTag, lcms2::Tag::MLU(&mlu));
        }
        Ok(profile)
    }

    /// The serialized ICC profile to embed in a file.
    pub fn icc_bytes(self) -> Result<Vec<u8>, Error> {
        Ok(self.profile()?.icc()?)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::Mat3d;
    use crate::cie::{REC2020, SRGB};
    use approx::assert_abs_diff_eq;

    #[test]
    fn every_output_space_serializes_and_round_trips() {
        for s in OutputSpace::ALL {
            let bytes = s.icc_bytes().unwrap();
            assert!(bytes.len() > 128, "{s:?}");
            assert_eq!(&bytes[36..40], b"acsp");
            let back = Profile::new_icc(&bytes).unwrap();
            assert!(back.is_matrix_shaper());
        }
    }

    #[test]
    fn wide_spaces_hold_saturated_colors_srgb_clips() {
        // A saturated Rec.2020 green stays in range in ProPhoto but not sRGB.
        let n = 5;
        let idx = |r: usize, g: usize, b: usize| ((b * n + g) * n + r) * 3;
        let srgb = build_display_lut(&REC2020, &OutputSpace::Srgb.profile().unwrap(), n).unwrap();
        let pp = build_display_lut(&REC2020, &OutputSpace::ProPhoto.profile().unwrap(), n).unwrap();
        let i = idx(0, n - 1, 0);
        assert!(srgb[i] <= 0.001, "sRGB red should clip to 0: {}", srgb[i]);
        assert!(pp[i] > 0.05, "ProPhoto red should be >0: {}", pp[i]);
    }

    #[test]
    fn lut_matches_the_analytic_matrix_and_oetf_in_gamut() {
        let n = 17;
        let lut = build_display_lut(&REC2020, &srgb_profile(), n).unwrap();
        assert_eq!(lut.len(), n * n * n * 3);
        let m: Mat3d = SRGB.xyz_to_rgb().mul(&REC2020.rgb_to_xyz());
        let denom = (n - 1) as f32;
        // Neutral axis and a desaturated colour (inside sRGB).
        for (r, g, b) in [(0, 0, 0), (8, 8, 8), (16, 16, 16), (10, 8, 6)] {
            let lin = [r, g, b].map(|i| crate::srgb_eotf(i as f32 / denom) as f64);
            let expect = m
                .mul_vec(lin)
                .map(|v| crate::srgb_oetf(v.clamp(0.0, 1.0) as f32));
            let at = ((b * n + g) * n + r) * 3;
            for c in 0..3 {
                assert_abs_diff_eq!(lut[at + c], expect[c], epsilon = 4e-3);
            }
        }
    }
}
