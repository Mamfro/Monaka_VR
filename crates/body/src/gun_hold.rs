//! The hand rig placing a held gun the game holds pointing elsewhere (a pistol in the off hand):
//! after the arms are solved, the hand turns at the wrist to line the gun's barrel up with its
//! controller, and what it holds turns along. A weapon strapped along the forearm: the forearm
//! first goes with the hand as the game posed the two (a straight wrist, the weapon along the arm),
//! the turn takes forearm and hand together, and the upper arm reaches for the elbow where that
//! leaves it.
//!
//! A game says, for each hand, what lines up ([`crate::hand_rig::HandRig::lined_up`]): where the
//! barrel points as the game posed the weapon, the element it hangs from, and which elements turn
//! ([`LinedUp`]). The rig does the rest ([`line_up`]), before the game's own adjustment.
//!
//! A game that places each held gun itself, the arms hanging from the guns, asks where the gun
//! goes instead ([`on_controller`]): on its controller's palm.

use crate::Frames;
use monaka_core::camera::{IDENTITY_MATRIX, Mat34, apply_3x3, apply_head, compose, position, rigid_inverse};
use monaka_core::math::{Vec3, dot, forward_axis, length, sub, unit};
use monaka_contract::protocol::HandPose;
use monaka_core::reach::turn_about;

/// A held weapon lined up with its controller, as the game describes it.
#[derive(Clone, Copy, Debug)]
pub struct LinedUp {
    /// Where the barrel points as the game posed the weapon (world, the arms as animated); none
    /// where it is not known (the weapon then stays as the hand holds it).
    pub barrel: Option<Vec3>,
    /// The element the weapon hangs from: its move from the animated pose carries the barrel.
    pub holder: usize,
    /// The wrist the hand turns about.
    pub wrist: usize,
    /// The elements that turn with the weapon: the hand and what hangs on it.
    pub hand: &'static [usize],
    /// A weapon strapped along the forearm.
    pub forearm: Option<Forearm>,
}

/// A weapon along the forearm: the forearm's elements (the first is the elbow) turn with the hand,
/// and the upper arm's (from the shoulder) reach for the elbow, stretched or shrunk within
/// `stretch`'s least and most (past that it leaves a gap at the elbow).
#[derive(Clone, Copy, Debug)]
pub struct Forearm {
    pub forearm: &'static [usize],
    pub shoulder: usize,
    pub upper_arm: &'static [usize],
    pub stretch: [f32; 2],
}

/// Lines `lined`'s weapon up in `frames` with the controller at `palm` (tracking space) on
/// `origin` (where the tracking space sits in the world).
pub fn line_up(frames: &mut Frames, lined: &LinedUp, origin: &Mat34, palm: Option<&HandPose>) {
    let (animated, now) = (frames.animated, &mut frames.now);
    let elements = lined.hand.iter().chain(lined.forearm.iter().flat_map(|f| f.forearm.iter().chain(f.upper_arm).chain([&f.shoulder])));
    if elements.chain([&lined.holder, &lined.wrist]).any(|&e| e >= now.len() || e >= animated.len()) {
        return;
    }
    let elbow = lined.forearm.as_ref().and_then(|f| f.forearm.first().copied());
    let elbow_before = elbow.map(|e| position(&now[e]));
    if let Some(forearm) = &lined.forearm {
        let hand = compose(&now[lined.wrist], &rigid_inverse(&animated[lined.wrist]));
        for &joint in forearm.forearm {
            now[joint] = compose(&hand, &animated[joint]);
        }
    }
    let attach = compose(&now[lined.holder], &rigid_inverse(&animated[lined.holder]));
    let wrist = position(&now[lined.wrist]);
    let twist = lined.barrel.zip(palm).and_then(|(barrel, palm)| {
        let barrel = apply_3x3(&attach, barrel);
        let line = apply_3x3(origin, forward_axis(palm.orientation));
        turn(wrist, barrel, line).filter(|about| about.iter().all(|v| v.is_finite()))
    });
    if attach.iter().all(|v| v.is_finite())
        && let Some(twist) = twist
    {
        for &joint in lined.forearm.iter().flat_map(|f| f.forearm.iter()).chain(lined.hand) {
            now[joint] = compose(&twist, &now[joint]);
        }
    }
    if let (Some(forearm), Some(elbow), Some(before)) = (&lined.forearm, elbow, elbow_before)
        && let Some(reach) = reach_for(position(&now[forearm.shoulder]), before, position(&now[elbow]), forearm.stretch)
    {
        for &joint in forearm.upper_arm {
            now[joint] = compose(&reach, &now[joint]);
        }
    }
}

/// Where a gun held by its controller goes in the world (canonical): the controller's palm `palm`
/// (tracking space) on `origin`, where the tracking space sits in the world as the view is drawn.
pub fn on_controller(origin: &Mat34, palm: &HandPose) -> Mat34 {
    compose(origin, &apply_head(&IDENTITY_MATRIX, palm.orientation, palm.position))
}

/// The shortest turn about `pivot` taking direction `from` onto `to`; none for directions nearly
/// opposite, whose turn has no settled axis (the hand then stays as it is).
pub fn turn(pivot: Vec3, from: Vec3, to: Vec3) -> Option<Mat34> {
    (dot(unit(from)?, unit(to)?) >= -0.99).then(|| turn_about(pivot, from, to)).flatten()
}

/// The move turning the upper arm at `shoulder` from its elbow at `from` to `to`, stretched along
/// the arm (within `least` to `most` times) to reach it.
pub fn reach_for(shoulder: Vec3, from: Vec3, to: Vec3, [least, most]: [f32; 2]) -> Option<Mat34> {
    let (a, b) = (sub(from, shoulder), sub(to, shoulder));
    let (la, lb) = (length(a), length(b));
    if !(la > 1e-3 && lb > 1e-3) {
        return None;
    }
    let turn = turn([0.0; 3], a, b)?;
    let s = (lb / la).clamp(least, most) - 1.0;
    let d = b.map(|v| v / lb);
    // (I + s d d^T) * turn, about the shoulder.
    let stretched = [
        [1.0 + s * d[0] * d[0], s * d[0] * d[1], s * d[0] * d[2]],
        [s * d[1] * d[0], 1.0 + s * d[1] * d[1], s * d[1] * d[2]],
        [s * d[2] * d[0], s * d[2] * d[1], 1.0 + s * d[2] * d[2]],
    ];
    let stretch_matrix = [
        stretched[0][0], stretched[0][1], stretched[0][2], 0.0, stretched[1][0], stretched[1][1], stretched[1][2], 0.0, stretched[2][0], stretched[2][1], stretched[2][2], 0.0,
    ];
    let linear = compose(&stretch_matrix, &turn);
    let r = [0, 1, 2].map(|row| linear[4 * row] * shoulder[0] + linear[4 * row + 1] * shoulder[1] + linear[4 * row + 2] * shoulder[2]);
    let mut about = linear;
    for row in 0..3 {
        about[4 * row + 3] = shoulder[row] - r[row];
    }
    about.iter().all(|v| v.is_finite()).then_some(about)
}

#[cfg(test)]
mod tests;
