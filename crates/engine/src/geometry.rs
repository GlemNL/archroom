//! Geometry resolution (plan §6.8): turns the Straighten and Crop params
//! into the concrete transform the geometry pass applies, and holds the
//! rectangle maths the crop tool needs ("constrain to image").
//!
//! Coordinates: the *canvas* is the oriented source (EXIF, Library turns,
//! Develop quarter turns and flip) at source resolution. The straightened
//! image is that canvas rotated by `angle` about its centre; the crop
//! rectangle is normalised to the canvas. All rotations use y-down screen
//! coordinates, so a positive angle turns the image clockwise.

use crate::Orientation;
use crate::ops::{Crop, CropParams, Straighten};
use crate::params::EditParams;

/// Smallest crop side, as a fraction of the canvas.
pub const MIN_CROP: f64 = 0.02;

#[derive(Debug, Clone, PartialEq)]
pub struct Geometry {
    /// Base orientation composed with the Develop quarter turns and flip.
    pub orientation: Orientation,
    /// The oriented source size in pixels.
    pub canvas: (u32, u32),
    pub angle_deg: f64,
    /// Normalised `[x0, y0, x1, y1]` in the canvas.
    pub crop: [f64; 4],
}

impl Geometry {
    /// Size of the crop at source resolution, in pixels.
    pub fn crop_px(&self) -> (f64, f64) {
        (
            (self.crop[2] - self.crop[0]) * f64::from(self.canvas.0),
            (self.crop[3] - self.crop[1]) * f64::from(self.canvas.1),
        )
    }
}

impl Geometry {
    /// Maps a position in the rendered output (`u`, `v` in 0..1) to the
    /// source image (normalised), the CPU twin of the geometry kernel.
    /// `None` when it falls outside the rotated image.
    pub fn output_to_source(&self, u: f64, v: f64) -> Option<[f64; 2]> {
        let (w, h) = (f64::from(self.canvas.0), f64::from(self.canvas.1));
        let cx = self.crop[0] + u * (self.crop[2] - self.crop[0]);
        let cy = self.crop[1] + v * (self.crop[3] - self.crop[1]);
        let (qx, qy) = rotate(cx * w - w / 2.0, cy * h - h / 2.0, -self.angle_deg);
        let (ox, oy) = ((qx + w / 2.0) / w, (qy + h / 2.0) / h);
        if !(0.0..=1.0).contains(&ox) || !(0.0..=1.0).contains(&oy) {
            return None;
        }
        let m = self.orientation.0;
        let (dx, dy) = (ox - 0.5, oy - 0.5);
        Some([
            f64::from(m[0][0]) * dx + f64::from(m[0][1]) * dy + 0.5,
            f64::from(m[1][0]) * dx + f64::from(m[1][1]) * dy + 0.5,
        ])
    }
}

/// The Develop-side transform of `crop` applied on top of `base`.
pub fn compose_orientation(base: Orientation, crop: &CropParams) -> Orientation {
    let mut o = base.rotated(crop.quarters);
    if crop.flip_h {
        o = o.flipped_h();
    }
    o
}

/// Resolves `params` for a `source`-sized image. `ignore_crop` renders the
/// whole canvas (the crop tool is open, plan §6.8).
pub fn resolve(
    source: (u32, u32),
    base: Orientation,
    params: &EditParams,
    ignore_crop: bool,
) -> Geometry {
    let crop_p = params.get::<Crop>();
    let orientation = compose_orientation(base, &crop_p);
    let canvas = orientation.oriented_size(source.0, source.1);
    let angle_deg = params.get::<Straighten>().angle.clamp(-45.0, 45.0);
    let crop = if ignore_crop {
        [0.0, 0.0, 1.0, 1.0]
    } else {
        sanitize(crop_p.rect())
    };
    Geometry {
        orientation,
        canvas,
        angle_deg,
        crop,
    }
}

/// Clamps a rectangle into the canvas and keeps at least `MIN_CROP` per side.
pub fn sanitize(r: [f64; 4]) -> [f64; 4] {
    let mut x0 = r[0].clamp(0.0, 1.0 - MIN_CROP);
    let mut y0 = r[1].clamp(0.0, 1.0 - MIN_CROP);
    let mut x1 = r[2].clamp(x0 + MIN_CROP, 1.0);
    let mut y1 = r[3].clamp(y0 + MIN_CROP, 1.0);
    if !(x0.is_finite() && y0.is_finite() && x1.is_finite() && y1.is_finite()) {
        (x0, y0, x1, y1) = (0.0, 0.0, 1.0, 1.0);
    }
    [x0, y0, x1, y1]
}

/// Rotates `(x, y)` (offsets from the canvas centre, pixels) clockwise by
/// `deg`.
fn rotate(x: f64, y: f64, deg: f64) -> (f64, f64) {
    let (s, c) = deg.to_radians().sin_cos();
    (x * c - y * s, x * s + y * c)
}

