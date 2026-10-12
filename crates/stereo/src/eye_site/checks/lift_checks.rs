//! The eye-site measurements against Dying Light 1's own, which they were lifted from.

use super::*;

/// DL1's step measurement as it was: both steps longer than 5 mm, then the dot product's sign.
fn step_before(older: [f32; 3], last: [f32; 3], now: [f32; 3]) -> Option<bool> {
    let step = |a: [f32; 3], b: [f32; 3]| [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let (before, after) = (step(older, last), step(last, now));
    let length = |v: [f32; 3]| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if length(before) > 0.005 && length(after) > 0.005 {
        let dot = before[0] * after[0] + before[1] * after[1] + before[2] * after[2];
        return Some(dot < 0.0);
    }
    None
}

/// DL1's camera match as it was: neither within the match distance, else the nearer (a tie a turn).
fn kind_before(to_turned: f32, to_base: f32, within: f32) -> usize {
    let nearest = to_turned.min(to_base);
    if nearest >= within {
        2
    } else if to_turned <= to_base {
        0
    } else {
        1
    }
}

#[test]
fn the_measurements_match_dl1s() {
    let points = [[0.0, 0.0, 0.0], [0.004, 0.0, 0.0], [0.01, 0.0, 0.0], [0.0, 0.02, 0.0], [-0.01, 0.0, 0.003], [f32::NAN, 0.0, 0.0]];
    for a in points {
        for b in points {
            for c in points {
                assert_eq!(step_back(a, b, c).map(|s| s.0), step_before(a, b, c), "{a:?} {b:?} {c:?}");
            }
        }
    }
    let distances = [0.0, 0.005, 0.0099, 0.01, 0.2, f32::INFINITY];
    for t in distances {
        for b in distances {
            assert_eq!(camera_kind(t, b, 0.01), kind_before(t, b, 0.01), "{t} {b}");
        }
    }
    let wrap = monaka_core::math::wrap_radians;
    for (older, last, yaw) in [(0.0, 0.1, 0.3), (3.0, 3.1, -3.1), (-1.0, -1.2, -1.5)] {
        assert_eq!(off_line(older, last, yaw), wrap(yaw - last) - wrap(last - older));
    }
}
