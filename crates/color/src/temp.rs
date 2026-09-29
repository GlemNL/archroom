//! Temp/Tint ↔ chromaticity via Robertson's isotemperature lines, the
//! method used by the DNG SDK (plan §6.4). Tint is the signed offset from
//! the Planckian locus along the isotherm, scaled by −3000 as in the DNG SDK. Like Lightroom, the value
//! describes the *illuminant*: positive = a greener light (so the correction
//! applied to the image is magenta); D65 reads about +10.

use crate::cie::Xy;

pub const MIN_TEMP: f64 = 2000.0;
pub const MAX_TEMP: f64 = 50000.0;
const TINT_SCALE: f64 = -3000.0;

/// (mired, u, v, isotherm slope) — Wyszecki & Stiles / Robertson.
const TABLE: [(f64, f64, f64, f64); 31] = [
    (0.0, 0.18006, 0.26352, -0.24341),
    (10.0, 0.18066, 0.26589, -0.25479),
    (20.0, 0.18133, 0.26846, -0.26876),
    (30.0, 0.18208, 0.27119, -0.28539),
    (40.0, 0.18293, 0.27407, -0.30470),
    (50.0, 0.18388, 0.27709, -0.32675),
    (60.0, 0.18494, 0.28021, -0.35156),
    (70.0, 0.18611, 0.28342, -0.37915),
    (80.0, 0.18740, 0.28668, -0.40955),
    (90.0, 0.18880, 0.28997, -0.44278),
    (100.0, 0.19032, 0.29326, -0.47888),
    (125.0, 0.19462, 0.30141, -0.58204),
    (150.0, 0.19962, 0.30921, -0.70471),
    (175.0, 0.20525, 0.31647, -0.84901),
    (200.0, 0.21142, 0.32312, -1.0182),
    (225.0, 0.21807, 0.32909, -1.2168),
    (250.0, 0.22511, 0.33439, -1.4512),
    (275.0, 0.23247, 0.33904, -1.7298),
    (300.0, 0.24010, 0.34308, -2.0637),
    (325.0, 0.24792, 0.34655, -2.4681),
    (350.0, 0.25591, 0.34951, -2.9641),
    (375.0, 0.26400, 0.35200, -3.5814),
    (400.0, 0.27218, 0.35407, -4.3633),
    (425.0, 0.28039, 0.35577, -5.3762),
    (450.0, 0.28863, 0.35714, -6.7262),
    (475.0, 0.29685, 0.35823, -8.5955),
    (500.0, 0.30505, 0.35907, -11.324),
    (525.0, 0.31320, 0.35968, -15.628),
    (550.0, 0.32129, 0.36011, -23.325),
    (575.0, 0.32931, 0.36038, -40.770),
    (600.0, 0.33724, 0.36051, -116.45),
];

fn xy_to_uv(xy: Xy) -> (f64, f64) {
    let d = -xy.x + 6.0 * xy.y + 1.5;
    (2.0 * xy.x / d, 3.0 * xy.y / d)
}

fn uv_to_xy(u: f64, v: f64) -> Xy {
    let d = u - 4.0 * v + 2.0;
    Xy::new(1.5 * u / d, v / d)
}

/// Unit isotherm direction at table row `i`.
fn dir(i: usize) -> (f64, f64) {
    let t = TABLE[i].3;
    let len = (1.0 + t * t).sqrt();
    (1.0 / len, t / len)
}

/// Chromaticity for a temperature (K) and tint.
pub fn temp_tint_to_xy(temp: f64, tint: f64) -> Xy {
    let r = (1.0e6 / temp.clamp(MIN_TEMP, MAX_TEMP)).clamp(TABLE[0].0, TABLE[30].0);
    let mut i = 0;
    while i < 29 && r >= TABLE[i + 1].0 {
        i += 1;
    }
    let f = (TABLE[i + 1].0 - r) / (TABLE[i + 1].0 - TABLE[i].0);
    let u = TABLE[i].1 * f + TABLE[i + 1].1 * (1.0 - f);
    let v = TABLE[i].2 * f + TABLE[i + 1].2 * (1.0 - f);
    let (d1, d2) = (dir(i), dir(i + 1));
    let (mut du, mut dv) = (d1.0 * f + d2.0 * (1.0 - f), d1.1 * f + d2.1 * (1.0 - f));
    let len = du.hypot(dv);
    du /= len;
    dv /= len;
    let offset = tint / TINT_SCALE;
    uv_to_xy(u + du * offset, v + dv * offset)
}

