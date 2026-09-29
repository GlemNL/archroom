//! `f64` 3×3 matrices for the color science.

use crate::Mat3;

/// Row-major 3×3 matrix over `f64`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mat3d(pub [[f64; 3]; 3]);

impl Mat3d {
    pub const IDENTITY: Mat3d = Mat3d([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);

    pub fn diag(d: [f64; 3]) -> Self {
        Mat3d([[d[0], 0.0, 0.0], [0.0, d[1], 0.0], [0.0, 0.0, d[2]]])
    }

    pub fn mul_vec(&self, v: [f64; 3]) -> [f64; 3] {
        let m = &self.0;
        [
            m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
            m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
            m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
        ]
    }

    pub fn mul(&self, o: &Mat3d) -> Mat3d {
        let (a, b) = (&self.0, &o.0);
        let mut out = [[0.0; 3]; 3];
        for (r, row) in out.iter_mut().enumerate() {
            for (c, cell) in row.iter_mut().enumerate() {
                *cell = a[r][0] * b[0][c] + a[r][1] * b[1][c] + a[r][2] * b[2][c];
            }
        }
        Mat3d(out)
    }

    pub fn transpose(&self) -> Mat3d {
        let m = &self.0;
        Mat3d([
            [m[0][0], m[1][0], m[2][0]],
            [m[0][1], m[1][1], m[2][1]],
            [m[0][2], m[1][2], m[2][2]],
        ])
    }

    pub fn det(&self) -> f64 {
        let m = &self.0;
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    }

    /// `None` when the matrix is (near-)singular.
    pub fn inverse(&self) -> Option<Mat3d> {
        let det = self.det();
        if det.abs() < 1e-12 {
            return None;
        }
        let m = &self.0;
        let inv = 1.0 / det;
        Some(Mat3d([
            [
                (m[1][1] * m[2][2] - m[1][2] * m[2][1]) * inv,
                (m[0][2] * m[2][1] - m[0][1] * m[2][2]) * inv,
                (m[0][1] * m[1][2] - m[0][2] * m[1][1]) * inv,
            ],
            [
                (m[1][2] * m[2][0] - m[1][0] * m[2][2]) * inv,
                (m[0][0] * m[2][2] - m[0][2] * m[2][0]) * inv,
                (m[0][2] * m[1][0] - m[0][0] * m[1][2]) * inv,
            ],
            [
                (m[1][0] * m[2][1] - m[1][1] * m[2][0]) * inv,
                (m[0][1] * m[2][0] - m[0][0] * m[2][1]) * inv,
                (m[0][0] * m[1][1] - m[0][1] * m[1][0]) * inv,
            ],
        ]))
    }

    pub fn to_f32(&self) -> Mat3 {
        let m = &self.0;
        let f = |r: usize| [m[r][0] as f32, m[r][1] as f32, m[r][2] as f32];
        Mat3([f(0), f(1), f(2)])
    }
}

impl From<Mat3> for Mat3d {
    fn from(m: Mat3) -> Self {
        let f = |r: usize| [m.0[r][0] as f64, m.0[r][1] as f64, m.0[r][2] as f64];
        Mat3d([f(0), f(1), f(2)])
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;

    #[test]
    fn inverse_times_matrix_is_identity() {
        let m = Mat3d([[0.4, 0.2, 0.1], [0.05, 0.9, 0.2], [0.0, 0.3, 0.8]]);
        let p = m.mul(&m.inverse().unwrap());
        for r in 0..3 {
            for c in 0..3 {
                assert_abs_diff_eq!(p.0[r][c], Mat3d::IDENTITY.0[r][c], epsilon = 1e-12);
            }
        }
    }

    #[test]
    fn singular_matrix_has_no_inverse() {
        assert!(
            Mat3d([[1.0, 2.0, 3.0], [2.0, 4.0, 6.0], [0.0, 0.0, 1.0]])
                .inverse()
                .is_none()
        );
    }
}
