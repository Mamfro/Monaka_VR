//! Tracked fingers on a game's hand skeleton (the viewer's finger block,
//! [`crate::controllers::fingers`]). Each finger is rebuilt from the bone it hangs from out: every
//! knuckle bone takes its rest pose against the bone before it (the model's own rest pose: a
//! skinned mesh's bind pose, a relaxed or flat hand) with the curl the rest pose has about the
//! knuckle's curl axis swapped for the tracked bend. Whatever the game animated (a grip, a fist)
//! goes; the hand itself stays where the rig put it.

use monaka_core::camera::{Mat34, compose, position};
use monaka_core::math::{Vec3, cross, dot, unit};
use monaka_core::protocol::{FingerPose, LEFT_HAND, RIGHT_HAND};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

/// Binary fingers ([`set_binary`]): each finger either open or clenched.
static BINARY: AtomicBool = AtomicBool::new(false);
/// Which fingers are clenched now, by side (thumb first).
static CLENCHED: Mutex<[[bool; 5]; 2]> = Mutex::new([[false; 5]; 2]);

/// A clenched finger's knuckles (radians, from the hand out: a tight fist), and an open one's (a
/// relaxed hand). The thumb has two.
const FIST: [[f32; 3]; 5] = [[0.7, 0.9, 0.0], [1.5, 1.7, 1.1], [1.5, 1.7, 1.1], [1.5, 1.7, 1.1], [1.5, 1.6, 1.0]];
const OPEN: [[f32; 3]; 5] = [[0.1, 0.1, 0.0], [0.1, 0.15, 0.1], [0.1, 0.15, 0.1], [0.12, 0.18, 0.1], [0.15, 0.2, 0.1]];
/// A finger clenches once it curls this share of the way to a fist, and opens again only below
/// the second: a finger resting on a trigger, half curled and twitching, stays one or the other.
const CLENCH_AT: f32 = 0.35;
const OPEN_AT: f32 = 0.2;

/// Turns binary fingers on or off: tracked fingers then snap to open or a tight fist (the Frame's
/// finger sensing reads a finger on a trigger as half curled).
pub fn set_binary(on: bool) {
    BINARY.store(on, Relaxed);
}

/// `tracked` as the game should take it: unchanged, or snapped to open and clenched fingers with
/// binary fingers on (`side` 0 left, 1 right).
pub fn shape(side: usize, tracked: FingerPose) -> FingerPose {
    if !BINARY.load(Relaxed) {
        return tracked;
    }
    let Ok(mut clenched) = CLENCHED.lock() else { return tracked };
    let Some(state) = clenched.get_mut(side) else { return tracked };
    let mut shaped = tracked;
    for (finger, bends) in shaped.fingers.iter_mut().enumerate() {
        let full: f32 = FIST[finger].iter().sum();
        let curl: f32 = bends.iter().take(3).map(|b| if b.is_finite() { b.max(0.0) } else { 0.0 }).sum::<f32>() / full;
        state[finger] = if state[finger] { curl >= OPEN_AT } else { curl >= CLENCH_AT };
        let target = if state[finger] { FIST[finger] } else { OPEN[finger] };
        *bends = [target[0], target[1], target[2], 0.0];
    }
    shaped
}

/// Whether hand `side`'s fingers may follow tracking: the off hand unless it rides a weapon held
/// two-handed ([`crate::off_hand_on_weapon`]), the weapon hand (`aim_side`) only with nothing
/// drawn (`drawn`); a hand that grips keeps the game's grip.
pub fn hand_is_free(side: usize, aim_side: usize, drawn: bool) -> bool {
    if side == aim_side { !drawn } else { !crate::off_hand_on_weapon() }
}

/// Each free hand's tracked fingers ([`crate::controllers::fingers`], when the viewer tracked
/// them lately) handed to `place` with its side, after the rig placed the hands.
pub fn place_free_hands(aim_side: usize, drawn: bool, mut place: impl FnMut(usize, &FingerPose)) {
    for side in [LEFT_HAND, RIGHT_HAND] {
        if hand_is_free(side, aim_side, drawn)
            && let Some(tracked) = crate::controllers::fingers(side)
        {
            place(side, &tracked);
        }
    }
}

