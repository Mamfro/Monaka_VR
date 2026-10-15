use super::*;
use crate::math::IDENTITY;

/// A turn of `angle` radians about axis `axis` (unit).
fn about(axis: Vec3, angle: f32) -> Quat {
    let s = (angle / 2.0).sin();
    [axis[0] * s, axis[1] * s, axis[2] * s, (angle / 2.0).cos()]
}

#[test]
fn a_flat_hand_bends_nothing() {
    let joints = [IDENTITY; JOINT_COUNT];
    assert!(bends(1, &joints).iter().flatten().all(|v| v.abs() < 1e-6));
}

#[test]
fn a_curled_finger_bends_toward_the_palm() {
    // The index finger's proximal bone turned 60 degrees toward the palm (about -X: -Z goes
    // toward -Y, the palm side), its intermediate a further 30, as one chain; the rest flat.
    let mut joints = [IDENTITY; JOINT_COUNT];
    let first = about([-1.0, 0.0, 0.0], 60f32.to_radians());
    joints[7] = first;
    joints[8] = multiply(first, about([-1.0, 0.0, 0.0], 30f32.to_radians()));
    joints[9] = joints[8];
    joints[10] = joints[8];
    let index = bends(1, &joints)[1];
    assert!((index[0] - 60f32.to_radians()).abs() < 1e-4, "{index:?}");
    assert!((index[1] - 30f32.to_radians()).abs() < 1e-4, "{index:?}");
    assert!(index[2].abs() < 1e-4 && index[3].abs() < 1e-4, "{index:?}");
}

#[test]
fn spread_is_toward_the_thumb_on_either_hand() {
    // The index's proximal bone turned toward +X (-Z goes toward +X: a turn about -Y).
    let mut joints = [IDENTITY; JOINT_COUNT];
    joints[7] = about([0.0, 1.0, 0.0], -(10f32.to_radians()));
    for i in 8..=10 {
        joints[i] = joints[7];
    }
    // +X is the left hand's thumb side, the right hand's little-finger side.
    assert!(bends(0, &joints)[1][3] > 0.15);
    assert!(bends(1, &joints)[1][3] < -0.15);
}
