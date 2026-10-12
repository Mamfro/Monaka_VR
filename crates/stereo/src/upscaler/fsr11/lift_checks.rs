//! The D3D11 per-eye upscaler against Dying Light 1's own FSR it was lifted from.

use super::*;
use std::time::Duration;

/// DL1's jitter shift as it was.
fn jittered_before(frustum: Frustum, jitter: [f32; 2], (width, height): (u32, u32)) -> Frustum {
    let dx = 2.0 * jitter[0] / width as f32 * (frustum.right - frustum.left) / 2.0;
    let dy = -2.0 * jitter[1] / height as f32 * (frustum.up - frustum.down) / 2.0;
    Frustum { left: frustum.left - dx, right: frustum.right - dx, up: frustum.up - dy, down: frustum.down - dy }
}

/// DL1's frame time and cut as they were.
fn time_before(last: Option<(Instant, [f32; 3])>, now: Instant, position: [f32; 3]) -> (f32, bool) {
    match last {
        Some((then, at)) => {
            let moved = (0..3).map(|i| (position[i] - at[i]).powi(2)).sum::<f32>().sqrt();
            ((now - then).as_secs_f32() * 1000.0, moved > 2.0)
        }
        None => (22.0, false),
    }
}

/// DL1's choice at the present as it was: its own eye for this present, flat after four frames
/// without a 3D view, else nothing.
fn out_before(n: u64, made: Option<u64>, viewless: u32) -> Out {
    match made {
        Some(shown) if shown == n => Out::Eye,
        _ if viewless >= 4 => Out::Flat,
        _ => Out::Nothing,
    }
}

#[test]
fn the_upscaler_decides_as_dl1s_did() {
    let frusta = [Frustum { left: -1.0, right: 1.0, up: 1.0, down: -1.0 }, Frustum { left: -1.2, right: 0.8, up: 0.9, down: -1.1 }];
    for frustum in frusta {
        for jitter in [[0.0, 0.0], [0.25, -0.4], [-0.5, 0.5]] {
            for render in [(1440, 1600), (1000, 1000)] {
                assert_eq!(jittered_at(frustum, jitter, render), jittered_before(frustum, jitter, render));
            }
        }
    }
    let now = Instant::now();
    for last in [None, Some((now - Duration::from_millis(14), [1.0, 2.0, 3.0]))] {
        for position in [[1.0, 2.0, 3.0], [1.0, 2.0, 5.5], [9.0, 2.0, 3.0]] {
            assert_eq!(frame_time_and_cut(last, now, position), time_before(last, now, position));
        }
    }
    for n in 0..6 {
        for made in [None, Some(0), Some(3), Some(5)] {
            for viewless in 0..6 {
                assert_eq!(out(n, made, viewless), out_before(n, made, viewless));
            }
        }
    }
}