/// Joints a hand's fingers are rebuilt on.
pub trait FingerJoints {
    /// Where joint `joint` is now (row-major 3x4, any one space for all joints).
    fn world(&self, joint: i32) -> Option<Mat34>;
    fn set_world(&mut self, joint: i32, matrix: &Mat34);
    /// Joint `to`'s rest pose in joint `from`'s frame (`from` its parent, or the bone a follower
    /// rides).
    fn rest_between(&self, from: i32, to: i32) -> Option<Mat34>;
}

/// One finger.
#[derive(Clone, Debug, Default)]
pub struct Finger {
    /// The bone the bent ones hang from, kept where it is (a metacarpal, or the hand).
    pub base: i32,
    /// The knuckle bones from the hand out, each bent by the next tracked bend (three for a
    /// finger, two for a thumb whose base bone stays).
    pub knuckles: Vec<i32>,
    /// Joints riding a knuckle bone rigidly (end joints, knuckle helpers): (joint, the bone).
    pub followers: Vec<(i32, i32)>,
}

/// A hand's fingers in [`FingerPose::fingers`] order (thumb, index, middle, ring, little), and the
/// skeleton's bone axes: `curl` is the axis a knuckle turns about to curl toward the palm
/// (right-handed, in the bone's own frame); `along` runs down a bone toward its tip, for a last
/// knuckle with nothing after it (otherwise the next joint's rest offset gives it).
#[derive(Clone, Debug, Default)]
pub struct Hand {
    pub fingers: [Option<Finger>; 5],
    pub along: Vec3,
    pub curl: Vec3,
}

/// A knuckle bends no further back than this, nor further in (radians): tracking glitches stay
/// within a hand's reach.
const BACK: f32 = -0.35;
const IN: f32 = 1.9;

/// A turn by `angle` about the unit axis `a` (row-major 3x4, no translation).
fn turn(a: Vec3, angle: f32) -> Mat34 {
    let (s, c) = angle.sin_cos();
    let t = 1.0 - c;
    let [x, y, z] = a;
    [t * x * x + c, t * x * y - s * z, t * x * z + s * y, 0.0, t * x * y + s * z, t * y * y + c, t * y * z - s * x, 0.0, t * x * z - s * y, t * y * z + s * x, t * z * z + c, 0.0]
}

fn apply_rotation(m: &Mat34, v: Vec3) -> Vec3 {
    [0, 1, 2].map(|r| m[4 * r] * v[0] + m[4 * r + 1] * v[1] + m[4 * r + 2] * v[2])
}

/// How far `rest` (a bone's rest pose against the bone before it) curls the bone about `curl`:
/// the angle its `along` axis is turned through about `curl`.
fn rest_curl(rest: &Mat34, along: Vec3, curl: Vec3) -> f32 {
    let turned = apply_rotation(rest, along);
    dot(turned, cross(curl, along)).atan2(dot(turned, along))
}

