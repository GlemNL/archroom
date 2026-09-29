//! Display orientation as a dihedral-group element (EXIF 1–8 plus the
//! Library's user quarter turns), expressed as the linear part of the
//! oriented→source coordinate map the geometry kernel applies (plan §6.8).

/// `source_uv - 0.5 = m · (oriented_uv - 0.5)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Orientation(pub [[i32; 2]; 2]);

impl Default for Orientation {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Orientation {
    pub const IDENTITY: Orientation = Orientation([[1, 0], [0, 1]]);

    /// EXIF orientation tag (1–8); anything else is treated as 1.
    pub fn from_exif(tag: i32) -> Self {
        Orientation(match tag {
            2 => [[-1, 0], [0, 1]],
            3 => [[-1, 0], [0, -1]],
            4 => [[1, 0], [0, -1]],
            5 => [[0, 1], [1, 0]],
            6 => [[0, 1], [-1, 0]],
            7 => [[0, -1], [-1, 0]],
            8 => [[0, -1], [1, 0]],
            _ => [[1, 0], [0, 1]],
        })
    }

    /// Additionally rotates the displayed image by `quarters` clockwise
    /// turns (negative = counter-clockwise), as `photos.user_orientation`.
    pub fn rotated(self, quarters: i32) -> Self {
        const CW: [[i32; 2]; 2] = [[0, 1], [-1, 0]];
        let mut m = self.0;
        for _ in 0..quarters.rem_euclid(4) {
            m = mul(m, CW);
        }
        Orientation(m)
    }

    /// Additionally mirrors the displayed image left/right.
    pub fn flipped_h(self) -> Self {
        let m = self.0;
        Orientation([[-m[0][0], m[0][1]], [-m[1][0], m[1][1]]])
    }

    /// True when width and height trade places.
    pub fn swaps_axes(self) -> bool {
        self.0[0][0] == 0
    }

    /// Oriented (displayed) size of a `w`×`h` source.
    pub fn oriented_size(self, w: u32, h: u32) -> (u32, u32) {
        if self.swaps_axes() { (h, w) } else { (w, h) }
    }

    pub fn as_f32(self) -> [f32; 4] {
        let m = self.0;
        [
            m[0][0] as f32,
            m[0][1] as f32,
            m[1][0] as f32,
            m[1][1] as f32,
        ]
    }

    /// Maps oriented normalized coords to source normalized coords.
    pub fn to_source(self, uv: [f32; 2]) -> [f32; 2] {
        let m = self.as_f32();
        let (x, y) = (uv[0] - 0.5, uv[1] - 0.5);
        [m[0] * x + m[1] * y + 0.5, m[2] * x + m[3] * y + 0.5]
    }
}

fn mul(a: [[i32; 2]; 2], b: [[i32; 2]; 2]) -> [[i32; 2]; 2] {
    let mut o = [[0; 2]; 2];
    for (r, row) in o.iter_mut().enumerate() {
        for (c, cell) in row.iter_mut().enumerate() {
            *cell = a[r][0] * b[0][c] + a[r][1] * b[1][c];
        }
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exif_6_puts_the_source_top_left_at_the_display_top_right() {
        // EXIF 6 = rotate 90° CW to display.
        let o = Orientation::from_exif(6);
        assert_eq!(o.to_source([1.0, 0.0]), [0.0, 0.0]);
        assert_eq!(o.oriented_size(6000, 4000), (4000, 6000));
    }

    #[test]
    fn exif_8_puts_the_source_top_left_at_the_display_bottom_left() {
        assert_eq!(Orientation::from_exif(8).to_source([0.0, 1.0]), [0.0, 0.0]);
    }

    #[test]
    fn flipping_matches_the_mirrored_exif_tags() {
        assert_eq!(Orientation::IDENTITY.flipped_h(), Orientation::from_exif(2));
        for tag in 1..=8 {
            let o = Orientation::from_exif(tag);
            assert_eq!(o.flipped_h().flipped_h(), o);
        }
    }

    #[test]
    fn four_quarter_turns_are_the_identity_and_one_turn_matches_exif_6() {
        for tag in 1..=8 {
            let o = Orientation::from_exif(tag);
            assert_eq!(o.rotated(4), o);
            assert_eq!(o.rotated(1).rotated(-1), o);
        }
        assert_eq!(Orientation::IDENTITY.rotated(1), Orientation::from_exif(6));
        assert_eq!(Orientation::IDENTITY.rotated(-1), Orientation::from_exif(8));
        assert_eq!(Orientation::IDENTITY.rotated(2), Orientation::from_exif(3));
    }
}
