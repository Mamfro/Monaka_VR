use super::*;

const SQUARE: [f32; 4] = [-std::f32::consts::FRAC_PI_4, std::f32::consts::FRAC_PI_4, std::f32::consts::FRAC_PI_4, -std::f32::consts::FRAC_PI_4];

fn close(a: [f32; 2], b: [f32; 2]) -> bool {
    (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3
}

#[test]
fn straight_ahead_is_the_middle() {
    let eye = EyeView { orientation: [0.0, 0.0, 0.0, 1.0], position: [0.0, 1.6, 0.0], fov: SQUARE };
    assert!(close(eye.pixel([0.0, 1.6, -2.0], 100.0, 100.0).unwrap(), [50.0, 50.0]));
    // 45 degrees right and up: the top right corner.
    assert!(close(eye.pixel([1.0, 2.6, -1.0], 100.0, 100.0).unwrap(), [100.0, 0.0]));
    assert_eq!(eye.pixel([0.0, 1.6, 1.0], 100.0, 100.0), None, "behind");
}

#[test]
fn a_turned_eye_sees_what_it_turned_to() {
    // Turned 90 degrees left (about +Y): what was on the left (-X) is straight ahead.
    let half = std::f32::consts::FRAC_PI_4;
    let eye = EyeView { orientation: [0.0, half.sin(), 0.0, half.cos()], position: [1.0, 0.0, 0.0], fov: SQUARE };
    assert!(close(eye.pixel([-2.0, 0.0, 0.0], 100.0, 100.0).unwrap(), [50.0, 50.0]));
    assert_eq!(eye.pixel([3.0, 0.0, 0.0], 100.0, 100.0), None, "behind it now");
}
