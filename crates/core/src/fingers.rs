//! Finger tracking: OpenXR's hand joints (`XR_EXT_hand_tracking`) turned into game-neutral bends
//! for a producer's hand skeleton. Per finger: how far each of its three knuckles bends toward
//! the palm, from the hand out, and how far the first one spreads it toward the thumb.

use crate::math::{Quat, Vec3, conjugate, multiply, rotate};

/// OpenXR's hand joints (`XrHandJointEXT`): palm, wrist, then each finger from its base.
pub const JOINT_COUNT: usize = 26;
/// Each finger's joints, base to tip (`XR_HAND_JOINT_*_EXT`): the thumb has no intermediate.
const THUMB: [usize; 4] = [2, 3, 4, 5];
const FINGERS: [[usize; 5]; 4] = [[6, 7, 8, 9, 10], [11, 12, 13, 14, 15], [16, 17, 18, 19, 20], [21, 22, 23, 24, 25]];

pub const THUMB_FINGER: usize = 0;
/// Fingers in [`monaka_contract::protocol::FingerPose::fingers`]: thumb, index, middle, ring, little.
pub const FINGER_COUNT: usize = 5;

/// How the bone after a joint turns from the bone before it, in the earlier bone's frame
/// (OpenXR joint axes: -Z along the bone toward the tip, +Y out of the back of the hand): its bend
/// toward the palm and its turn toward +X (radians).
fn bend(parent: Quat, child: Quat) -> (f32, f32) {
    let d: Vec3 = rotate(multiply(conjugate(parent), child), [0.0, 0.0, -1.0]);
    ((-d[1]).atan2(-d[2]), d[0].atan2(-d[2]))
}

/// Each finger's bends from the 26 joint orientations of hand `side` (0 left, 1 right): per finger
/// the bends of its three knuckles from the hand out and the spread of the first toward the thumb
/// (radians). The thumb's three: its base knuckle (metacarpal to proximal), its tip knuckle, and
/// none; the thumb's own base turn from the wrist is left out (it carries the thumb's rest pose).
pub fn bends(side: usize, joints: &[Quat; JOINT_COUNT]) -> [[f32; 4]; FINGER_COUNT] {
    // +X is the hand's right side for both hands (OpenXR's axes are right-handed): the thumb is
    // on the left of a right hand and on the right of a left one, palm down.
    let toward_thumb = if side == 0 { 1.0 } else { -1.0 };
    let mut out = [[0.0; 4]; FINGER_COUNT];
    let (a, _) = bend(joints[THUMB[0]], joints[THUMB[1]]);
    let (b, _) = bend(joints[THUMB[1]], joints[THUMB[2]]);
    out[THUMB_FINGER] = [a, b, 0.0, 0.0];
    for (finger, chain) in FINGERS.iter().enumerate() {
        let (base, spread) = bend(joints[chain[0]], joints[chain[1]]);
        let (middle, _) = bend(joints[chain[1]], joints[chain[2]]);
        let (tip, _) = bend(joints[chain[2]], joints[chain[3]]);
        out[finger + 1] = [base, middle, tip, spread * toward_thumb];
    }
    out
}

#[cfg(test)]
mod tests;
