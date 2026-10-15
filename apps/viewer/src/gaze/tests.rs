use super::*;

fn close(a: Vec3, b: Vec3) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5)
}

/// A turn of `degrees` about unit axis `axis`.
fn turn(axis: Vec3, degrees: f32) -> Quat {
    let (s, c) = (degrees.to_radians() * 0.5).sin_cos();
    [axis[0] * s, axis[1] * s, axis[2] * s, c]
}

#[test]
fn a_pose_looks_along_its_forward() {
    let ahead = from_pose(math::IDENTITY, [0.0, 0.0, 0.0], 77, 5).unwrap();
    assert_eq!(ahead, EyeGaze { direction: [0.0, 0.0, -1.0], origin: [0.0; 3], sample_time: 77, received_tick: 5, valid: 1 });
    // Looking 90 degrees to the left (a turn about +Y): -X in VIEW space.
    let left = from_pose(turn([0.0, 1.0, 0.0], 90.0), [0.01, 0.0, -0.02], 0, 0).unwrap();
    assert!(close(left.direction, [-1.0, 0.0, 0.0]), "{:?}", left.direction);
    assert_eq!(left.origin, [0.01, 0.0, -0.02]);
    // Looking 30 degrees down (a turn about +X, negative).
    let down = from_pose(turn([1.0, 0.0, 0.0], -30.0), [0.0; 3], 0, 0).unwrap();
    let (s, c) = 30f32.to_radians().sin_cos();
    assert!(close(down.direction, [0.0, -s, -c]), "{:?}", down.direction);
    assert!(from_pose([f32::NAN, 0.0, 0.0, 1.0], [0.0; 3], 0, 0).is_none(), "no finite direction");
}

#[test]
fn two_eyes_meet_between_them() {
    let ipd = 0.064;
    // Each eye turned 5 degrees inward, converging ahead.
    let left = (turn([0.0, 1.0, 0.0], -5.0), [-ipd / 2.0, 0.0, 0.0]);
    let right = (turn([0.0, 1.0, 0.0], 5.0), [ipd / 2.0, 0.0, 0.0]);
    let both = from_eyes([Some(left), Some(right)], 9, 3).unwrap();
    assert!(close(both.direction, [0.0, 0.0, -1.0]), "{:?}", both.direction);
    assert!(close(both.origin, [0.0; 3]));
    assert_eq!((both.sample_time, both.received_tick, both.valid), (9, 3, 1));
    // One eye alone stands in for both.
    let one = from_eyes([None, Some(right)], 9, 3).unwrap();
    assert_eq!(one, from_pose(right.0, right.1, 9, 3).unwrap());
    assert!(from_eyes([None, None], 9, 3).is_none());
}
