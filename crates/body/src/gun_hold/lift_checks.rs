//! The rig's lining up against Dishonored's left weapon before it, on its arm's joints.

use super::*;
use crate::Snapshot;
use monaka_core::math::rotate;

/// Dishonored's left arm: the hand and what hangs on it (joints 4 to 61), the forearm and its
/// helpers, the upper arm and its twist helper, the hand attachment.
const BLOCK_HAND: [usize; 58] = {
    let mut joints = [0; 58];
    let mut k = 0;
    while k < 58 {
        joints[k] = 4 + k;
        k += 1;
    }
    joints
};
const FOREARM_JOINTS: [usize; 7] = [3, 62, 63, 64, 65, 66, 67];
const UPPER_ARM_JOINTS: [usize; 2] = [2, 68];
const ATTACH: usize = 59;

fn old_turn(pivot: Vec3, from: Vec3, to: Vec3) -> Option<Mat34> {
    (dot(unit(from)?, unit(to)?) >= -0.99).then(|| turn_about(pivot, from, to)).flatten()
}

fn old_reach_for(shoulder: Vec3, from: Vec3, to: Vec3) -> Option<Mat34> {
    let (a, b) = (sub(from, shoulder), sub(to, shoulder));
    let (la, lb) = (length(a), length(b));
    if !(la > 1e-3 && lb > 1e-3) {
        return None;
    }
    let turn = old_turn([0.0; 3], a, b)?;
    let s = (lb / la).clamp(0.6, 1.6) - 1.0;
    let d = b.map(|v| v / lb);
    let stretch = [
        1.0 + s * d[0] * d[0],
        s * d[0] * d[1],
        s * d[0] * d[2],
        0.0,
        s * d[1] * d[0],
        1.0 + s * d[1] * d[1],
        s * d[1] * d[2],
        0.0,
        s * d[2] * d[0],
        s * d[2] * d[1],
        1.0 + s * d[2] * d[2],
        0.0,
    ];
    let linear = compose(&stretch, &turn);
    let r = [0, 1, 2].map(|row| linear[4 * row] * shoulder[0] + linear[4 * row + 1] * shoulder[1] + linear[4 * row + 2] * shoulder[2]);
    let mut about = linear;
    for row in 0..3 {
        about[4 * row + 3] = shoulder[row] - r[row];
    }
    about.iter().all(|v| v.is_finite()).then_some(about)
}

/// Dishonored's `adjust` before, its inputs passed in: the weapon's barrel as the game posed it
/// (none: no line-up), the controller, the tracking origin, whether it is along the forearm.
fn old_adjust(animated: &[Mat34], now: &mut [Mat34], barrel: Option<Vec3>, palm: Option<&HandPose>, origin: &Mat34, straight: bool) {
    let elbow_before = position(&now[3]);
    if straight {
        let hand = compose(&now[4], &rigid_inverse(&animated[4]));
        for joint in FOREARM_JOINTS {
            now[joint] = compose(&hand, &animated[joint]);
        }
    }
    let attach = compose(&now[ATTACH], &rigid_inverse(&animated[ATTACH]));
    let wrist = position(&now[4]);
    let twist = |attach: &Mat34| {
        let (barrel, palm) = (barrel?, palm?);
        let barrel = apply_3x3(attach, barrel);
        let line = apply_3x3(origin, forward_axis(palm.orientation));
        old_turn(wrist, barrel, line).filter(|about| about.iter().all(|v| v.is_finite()))
    };
    if attach.iter().all(|v| v.is_finite())
        && let Some(twist) = twist(&attach)
    {
        let block: Vec<usize> = if straight { FOREARM_JOINTS.iter().chain(&BLOCK_HAND).copied().collect() } else { BLOCK_HAND.to_vec() };
        for joint in block {
            now[joint] = compose(&twist, &now[joint]);
        }
    }
    if straight && let Some(reach) = old_reach_for(position(&now[2]), elbow_before, position(&now[3])) {
        for joint in UPPER_ARM_JOINTS {
            now[joint] = compose(&reach, &now[joint]);
        }
    }
}

/// A frame turned by `q` (quaternion) at `p`.
fn frame(q: [f32; 4], p: Vec3) -> Mat34 {
    let [x, y, z] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]].map(|axis| rotate(q, axis));
    [x[0], y[0], z[0], p[0], x[1], y[1], z[1], p[1], x[2], y[2], z[2], p[2]]
}

fn quat(axis: Vec3, angle: f32) -> [f32; 4] {
    let a = unit(axis).unwrap();
    let s = (angle / 2.0).sin();
    [a[0] * s, a[1] * s, a[2] * s, (angle / 2.0).cos()]
}

#[test]
fn the_left_weapon_lines_up_as_dishonored_lined_it_up() {
    let mut compared = 0;
    for i in 0..60 {
        let t = i as f32 * 0.13;
        // The arm as animated, and as the rig solved it (each joint moved a little differently).
        let animated: Vec<Mat34> = (0..69).map(|j| frame(quat([0.3, 1.0, 0.2 * j as f32], 0.1 * j as f32), [0.01 * j as f32, 1.2 - 0.005 * j as f32, -0.2 - 0.004 * j as f32])).collect();
        let solved: Vec<Mat34> = animated.iter().enumerate().map(|(j, m)| compose(&frame(quat([1.0, 0.2 * t.sin(), 0.4], 0.3 + 0.05 * (j % 7) as f32 + 0.2 * t.cos()), [0.05 * t.cos(), 0.02, -0.03]), m)).collect();
        let barrel = (i % 9 != 0).then(|| unit([0.2 * t.cos(), 1.0, -0.4]).unwrap());
        let palm = (i % 7 != 0).then(|| HandPose { position: [0.1, 1.1, -0.3], orientation: quat([0.2, 1.0, 0.1 * t], 0.4 * t.sin() + if i % 11 == 0 { 3.1 } else { 0.0 }), valid: 1, received_tick: 0 });
        let origin = frame(quat([0.0, 1.0, 0.0], 0.7 * t), [3.0, 0.0, -2.0]);
        for straight in [false, true] {
            let mut old = solved.clone();
            old_adjust(&animated, &mut old, barrel, palm.as_ref(), &origin, straight);
            let snapshot = Snapshot::of(1, animated.clone()).unwrap();
            let mut frames = crate::Frames::new(&snapshot);
            frames.now = solved.clone();
            let forearm = straight.then_some(Forearm { forearm: &FOREARM_JOINTS, shoulder: 2, upper_arm: &UPPER_ARM_JOINTS, stretch: [0.6, 1.6] });
            let lined = LinedUp { barrel, holder: ATTACH, wrist: 4, hand: &BLOCK_HAND, forearm };
            line_up(&mut frames, &lined, &origin, palm.as_ref());
            assert_eq!(frames.now, old, "{i} {straight}");
            compared += (frames.now != solved) as usize;
        }
    }
    assert!(compared > 60, "{compared}");
}
