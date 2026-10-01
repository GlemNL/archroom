//! CPU rasterization of brush zones (plan v0.2.0 §3.2): the strokes become
//! a source-aligned `R8` mask the local pass samples through the geometry
//! transform. Painting streams points into the last stroke, so the raster
//! updates incrementally (only the new dabs) and stays bit-identical to a
//! full rebuild.

use super::Stroke;

/// Longest edge of a brush mask in pixels.
pub const MAX_MASK_EDGE: u32 = 2048;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrushRaster {
    pub w: u32,
    pub h: u32,
    pub data: Vec<u8>,
}

impl BrushRaster {
    /// An empty mask for a `src`-pixel source: long edge `min(src, 2048)`.
    pub fn new(src: (u32, u32)) -> Self {
        let (w, h) = Self::dims(src);
        Self {
            w,
            h,
            data: vec![0; (w * h) as usize],
        }
    }

    /// The mask size for a `src`-pixel source.
    pub fn dims(src: (u32, u32)) -> (u32, u32) {
        let long = src.0.max(src.1).max(1);
        let s = (f64::from(MAX_MASK_EDGE) / f64::from(long)).min(1.0);
        let dim = |v: u32| ((f64::from(v) * s).round() as u32).max(1);
        (dim(src.0), dim(src.1))
    }

    pub fn clear(&mut self) {
        self.data.fill(0);
    }

    /// Rasterizes every stroke from scratch.
    pub fn rebuild(&mut self, strokes: &[Stroke]) {
        self.clear();
        for s in strokes {
            self.stamp_stroke(s, 0);
        }
    }

    /// Brings the mask from `old` to `new`. When `new` only adds points to
    /// the last stroke and/or appends strokes, only the new dabs are
    /// stamped; otherwise (undo, erase of history, edits) it rebuilds.
    /// Returns true when it was incremental.
    pub fn sync(&mut self, old: &[Stroke], new: &[Stroke]) -> bool {
        let Some(last) = old.last() else {
            self.rebuild(new);
            return false;
        };
        let n = old.len();
        let appendable = new.len() >= n
            && old[..n - 1] == new[..n - 1]
            && same_brush(last, &new[n - 1])
            && new[n - 1].points.starts_with(&last.points);
        if !appendable {
            self.rebuild(new);
            return false;
        }
        self.stamp_stroke(&new[n - 1], last.points.len());
        for s in &new[n..] {
            self.stamp_stroke(s, 0);
        }
        true
    }

    /// Stamps the dabs of `stroke` from point index `from` on (the first
    /// point stamps alone; later points stamp the segment ending there).
    fn stamp_stroke(&mut self, stroke: &Stroke, from: usize) {
        let diag = f64::from(self.w).hypot(f64::from(self.h));
        let radius = (stroke.size * diag / 2.0).max(0.5);
        let step = (radius * 0.25).max(0.5);
        let (fw, fh) = (f64::from(self.w), f64::from(self.h));
        let px = |p: [f64; 2]| (p[0] * fw, p[1] * fh);
        for i in from..stroke.points.len() {
            let b = px(stroke.points[i]);
            if i == 0 {
                self.dab(b, radius, stroke);
                continue;
            }
            let a = px(stroke.points[i - 1]);
            let n = ((b.0 - a.0).hypot(b.1 - a.1) / step).ceil().max(1.0) as u32;
            for k in 1..=n {
                let t = f64::from(k) / f64::from(n);
                self.dab(
                    (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t),
                    radius,
                    stroke,
                );
            }
        }
    }

