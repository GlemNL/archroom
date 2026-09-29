//! Raw color glue (plan §6.3/§6.4): from a decoded raw's `CameraColor` to
//! the matrix the scene stage applies, and the as-shot Temp/Tint the UI
//! shows.
//!
//! Decoded pixels are `raw * d65_mul` (demosaiced once at the D65
//! reference white). To colour-manage them we undo `d65_mul`, invert the
//! DNG ColorMatrix to reach XYZ under the scene illuminant, adapt that
//! illuminant to D65 with a CAT, then move to linear Rec.2020. Changing
//! white balance only changes the illuminant → one 3×3, no re-demosaic.

use archroom_color::Mat3d;
use archroom_color::cat::Cat;
use archroom_color::cie::{D65, REC2020, Xy};
use archroom_color::temp::{temp_tint_to_xy, xy_to_temp_tint};
use archroom_io::CameraColor;

/// The working space every scene/tone op runs in.
pub const WORKING_SPACE: archroom_color::cie::Primaries = REC2020;

#[derive(Debug, Clone)]
pub struct SceneColor {
    camera_to_xyz: Mat3d,
    d65_mul: [f64; 3],
    as_shot_mul: Option<[f64; 3]>,
}

impl SceneColor {
    /// `None` if the camera matrix is singular (an all-zero LibRaw matrix
    /// is reported as identity, which is fine: it just isn't colour-managed).
    pub fn new(camera: &CameraColor) -> Option<Self> {
        let cam: Mat3d = camera.xyz_to_camera.into();
        Some(Self {
            camera_to_xyz: cam.inverse()?,
            d65_mul: camera.d65_mul.map(f64::from),
            as_shot_mul: camera.as_shot_mul.map(|m| m.map(f64::from)),
        })
    }

    /// Chromaticity of the recorded illuminant; D65 when the file has no
    /// as-shot white balance.
    pub fn as_shot_white(&self) -> Xy {
        let Some(mul) = self.as_shot_mul else {
            return D65;
        };
        // Raw response of a neutral surface under the as-shot light is 1/mul.
        let raw = mul.map(|m| 1.0 / m);
        Xy::from_xyz(self.camera_to_xyz.mul_vec(raw))
    }

    /// The illuminant chromaticity for which the given *mean decoded colour*
    /// is neutral — the gray-world estimate behind Auto WB (plan §6.4).
    pub fn white_from_decoded(&self, decoded: [f64; 3]) -> Xy {
        let raw = [
            decoded[0] / self.d65_mul[0],
            decoded[1] / self.d65_mul[1],
            decoded[2] / self.d65_mul[2],
        ];
        Xy::from_xyz(self.camera_to_xyz.mul_vec(raw))
    }

    /// "As Shot 5230 K / +7" (plan §6.3).
    pub fn as_shot_temp_tint(&self) -> (f64, f64) {
        xy_to_temp_tint(self.as_shot_white())
    }

    /// Decoded camera RGB → linear Rec.2020 with `illuminant` adapted to D65.
    pub fn decoded_to_working(&self, illuminant: Xy, cat: Cat) -> Mat3d {
        let undo = Mat3d::diag(self.d65_mul.map(|m| 1.0 / m));
        let adapt = cat.adapt(illuminant.to_xyz(), D65.to_xyz());
        WORKING_SPACE
            .xyz_to_rgb()
            .mul(&adapt)
            .mul(&self.camera_to_xyz)
            .mul(&undo)
    }

    /// The matrix for a chosen Temp/Tint (custom white balance).
    pub fn decoded_to_working_for(&self, temp: f64, tint: f64, cat: Cat) -> Mat3d {
        self.decoded_to_working(temp_tint_to_xy(temp, tint), cat)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;
    use archroom_color::Mat3;

    /// The Nikon D780 sample from `/home/clem/Downloads/_7808140.NEF`.
    fn d780() -> CameraColor {
        CameraColor {
            xyz_to_camera: Mat3([
                [0.9943, -0.3269, -0.0839],
                [-0.5323, 1.3269, 0.2259],
                [-0.1198, 0.2083, 0.7557],
            ]),
            d65_mul: [1.8982556, 0.9372672, 1.0902004],
            as_shot_mul: Some([1.4921875, 1.0, 1.4960938]),
        }
    }

    #[test]
    fn the_as_shot_white_decodes_to_neutral_working_rgb() {
        let c = d780();
        let sc = SceneColor::new(&c).unwrap();
        let mul = c.as_shot_mul.unwrap();
        // A neutral surface under the as-shot light, as the decoder sees it.
        let decoded: [f64; 3] = std::array::from_fn(|i| c.d65_mul[i] as f64 / mul[i] as f64);
        let m = sc.decoded_to_working(sc.as_shot_white(), Cat::Cat16);
        let out = m.mul_vec(decoded);
        assert_abs_diff_eq!(out[0], out[1], epsilon = 1e-6);
        assert_abs_diff_eq!(out[2], out[1], epsilon = 1e-6);
    }

    #[test]
    fn the_d65_reference_white_is_neutral_under_a_d65_illuminant() {
        let sc = SceneColor::new(&d780()).unwrap();
        let out = sc
            .decoded_to_working(D65, Cat::Cat16)
            .mul_vec([1.0, 1.0, 1.0]);
        assert_abs_diff_eq!(out[0], out[1], epsilon = 2e-3);
        assert_abs_diff_eq!(out[2], out[1], epsilon = 2e-3);
    }

    #[test]
    fn as_shot_temp_tint_is_plausible_and_re_derivable() {
        let sc = SceneColor::new(&d780()).unwrap();
        let (t, tint) = sc.as_shot_temp_tint();
        assert!((2000.0..=12000.0).contains(&t), "T = {t}");
        assert!(tint.abs() < 60.0, "tint = {tint}");
        // Feeding Temp/Tint back must give the same illuminant.
        let xy = temp_tint_to_xy(t, tint);
        let w = sc.as_shot_white();
        assert_abs_diff_eq!(xy.x, w.x, epsilon = 2e-3);
        assert_abs_diff_eq!(xy.y, w.y, epsilon = 2e-3);
        eprintln!("as shot: {t:.0} K / {tint:+.1}");
    }

    #[test]
    fn no_as_shot_wb_falls_back_to_d65() {
        let mut c = d780();
        c.as_shot_mul = None;
        let sc = SceneColor::new(&c).unwrap();
        assert_eq!(sc.as_shot_white(), D65);
    }
}
