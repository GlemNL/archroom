//! Chromatic adaptation transforms (plan §6.4: CAT16 default, Bradford
//! kept for comparisons).

use crate::Mat3d;
use crate::cie::Xyz;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cat {
    Cat16,
    Bradford,
}

impl Cat {
    /// XYZ → cone-like response matrix.
    pub fn matrix(self) -> Mat3d {
        match self {
            Cat::Cat16 => Mat3d([
                [0.401288, 0.650173, -0.051461],
                [-0.250268, 1.204414, 0.045854],
                [-0.002079, 0.048952, 0.953127],
            ]),
            Cat::Bradford => Mat3d([
                [0.8951, 0.2664, -0.1614],
                [-0.7502, 1.7135, 0.0367],
                [0.0389, -0.0685, 1.0296],
            ]),
        }
    }

    /// The XYZ→XYZ matrix that maps colors seen under `src` white to the
    /// corresponding colors under `dst` white (full adaptation, D = 1).
    pub fn adapt(self, src_white: Xyz, dst_white: Xyz) -> Mat3d {
        let m = self.matrix();
        let Some(inv) = m.inverse() else {
            return Mat3d::IDENTITY;
        };
        let s = m.mul_vec(src_white);
        let d = m.mul_vec(dst_white);
        let scale = Mat3d::diag([d[0] / s[0], d[1] / s[1], d[2] / s[2]]);
        inv.mul(&scale).mul(&m)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::cie::{D50, D65};
    use approx::assert_abs_diff_eq;

    #[test]
    fn source_white_adapts_exactly_to_destination_white() {
        for cat in [Cat::Cat16, Cat::Bradford] {
            let out = cat.adapt(D50.to_xyz(), D65.to_xyz()).mul_vec(D50.to_xyz());
            for (o, w) in out.iter().zip(D65.to_xyz()) {
                assert_abs_diff_eq!(*o, w, epsilon = 1e-9);
            }
        }
    }

    #[test]
    fn adapting_to_the_same_white_is_identity() {
        let m = Cat::Cat16.adapt(D65.to_xyz(), D65.to_xyz());
        for r in 0..3 {
            for c in 0..3 {
                assert_abs_diff_eq!(m.0[r][c], Mat3d::IDENTITY.0[r][c], epsilon = 1e-9);
            }
        }
    }

    #[test]
    fn bradford_d65_to_d50_matches_the_published_matrix() {
        // Bruce Lindbloom's D65→D50 Bradford matrix (his D50 = 0.96422, 1, 0.82521).
        let m = Cat::Bradford.adapt([0.95047, 1.0, 1.08883], [0.96422, 1.0, 0.82521]);
        assert_abs_diff_eq!(m.0[0][0], 1.0478112, epsilon = 1e-5);
        assert_abs_diff_eq!(m.0[1][1], 0.9905, epsilon = 2e-3);
        assert_abs_diff_eq!(m.0[2][2], 0.7521, epsilon = 2e-3);
    }
}