    /// One soft disc at `c` (raster pixels): painted with max-over, or
    /// erased by multiplying with `1 − dab`.
    fn dab(&mut self, c: (f64, f64), radius: f64, s: &Stroke) {
        let flow = s.flow.clamp(0.0, 1.0);
        let feather = s.feather.clamp(0.0, 1.0);
        let core = radius * (1.0 - feather);
        let x0 = ((c.0 - radius).floor().max(0.0)) as u32;
        let y0 = ((c.1 - radius).floor().max(0.0)) as u32;
        let x1 = (((c.0 + radius).ceil()).max(0.0) as u32).min(self.w);
        let y1 = (((c.1 + radius).ceil()).max(0.0) as u32).min(self.h);
        for y in y0..y1 {
            for x in x0..x1 {
                let d = (f64::from(x) + 0.5 - c.0).hypot(f64::from(y) + 0.5 - c.1);
                let t = if d >= radius {
                    0.0
                } else if d <= core {
                    1.0
                } else {
                    let u = (radius - d) / (radius - core);
                    u * u * (3.0 - 2.0 * u)
                };
                let v = (t * flow * 255.0).round() as u32;
                if v == 0 {
                    continue;
                }
                let cell = &mut self.data[(y * self.w + x) as usize];
                if s.erase {
                    *cell = ((u32::from(*cell) * (255 - v) + 127) / 255) as u8;
                } else {
                    *cell = (*cell).max(v as u8);
                }
            }
        }
    }
}

fn same_brush(a: &Stroke, b: &Stroke) -> bool {
    a.size == b.size && a.feather == b.feather && a.flow == b.flow && a.erase == b.erase
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stroke(points: &[[f64; 2]]) -> Stroke {
        Stroke {
            size: 0.1,
            points: points.to_vec(),
            ..Default::default()
        }
    }

    #[test]
    fn the_mask_fits_the_source_and_is_capped() {
        let r = BrushRaster::new((6000, 4000));
        assert_eq!((r.w, r.h), (2048, 1365));
        let r = BrushRaster::new((600, 400));
        assert_eq!((r.w, r.h), (600, 400));
    }

    #[test]
    fn one_dab_is_a_soft_disc_of_the_right_size() {
        let mut r = BrushRaster::new((400, 400));
        let s = Stroke {
            size: 0.25,
            feather: 0.5,
            points: vec![[0.5, 0.5]],
            ..Default::default()
        };
        r.rebuild(&[s]);
        let radius = 0.25 * 400f64.hypot(400.0) / 2.0;
        let at = |x: f64, y: f64| r.data[(y as u32 * r.w + x as u32) as usize];
        assert_eq!(at(200.0, 200.0), 255, "hard core");
        assert_eq!(at(200.0 + radius + 2.0, 200.0), 0, "outside");
        let mid = at(200.0 + radius * 0.75, 200.0);
        assert!(mid > 100 && mid < 155, "half-way down the ramp: {mid}");
    }

    #[test]
    fn painting_twice_never_exceeds_one_and_erasing_clears() {
        let mut r = BrushRaster::new((200, 200));
        let a = stroke(&[[0.3, 0.5], [0.7, 0.5]]);
        r.rebuild(&[a.clone(), a.clone()]);
        let once = {
            let mut o = BrushRaster::new((200, 200));
            o.rebuild(std::slice::from_ref(&a));
            o
        };
        assert_eq!(r, once, "max-over is idempotent");
        let erase = Stroke {
            erase: true,
            feather: 0.0,
            ..stroke(&[[0.3, 0.5], [0.7, 0.5]])
        };
        r.rebuild(&[a, erase]);
        assert!(r.data.iter().all(|&v| v == 0));
    }

    #[test]
    fn incremental_updates_equal_a_full_rebuild() {
        let pts: Vec<[f64; 2]> = (0..20)
            .map(|i| [0.1 + i as f64 * 0.04, 0.3 + (i as f64 * 0.7).sin() * 0.1])
            .collect();
        let erase = Stroke {
            erase: true,
            ..stroke(&[[0.4, 0.3], [0.5, 0.35]])
        };
        let mut inc = BrushRaster::new((300, 200));
        let mut old: Vec<Stroke> = vec![];
        for n in [1, 2, 5, 11, 20] {
            let new = vec![stroke(&pts[..n])];
            let was_inc = inc.sync(&old, &new);
            assert_eq!(was_inc, !old.is_empty());
            old = new;
        }
        // Appending a stroke (an erase) is incremental too.
        let mut new = old.clone();
        new.push(erase);
        assert!(inc.sync(&old, &new));
        let mut full = BrushRaster::new((300, 200));
        full.rebuild(&new);
        assert_eq!(inc, full, "bit-exact");
        // Removing points (undo) rebuilds.
        let undone = vec![stroke(&pts[..5])];
        assert!(!inc.sync(&new, &undone));
        full.rebuild(&undone);
        assert_eq!(inc, full);
    }
}