/// Puts `hand`'s fingers on `tracked`.
pub fn apply(joints: &mut dyn FingerJoints, hand: &Hand, tracked: &FingerPose) {
    for (finger, bends) in hand.fingers.iter().zip(tracked.fingers) {
        let Some(finger) = finger else { continue };
        let Some(mut previous) = joints.world(finger.base) else { continue };
        let mut from = finger.base;
        let mut placed: Vec<(i32, Mat34)> = Vec::with_capacity(finger.knuckles.len());
        for (k, &bone) in finger.knuckles.iter().enumerate() {
            let Some(rest) = joints.rest_between(from, bone) else { break };
            let bend = bends.get(k).copied().filter(|b| b.is_finite()).unwrap_or(0.0).clamp(BACK, IN);
            // Down the bone: toward the next knuckle, or the end joint riding this one.
            let next = finger.knuckles.get(k + 1).copied().or_else(|| finger.followers.iter().find(|f| f.1 == bone).map(|f| f.0));
            let along = next.and_then(|n| joints.rest_between(bone, n)).and_then(|r| unit(position(&r))).unwrap_or(hand.along);
            let local = compose(&rest, &turn(hand.curl, bend - rest_curl(&rest, along, hand.curl)));
            previous = compose(&previous, &local);
            if !previous.iter().all(|v| v.is_finite()) {
                break;
            }
            joints.set_world(bone, &previous);
            placed.push((bone, previous));
            from = bone;
        }
        for &(follower, bone) in &finger.followers {
            let Some(&(_, world)) = placed.iter().find(|(b, _)| *b == bone) else { continue };
            if let Some(rest) = joints.rest_between(bone, follower) {
                joints.set_world(follower, &compose(&world, &rest));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_fingers_snap_with_a_gap() {
        set_binary(true);
        let pose = |bend: f32| FingerPose { valid: 1, fingers: [[bend, bend, bend, 0.2]; 5], ..FingerPose::default() };
        // 0.55 rad per knuckle: 38% of the index's fist, past the 35% that clenches.
        let half = shape(1, pose(0.55));
        assert_eq!(half.fingers[1], [FIST[1][0], FIST[1][1], FIST[1][2], 0.0], "clenched");
        // Easing off to 28% keeps it clenched; 14%, below a fifth, opens it.
        assert_eq!(shape(1, pose(0.4)).fingers[1][0], FIST[1][0], "still clenched");
        assert_eq!(shape(1, pose(0.2)).fingers[1], [OPEN[1][0], OPEN[1][1], OPEN[1][2], 0.0], "open");
        // The other hand keeps its own state.
        assert_eq!(shape(0, pose(0.4)).fingers[1][0], OPEN[1][0], "the left hand never clenched");
        set_binary(false);
        assert_eq!(shape(1, pose(0.4)).fingers[1], [0.4, 0.4, 0.4, 0.2], "off: as tracked");
    }

    const IDENTITY: Mat34 = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0];

    /// A finger of three 4 cm bones along +x from the hand at the origin, its rest pose relaxed
    /// (each knuckle 20 degrees about +y, the curl axis), plus an end joint on the last bone.
    struct Test {
        worlds: Vec<Mat34>,
    }

    impl FingerJoints for Test {
        fn world(&self, joint: i32) -> Option<Mat34> {
            self.worlds.get(joint as usize).copied()
        }
        fn set_world(&mut self, joint: i32, matrix: &Mat34) {
            self.worlds[joint as usize] = *matrix;
        }
        fn rest_between(&self, from: i32, to: i32) -> Option<Mat34> {
            let mut rest = turn([0.0, 1.0, 0.0], 20f32.to_radians());
            // Each bone sits 4 cm along its parent's x (the end, 2 cm).
            rest[3] = if to == 4 { 0.02 } else { 0.04 };
            (to == from + 1).then_some(rest)
        }
    }

    fn hand() -> Hand {
        let index = Finger { base: 0, knuckles: vec![1, 2, 3], followers: vec![(4, 3)] };
        Hand { fingers: [None, Some(index), None, None, None], along: [1.0, 0.0, 0.0], curl: [0.0, 1.0, 0.0] }
    }

    fn tip(bend: f32) -> Vec3 {
        let mut test = Test { worlds: vec![IDENTITY; 5] };
        let mut tracked = FingerPose { valid: 1, ..FingerPose::default() };
        tracked.fingers[1] = [bend, bend, bend, 0.0];
        apply(&mut test, &hand(), &tracked);
        [test.worlds[4][3], test.worlds[4][7], test.worlds[4][11]]
    }

    #[test]
    fn an_open_hand_lies_straight() {
        // No bend: every bone along the hand's x, the relaxed curl taken out.
        let t = tip(0.0);
        assert!((t[0] - 0.14).abs() < 1e-5 && t[1].abs() < 1e-5 && t[2].abs() < 1e-5, "{t:?}");
    }

    #[test]
    fn a_curl_turns_about_the_curl_axis() {
        // A quarter turn at each knuckle about +y (x goes to -z): the bones run 4 cm out along x,
        // down along -z, back along -x, and the end 2 cm up along +z, under the hand.
        let t = tip(std::f32::consts::FRAC_PI_2);
        assert!(t[0].abs() < 1e-5 && t[1].abs() < 1e-5 && (t[2] + 0.02).abs() < 1e-5, "{t:?}");
    }
}