/// True when every corner of `r` lies inside the image rotated by
/// `angle_deg` in a `canvas`-sized frame.
pub fn rect_inside(angle_deg: f64, canvas: (u32, u32), r: [f64; 4]) -> bool {
    let (w, h) = (f64::from(canvas.0), f64::from(canvas.1));
    let eps = 1e-6 * w.max(h);
    [(r[0], r[1]), (r[2], r[1]), (r[2], r[3]), (r[0], r[3])]
        .into_iter()
        .all(|(x, y)| {
            let (qx, qy) = rotate(x * w - w / 2.0, y * h - h / 2.0, -angle_deg);
            qx.abs() <= w / 2.0 + eps && qy.abs() <= h / 2.0 + eps
        })
}

/// The last rectangle on the way from `prev` (valid) to `cand` that still
/// lies inside the image: bisection on the interpolation parameter.
pub fn constrain_between(
    angle_deg: f64,
    canvas: (u32, u32),
    prev: [f64; 4],
    cand: [f64; 4],
) -> [f64; 4] {
    if rect_inside(angle_deg, canvas, cand) {
        return cand;
    }
    let lerp = |t: f64| std::array::from_fn(|i| prev[i] + (cand[i] - prev[i]) * t);
    let (mut lo, mut hi) = (0.0, 1.0);
    for _ in 0..30 {
        let mid = (lo + hi) / 2.0;
        if rect_inside(angle_deg, canvas, lerp(mid)) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lerp(lo)
}

/// Shrinks `r` about its centre until it fits inside the rotated image
/// (used when the angle changes under an existing crop). A centre outside
/// the image first moves to the canvas centre.
pub fn fit_rect(angle_deg: f64, canvas: (u32, u32), r: [f64; 4]) -> [f64; 4] {
    if rect_inside(angle_deg, canvas, r) {
        return r;
    }
    let (mut cx, mut cy) = ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0);
    let (hw, hh) = ((r[2] - r[0]) / 2.0, (r[3] - r[1]) / 2.0);
    if !rect_inside(angle_deg, canvas, [cx, cy, cx, cy]) {
        (cx, cy) = (0.5, 0.5);
    }
    let make = |s: f64| [cx - hw * s, cy - hh * s, cx + hw * s, cy + hh * s];
    let (mut lo, mut hi) = (0.0, 1.0);
    for _ in 0..30 {
        let mid = (lo + hi) / 2.0;
        if rect_inside(angle_deg, canvas, make(mid)) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    make(lo)
}

/// A crop rectangle after the canvas is turned a quarter clockwise.
pub fn rotate_rect_cw(r: [f64; 4]) -> [f64; 4] {
    [1.0 - r[3], r[0], 1.0 - r[1], r[2]]
}

/// A crop rectangle after the canvas is mirrored left/right.
pub fn mirror_rect(r: [f64; 4]) -> [f64; 4] {
    [1.0 - r[2], r[1], 1.0 - r[0], r[3]]
}

/// The straighten angle that makes a line from `a` to `b` (canvas pixels,
/// as drawn on the current straightened view) level, given the current
/// angle. Lines closer to vertical level to the vertical.
pub fn level_angle(current_deg: f64, a: (f64, f64), b: (f64, f64)) -> f64 {
    let mut slope = (b.1 - a.1).atan2(b.0 - a.0).to_degrees();
    // Fold to (−90°, 90°], then to the nearest of horizontal/vertical.
    if slope > 90.0 {
        slope -= 180.0;
    } else if slope <= -90.0 {
        slope += 180.0;
    }
    if slope > 45.0 {
        slope -= 90.0;
    } else if slope < -45.0 {
        slope += 90.0;
    }
    (current_deg - slope).clamp(-45.0, 45.0)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::ops::{CropParams, StraightenParams};

    fn params(angle: f64, crop: CropParams) -> EditParams {
        let mut p = EditParams::default();
        p.set::<Straighten>(StraightenParams { angle });
        p.set::<Crop>(crop);
        p
    }

    #[test]
    fn defaults_resolve_to_the_oriented_full_frame() {
        let g = resolve(
            (600, 400),
            Orientation::from_exif(6),
            &EditParams::default(),
            false,
        );
        assert_eq!(g.canvas, (400, 600));
        assert_eq!(g.crop, [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(g.angle_deg, 0.0);
    }

    #[test]
    fn quarter_turns_and_flips_compose_onto_the_base_orientation() {
        let crop = CropParams {
            quarters: 1,
            ..Default::default()
        };
        let g = resolve((600, 400), Orientation::IDENTITY, &params(0.0, crop), false);
        assert_eq!(g.orientation, Orientation::from_exif(6));
        assert_eq!(g.canvas, (400, 600));
        let flip = CropParams {
            flip_h: true,
            ..Default::default()
        };
        let g = resolve((600, 400), Orientation::IDENTITY, &params(0.0, flip), false);
        assert_eq!(g.orientation, Orientation::from_exif(2));
    }

    #[test]
    fn output_positions_map_back_through_crop_orientation_and_angle() {
        // Identity: the output centre is the source centre.
        let g = resolve(
            (600, 400),
            Orientation::IDENTITY,
            &EditParams::default(),
            false,
        );
        let c = g.output_to_source(0.5, 0.5).unwrap();
        assert!((c[0] - 0.5).abs() < 1e-9 && (c[1] - 0.5).abs() < 1e-9);

        // EXIF 6: the display's top-right is the source's top-left.
        let g = resolve(
            (600, 400),
            Orientation::from_exif(6),
            &EditParams::default(),
            false,
        );
        let c = g.output_to_source(1.0, 0.0).unwrap();
        assert!(c[0].abs() < 1e-9 && c[1].abs() < 1e-9, "{c:?}");

        // A crop of the right half: its left edge is the source's centre.
        let crop = CropParams {
            x0: 0.5,
            ..Default::default()
        };
        let g = resolve((600, 400), Orientation::IDENTITY, &params(0.0, crop), false);
        assert!((g.output_to_source(0.0, 0.5).unwrap()[0] - 0.5).abs() < 1e-9);

        // Rotated full frame: the corner falls outside the image.
        let g = resolve(
            (600, 400),
            Orientation::IDENTITY,
            &params(15.0, CropParams::default()),
            false,
        );
        assert!(g.output_to_source(0.0, 0.0).is_none());
        assert!(g.output_to_source(0.5, 0.5).is_some());
    }

    #[test]
    fn ignore_crop_shows_the_whole_canvas() {
        let crop = CropParams {
            x0: 0.2,
            y0: 0.1,
            x1: 0.8,
            y1: 0.9,
            ..Default::default()
        };
        let p = params(3.0, crop);
        assert_eq!(
            resolve((100, 100), Orientation::IDENTITY, &p, true).crop,
            [0.0, 0.0, 1.0, 1.0]
        );
        assert_eq!(
            resolve((100, 100), Orientation::IDENTITY, &p, false).crop,
            [0.2, 0.1, 0.8, 0.9]
        );
    }

    #[test]
    fn a_rotated_image_no_longer_covers_the_canvas_corners() {
        assert!(rect_inside(0.0, (300, 200), [0.0, 0.0, 1.0, 1.0]));
        assert!(!rect_inside(5.0, (300, 200), [0.0, 0.0, 1.0, 1.0]));
        assert!(rect_inside(5.0, (300, 200), [0.3, 0.3, 0.7, 0.7]));
    }

    #[test]
    fn fit_rect_shrinks_the_full_frame_to_the_inscribed_rectangle() {
        let r = fit_rect(10.0, (300, 200), [0.0, 0.0, 1.0, 1.0]);
        assert!(rect_inside(10.0, (300, 200), r));
        assert!(r[2] - r[0] < 0.9 && r[2] - r[0] > 0.5, "{r:?}");
        // A little more and it would poke out.
        let bigger = [r[0] - 0.01, r[1] - 0.01, r[2] + 0.01, r[3] + 0.01];
        assert!(!rect_inside(10.0, (300, 200), bigger));
    }

    #[test]
    fn constrain_between_stops_at_the_edge_of_the_image() {
        let prev = [0.3, 0.3, 0.7, 0.7];
        let cand = [0.0, 0.0, 1.0, 1.0];
        let r = constrain_between(8.0, (300, 200), prev, cand);
        assert!(rect_inside(8.0, (300, 200), r));
        assert!(
            r[0] < prev[0] && r[2] > prev[2],
            "it grew as far as it could"
        );
        assert_eq!(
            constrain_between(0.0, (300, 200), prev, cand),
            cand,
            "unrotated, the whole frame is fine"
        );
    }

    #[test]
    fn rect_helpers_are_their_own_inverses() {
        let r = [0.1, 0.2, 0.6, 0.9];
        let mut x = r;
        for _ in 0..4 {
            x = rotate_rect_cw(x);
        }
        for (a, b) in x.iter().zip(&r) {
            assert!((a - b).abs() < 1e-12);
        }
        for (a, b) in mirror_rect(mirror_rect(r)).iter().zip(&r) {
            assert!((a - b).abs() < 1e-12);
        }
    }

    #[test]
    fn a_tilted_horizon_levels_to_the_matching_angle() {
        // A line sloping 3° clockwise (down to the right) needs a 3° CCW turn.
        let a = (0.0, 0.0);
        let b = (1000.0, 1000.0 * 3f64.to_radians().tan());
        assert!((level_angle(0.0, a, b) + 3.0).abs() < 1e-9);
        // Nearly vertical lines level to the vertical.
        let v = (0.0, 1000.0);
        let angle = level_angle(0.0, a, (v.1 * 2f64.to_radians().tan(), v.1));
        assert!(angle.abs() < 3.0);
    }
}
