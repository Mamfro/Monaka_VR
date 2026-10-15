//! Tracked fingers on a game's hand skeleton (the viewer's finger block,
//! [`crate::controllers::fingers`]). Each finger is rebuilt from the bone it hangs from out: every
//! knuckle bone takes its rest pose against the bone before it (the model's own rest pose: a
//! skinned mesh's bind pose, a relaxed or flat hand) with the curl the rest pose has about the
//! knuckle's curl axis swapped for the tracked bend. Whatever the game animated (a grip, a fist)
//! goes; the hand itself stays where the rig put it.

use monaka_core::camera::{Mat34, compose, position};
use monaka_core::math::{Vec3, cross, dot, unit};
use monaka_contract::protocol::{FingerPose, LEFT_HAND, RIGHT_HAND};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::time::Instant;

/// Binary fingers ([`set_binary`]): each finger either open or clenched.
static BINARY: AtomicBool = AtomicBool::new(false);
/// Each hand's fingers with binary fingers on (by side, thumb first).
static HANDS: Mutex<[Clench; 2]> = Mutex::new([Clench::OPEN; 2]);

/// One hand's fingers with binary fingers on: which are clenched, how far each has gone from open
/// toward the fist (0 to 1), and when that was worked out.
#[derive(Clone, Copy)]
struct Clench {
    clenched: [bool; 5],
    amount: [f32; 5],
    at: Option<Instant>,
}

impl Clench {
    const OPEN: Clench = Clench { clenched: [false; 5], amount: [0.0; 5], at: None };
}

/// How long a finger takes to go from open to the fist, or back (seconds): a quick clench, seen.
const CLENCH_SECONDS: f32 = 0.15;

/// A clenched finger's knuckles (radians, from the hand out: a tight fist), and an open one's (a
/// relaxed hand). The thumb has two.
const FIST: [[f32; 3]; 5] = [[0.7, 0.9, 0.0], [1.5, 1.7, 1.1], [1.5, 1.7, 1.1], [1.5, 1.7, 1.1], [1.5, 1.6, 1.0]];
const OPEN: [[f32; 3]; 5] = [[0.1, 0.1, 0.0], [0.1, 0.15, 0.1], [0.1, 0.15, 0.1], [0.12, 0.18, 0.1], [0.15, 0.2, 0.1]];
/// A finger clenches once it curls this share of the way to a fist, and opens again only below
/// the second: a finger resting on a trigger, half curled and twitching, stays one or the other.
const CLENCH_AT: f32 = 0.35;
const OPEN_AT: f32 = 0.2;

/// Turns binary fingers on or off: tracked fingers then go to open or a tight fist (the Frame's
/// finger sensing reads a finger on a trigger as half curled), moving there quickly rather than
/// jumping.
pub fn set_binary(on: bool) {
    BINARY.store(on, Relaxed);
}

/// `tracked` as the game should take it: unchanged, or each finger on its way to open or clenched
/// with binary fingers on (`side` 0 left, 1 right).
pub fn shape(side: usize, tracked: FingerPose) -> FingerPose {
    shape_at(side, tracked, Instant::now())
}

/// [`shape`] at `now`.
fn shape_at(side: usize, tracked: FingerPose, now: Instant) -> FingerPose {
    if !BINARY.load(Relaxed) {
        return tracked;
    }
    let Ok(mut hands) = HANDS.lock() else { return tracked };
    let Some(hand) = hands.get_mut(side) else { return tracked };
    // How far a finger may move since the last look (a hand seen again after a while gets there
    // at once).
    let step = hand.at.map_or(1.0, |at| now.saturating_duration_since(at).as_secs_f32() / CLENCH_SECONDS);
    hand.at = Some(now);
    let mut shaped = tracked;
    for (finger, bends) in shaped.fingers.iter_mut().enumerate() {
        let full: f32 = FIST[finger].iter().sum();
        let curl: f32 = bends.iter().take(3).map(|b| if b.is_finite() { b.max(0.0) } else { 0.0 }).sum::<f32>() / full;
        let clenched = &mut hand.clenched[finger];
        *clenched = if *clenched { curl >= OPEN_AT } else { curl >= CLENCH_AT };
        let amount = &mut hand.amount[finger];
        *amount = if *clenched { (*amount + step).min(1.0) } else { (*amount - step).max(0.0) };
        // Eased in and out: the finger starts and settles gently.
        let eased = *amount * *amount * (3.0 - 2.0 * *amount);
        let knuckle = |k: usize| OPEN[finger][k] + (FIST[finger][k] - OPEN[finger][k]) * eased;
        *bends = [knuckle(0), knuckle(1), knuckle(2), 0.0];
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
mod tests;
