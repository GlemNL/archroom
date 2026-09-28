//! `archroom-color`: color math (matrices, CAT16, Temp/Tint, transfer
//! functions, the `lcms2` wrapper). The real color science is M3 work (plan
//! roadmap); M0 only needs the 3x3 matrix type other crates build on.

/// A row-major 3x3 matrix over `f32`, used for camera-to-working-space and
/// chromatic-adaptation transforms.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mat3(pub [[f32; 3]; 3]);

impl Mat3 {
    pub const IDENTITY: Mat3 = Mat3([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);

    pub fn mul_vec3(&self, v: [f32; 3]) -> [f32; 3] {
        let m = &self.0;
        [
            m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
            m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
            m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
        ]
    }

    pub fn mul_mat3(&self, other: &Mat3) -> Mat3 {
        let a = &self.0;
        let b = &other.0;
        let mut out = [[0.0f32; 3]; 3];
        for (r, out_row) in out.iter_mut().enumerate() {
            for (c, out_cell) in out_row.iter_mut().enumerate() {
                *out_cell = a[r][0] * b[0][c] + a[r][1] * b[1][c] + a[r][2] * b[2][c];
            }
        }
        Mat3(out)
    }
}

/// The sRGB opto-electronic transfer function (linear → display-encoded),
/// used only by the M0 spike shader for a quick preview; the real output
/// stage (§6.9) builds its transform from `lcms2` in M3.
pub fn srgb_oetf(linear: f32) -> f32 {
    if linear <= 0.003_130_8 {
        linear * 12.92
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    }
}

pub fn srgb_eotf(encoded: f32) -> f32 {
    if encoded <= 0.040_45 {
        encoded / 12.92
    } else {
        ((encoded + 0.055) / 1.055).powf(2.4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_matrix_is_a_no_op() {
        let v = [1.0, 2.0, 3.0];
        assert_eq!(Mat3::IDENTITY.mul_vec3(v), v);
    }

    #[test]
    fn srgb_round_trips() {
        for i in 0..=20 {
            let x = i as f32 / 20.0;
            let round_tripped = srgb_eotf(srgb_oetf(x));
            assert!(
                (round_tripped - x).abs() < 1e-4,
                "x={x} got {round_tripped}"
            );
        }
    }
}
