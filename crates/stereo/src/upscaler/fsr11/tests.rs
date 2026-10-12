use super::*;
use std::time::Duration;

#[test]
fn a_jitter_moves_the_content_by_its_pixels() {
    let frustum = Frustum { left: -1.0, right: 1.0, up: 1.0, down: -1.0 };
    let shifted = jittered_at(frustum, [1.0, 0.0], (1000, 1000));
    // One pixel of 1000 across a frustum 2 wide is 0.002 of it: the edges move the other way.
    assert!((shifted.left - (-1.002)).abs() < 1e-6 && (shifted.right - 0.998).abs() < 1e-6);
    let down = jittered_at(frustum, [0.0, 1.0], (1000, 1000));
    assert!((down.up - 1.002).abs() < 1e-6, "y down moves the edges up");
}

#[test]
fn a_jump_is_a_cut_and_the_first_frame_has_a_guess() {
    let now = Instant::now();
    assert_eq!(frame_time_and_cut(None, now, [0.0; 3]), (FIRST_FRAME_MS, false));
    let then = now - Duration::from_millis(11);
    let (time, cut) = frame_time_and_cut(Some((then, [0.0; 3])), now, [0.0, 0.0, 1.0]);
    assert!((time - 11.0).abs() < 0.5 && !cut);
    assert!(frame_time_and_cut(Some((then, [0.0; 3])), now, [3.0, 0.0, 0.0]).1);
}

#[test]
fn a_frame_without_its_upscaled_eye_waits_unless_the_view_is_gone() {
    assert_eq!(out(10, Some(10), 0), Out::Eye);
    assert_eq!(out(10, Some(9), 0), Out::Nothing, "an older present's eye");
    assert_eq!(out(10, None, 3), Out::Nothing);
    assert_eq!(out(10, None, FLAT_AFTER), Out::Flat);
    assert_eq!(out(10, Some(10), FLAT_AFTER), Out::Eye);
}

#[test]
fn the_vertical_fov_of_a_centred_projection() {
    let mut projection = [0.0f32; 16];
    projection[5] = 1.0;
    assert!((fov_vertical(&projection) - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
}
