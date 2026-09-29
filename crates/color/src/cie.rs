//! CIE xy / XYZ and RGB primaries → matrix conversions.

use crate::Mat3d;

pub type Xyz = [f64; 3];

/// A chromaticity coordinate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Xy {
    pub x: f64,
    pub y: f64,
}

impl Xy {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// XYZ with Y = 1.
    pub fn to_xyz(self) -> Xyz {
        [self.x / self.y, 1.0, (1.0 - self.x - self.y) / self.y]
    }

    pub fn from_xyz(xyz: Xyz) -> Self {
        let sum = xyz[0] + xyz[1] + xyz[2];
        Self {
            x: xyz[0] / sum,
            y: xyz[1] / sum,
        }
    }
}

pub const D65: Xy = Xy::new(0.3127, 0.3290);
pub const D50: Xy = Xy::new(0.3457, 0.3585);

/// RGB primaries and white point → the RGB→XYZ matrix.
#[derive(Debug, Clone, Copy)]
pub struct Primaries {
    pub r: Xy,
    pub g: Xy,
    pub b: Xy,
    pub white: Xy,
}

pub const SRGB: Primaries = Primaries {
    r: Xy::new(0.640, 0.330),
    g: Xy::new(0.300, 0.600),
    b: Xy::new(0.150, 0.060),
    white: D65,
};
pub const DISPLAY_P3: Primaries = Primaries {
    r: Xy::new(0.680, 0.320),
    g: Xy::new(0.265, 0.690),
    b: Xy::new(0.150, 0.060),
    white: D65,
};
pub const REC2020: Primaries = Primaries {
    r: Xy::new(0.708, 0.292),
    g: Xy::new(0.170, 0.797),
    b: Xy::new(0.131, 0.046),
    white: D65,
};

impl Primaries {
    pub fn rgb_to_xyz(&self) -> Mat3d {
        let (r, g, b) = (self.r.to_xyz(), self.g.to_xyz(), self.b.to_xyz());
        let cols = Mat3d([[r[0], g[0], b[0]], [r[1], g[1], b[1]], [r[2], g[2], b[2]]]);
        // Scale each primary so RGB(1,1,1) lands on the white point.
        let s = cols
            .inverse()
            .map(|inv| inv.mul_vec(self.white.to_xyz()))
            .unwrap_or([1.0; 3]);
        Mat3d([
            [
                cols.0[0][0] * s[0],
                cols.0[0][1] * s[1],
                cols.0[0][2] * s[2],
            ],
            [
                cols.0[1][0] * s[0],
                cols.0[1][1] * s[1],
                cols.0[1][2] * s[2],
            ],
            [
                cols.0[2][0] * s[0],
                cols.0[2][1] * s[1],
                cols.0[2][2] * s[2],
            ],
        ])
    }

    pub fn xyz_to_rgb(&self) -> Mat3d {
        self.rgb_to_xyz().inverse().unwrap_or(Mat3d::IDENTITY)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;

    #[test]
    fn srgb_matrix_matches_the_published_values() {
        let m = SRGB.rgb_to_xyz();
        // IEC 61966-2-1 (to 4 dp; the spec rounds its white point).
        assert_abs_diff_eq!(m.0[0][0], 0.4124, epsilon = 2e-4);
        assert_abs_diff_eq!(m.0[1][1], 0.7152, epsilon = 2e-4);
        assert_abs_diff_eq!(m.0[2][2], 0.9505, epsilon = 2e-4);
    }

    #[test]
    fn white_maps_to_the_white_point_for_every_space() {
        for p in [SRGB, DISPLAY_P3, REC2020] {
            let xyz = p.rgb_to_xyz().mul_vec([1.0; 3]);
            let w = p.white.to_xyz();
            for i in 0..3 {
                assert_abs_diff_eq!(xyz[i], w[i], epsilon = 1e-9);
            }
        }
    }

    #[test]
    fn xy_round_trips_through_xyz() {
        let xy = Xy::from_xyz(D50.to_xyz());
        assert_abs_diff_eq!(xy.x, D50.x, epsilon = 1e-12);
        assert_abs_diff_eq!(xy.y, D50.y, epsilon = 1e-12);
    }
}
