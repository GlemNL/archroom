//! OkLab / OkLCh (Ottosson), defined on D65 XYZ.

use crate::Mat3d;
use crate::cie::Xyz;

const XYZ_TO_LMS: Mat3d = Mat3d([
    [0.8189330101, 0.3618667424, -0.1288597137],
    [0.0329845436, 0.9293118715, 0.0361456387],
    [0.0482003018, 0.2643662691, 0.6338517070],
]);
const LMS_TO_LAB: Mat3d = Mat3d([
    [0.2104542553, 0.7936177850, -0.0040720468],
    [1.9779984951, -2.4285922050, 0.4505937099],
    [0.0259040371, 0.7827717662, -0.8086757660],
]);

/// Linear RGB (for `p`) → OkLab's LMS, folding the RGB→XYZ step in so a
/// shader needs one 3×3 before the cube root.
pub fn rgb_to_lms(p: &crate::cie::Primaries) -> Mat3d {
    XYZ_TO_LMS.mul(&p.rgb_to_xyz())
}

pub fn xyz_to_oklab(xyz: Xyz) -> [f64; 3] {
    let lms = XYZ_TO_LMS.mul_vec(xyz).map(f64::cbrt);
    LMS_TO_LAB.mul_vec(lms)
}

pub fn oklab_to_xyz(lab: [f64; 3]) -> Xyz {
    let lms = LMS_TO_LAB
        .inverse()
        .unwrap_or(Mat3d::IDENTITY)
        .mul_vec(lab)
        .map(|v| v * v * v);
    XYZ_TO_LMS.inverse().unwrap_or(Mat3d::IDENTITY).mul_vec(lms)
}

/// (L, C, h in radians).
pub fn oklab_to_oklch(lab: [f64; 3]) -> [f64; 3] {
    [lab[0], lab[1].hypot(lab[2]), lab[2].atan2(lab[1])]
}

pub fn oklch_to_oklab(lch: [f64; 3]) -> [f64; 3] {
    [lch[0], lch[1] * lch[2].cos(), lch[1] * lch[2].sin()]
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::cie::D65;
    use approx::assert_abs_diff_eq;

    #[test]
    fn d65_white_is_l1_and_achromatic() {
        let lab = xyz_to_oklab(D65.to_xyz());
        assert_abs_diff_eq!(lab[0], 1.0, epsilon = 1e-3);
        assert_abs_diff_eq!(lab[1], 0.0, epsilon = 1e-3);
        assert_abs_diff_eq!(lab[2], 0.0, epsilon = 1e-3);
    }

    #[test]
    fn round_trips() {
        let xyz = [0.3, 0.4, 0.2];
        let back = oklab_to_xyz(oklch_to_oklab(oklab_to_oklch(xyz_to_oklab(xyz))));
        for i in 0..3 {
            assert_abs_diff_eq!(back[i], xyz[i], epsilon = 1e-9);
        }
    }
}
