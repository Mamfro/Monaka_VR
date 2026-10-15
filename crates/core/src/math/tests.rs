#[test]
fn a_point_is_placed_along_a_segment() {
    let share = |p| super::segment_share(p, [1.0, 0.0, 0.0], [3.0, 0.0, 0.0]);
    assert_eq!(share([2.0, 5.0, 0.0]), Some(0.5));
    assert_eq!(share([0.0, 0.0, 0.0]), Some(-0.5));
    assert_eq!(super::segment_share([0.0; 3], [1.0, 0.0, 0.0], [1.0, 0.0, 0.0]), None);
}

#[test]
fn a_turn_takes_from_onto_to_and_keeps_lengths() {
    let (from, to) = ([0.0, 0.0, -1.0], super::unit([0.3, 0.2, -1.0]).unwrap());
    let onto = super::turned(from, from, to);
    assert!(onto.iter().zip(to).all(|(a, b)| (a - b).abs() < 1e-6), "{onto:?}");
    let v = [0.4, -1.0, 0.2];
    assert!((super::length(super::turned(v, from, to)) - super::length(v)).abs() < 1e-6);
    assert_eq!(super::turned(v, from, from), v, "no turn");
}

#[test]
fn lean_only_removes_the_necks_turning() {
    // Nodding 30 degrees down about the neck moves the eyes forward and down.
    let half = (-30f32).to_radians() * 0.5;
    let q = [half.sin(), 0.0, 0.0, half.cos()];
    let turned = rotate(q, NECK_TO_EYES);
    let nodded = [0, 1, 2].map(|k| turned[k] - NECK_TO_EYES[k]);
    assert!(nodded[2] < -0.01 && nodded[1] < 0.0, "forward and down: {nodded:?}");
    let lean = lean_only(q, nodded);
    assert!(lean.iter().all(|v| v.abs() < 1e-5), "{lean:?}");
    // A lean with the head level is kept.
    assert_eq!(lean_only(IDENTITY, [0.1, 0.0, -0.05]), [0.1, 0.0, -0.05]);
    // rotate agrees with the rotation matrix.
    let r = crate::camera::rotation(q);
    let v = [0.3, -0.2, 0.5];
    let m = [0, 1, 2].map(|i| r[3 * i] * v[0] + r[3 * i + 1] * v[1] + r[3 * i + 2] * v[2]);
    assert!(rotate(q, v).iter().zip(m).all(|(a, b)| (a - b).abs() < 1e-6));
}

use super::*;

#[test]
fn angles_wrap_to_one_turn() {
    assert!((wrap_radians(3.0 * std::f32::consts::PI) - std::f32::consts::PI).abs() < 1e-5 || (wrap_radians(3.0 * std::f32::consts::PI) + std::f32::consts::PI).abs() < 1e-5);
    assert!((wrap_radians(-0.5) + 0.5).abs() < 1e-6);
    assert_eq!(wrap_degrees(190.0), -170.0);
    assert_eq!(wrap_degrees(-190.0), 170.0);
    assert_eq!(wrap_degrees(45.0), 45.0);
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-5
}

#[test]
fn along_follows_the_pointer() {
    assert_eq!(forward_axis(IDENTITY), [0.0, 0.0, -1.0]);
    // Turned 90 degrees left, forward is -X.
    let p = along([0.0, 1.6, 0.0], yaw_rotation(core::f32::consts::FRAC_PI_2), 10.0);
    assert!(close(p[0], -10.0) && close(p[1], 1.6) && p[2].abs() < 1e-5, "{p:?}");
}

#[test]
fn yaw_round_trips() {
    assert!(close(yaw(IDENTITY), 0.0));
    let left = yaw_rotation(core::f32::consts::FRAC_PI_2);
    assert!(close(yaw(left), core::f32::consts::FRAC_PI_2));
}

#[test]
fn eyes_straddle_the_head() {
    let pose = HeadPose { ipd: 0.064, valid: 1, ..HeadPose::default() };
    assert!(close(eye_position(&pose, 0)[0], -0.032));
    assert!(close(eye_position(&pose, 1)[0], 0.032));
    // Turned 90 degrees left (looking down -X), the right eye moves toward -Z.
    let turned = HeadPose { orientation: yaw_rotation(core::f32::consts::FRAC_PI_2), ..pose };
    let right = eye_position(&turned, 1);
    assert!(close(right[0], 0.0) && close(right[2], -0.032));
}

#[test]
fn head_from_views_measures_ipd() {
    let head = head_from_views(IDENTITY, [[-0.03, 1.6, 0.0], [0.03, 1.6, 0.0]], [[0.0; 4]; 2]);
    assert!(close(head.ipd, 0.06));
    assert!(close(head.position[1], 1.6) && close(head.position[0], 0.0));
}

#[test]
fn screen_anchors_ahead() {
    let (_, ahead) = ahead_of(IDENTITY, [0.0, 1.6, 0.0], 2.0);
    assert!(close(ahead[2], -2.0) && close(ahead[1], 1.6));
    let (rotation, left) = ahead_of(yaw_rotation(core::f32::consts::FRAC_PI_2), [0.0; 3], 2.0);
    assert!(close(left[0], -2.0) && close(left[2], 0.0));
    assert!(close(yaw(rotation), core::f32::consts::FRAC_PI_2));
}