/// Temperature (K) and tint for a chromaticity; clamped to the slider range.
pub fn xy_to_temp_tint(xy: Xy) -> (f64, f64) {
    let (u, v) = xy_to_uv(xy);
    // Signed distance from each isotherm; the sign flips across the match.
    let dist = |i: usize| {
        let (_, ui, vi, t) = TABLE[i];
        ((v - vi) - t * (u - ui)) / (1.0 + t * t).sqrt()
    };
    let mut last = dist(0);
    let mut idx = 30;
    let mut f = 0.0;
    for i in 1..31 {
        let d = dist(i);
        if d <= 0.0 || i == 30 {
            // Between rows i-1 and i.
            let denom = last - d;
            f = if denom.abs() < 1e-15 {
                0.0
            } else {
                last / denom
            };
            idx = i;
            break;
        }
        last = d;
    }
    let f = f.clamp(0.0, 1.0);
    let mired = TABLE[idx - 1].0 + f * (TABLE[idx].0 - TABLE[idx - 1].0);
    let temp = (1.0e6 / mired.max(1e-6)).clamp(MIN_TEMP, MAX_TEMP);

    // Tint: offset from the interpolated locus point along the isotherm.
    let (a, b) = (TABLE[idx - 1], TABLE[idx]);
    let (lu, lv) = (a.1 + f * (b.1 - a.1), a.2 + f * (b.2 - a.2));
    let (d1, d2) = (dir(idx - 1), dir(idx));
    let (mut du, mut dv) = (d1.0 * (1.0 - f) + d2.0 * f, d1.1 * (1.0 - f) + d2.1 * f);
    let len = du.hypot(dv);
    du /= len;
    dv /= len;
    let tint = ((u - lu) * du + (v - lv) * dv) * TINT_SCALE;
    (temp, tint.clamp(-150.0, 150.0))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;

    #[test]
    fn planckian_locus_matches_known_chromaticities() {
        // CIE 1931 blackbody chromaticities (Wyszecki & Stiles / Kim et al.).
        for (t, x, y) in [
            (2856.0, 0.4476, 0.4074),
            (5000.0, 0.3451, 0.3516),
            (6500.0, 0.3135, 0.3237),
            (10000.0, 0.2807, 0.2884),
        ] {
            let xy = temp_tint_to_xy(t, 0.0);
            assert_abs_diff_eq!(xy.x, x, epsilon = 1.5e-3);
            assert_abs_diff_eq!(xy.y, y, epsilon = 1.5e-3);
        }
    }

    #[test]
    fn temp_tint_round_trips() {
        for t in [2500.0, 3200.0, 4500.0, 5600.0, 6500.0, 8000.0, 12000.0] {
            for tint in [-80.0, -20.0, 0.0, 15.0, 60.0] {
                let (t2, tint2) = xy_to_temp_tint(temp_tint_to_xy(t, tint));
                assert!((t2 - t).abs() / t < 0.01, "T {t}/{tint} -> {t2}/{tint2}");
                assert!(
                    (tint2 - tint).abs() < 1.0,
                    "tint {t}/{tint} -> {t2}/{tint2}"
                );
            }
        }
    }

    #[test]
    fn positive_tint_is_a_greener_illuminant_and_d65_reads_about_plus_ten() {
        // Lightroom describes the illuminant: +tint = greener light.
        let g = temp_tint_to_xy(5500.0, 50.0);
        let m = temp_tint_to_xy(5500.0, -50.0);
        assert!(g.y > m.y);
        let (t, tint) = xy_to_temp_tint(crate::cie::D65);
        assert!(
            (t - 6504.0).abs() < 60.0 && (tint - 9.8).abs() < 2.0,
            "{t} K / {tint}"
        );
    }
}
