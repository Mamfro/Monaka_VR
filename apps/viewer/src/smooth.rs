//! Controller smoothing (`--controller-smoothing`, 0 off to 10): steadies the small shakes in the
//! controllers' tracking before any game reads them, without slowing real movement. Each hand's
//! grip pose goes through a One Euro filter (Casiez, Roussel and Vogel): a low-pass
//! whose cutoff rises with the hand's speed, so a hand held still is steadied hard while a swing or
//! a quick aim passes almost as tracked. Position and orientation are filtered apart, each by its
//! own speed. The aim pose keeps its offset from the grip (both come from one tracked controller),
//! so the two never drift apart.

use monaka_core::math::{self, Quat, Vec3, angle_between, conjugate, multiply, quat_unit, same_side};
use monaka_core::protocol::HandPose;

/// The cutoff (Hz) at rest at strength 1; strength `s` divides it by `s` (10 Hz at 1, 1 Hz at 10).
const REST_CUTOFF: f32 = 10.0;
/// How much the cutoff rises per metre per second of hand speed (Hz).
const POSITION_BETA: f32 = 30.0;
/// How much the cutoff rises per radian per second of hand turn (Hz).
const TURN_BETA: f32 = 10.0;
/// The cutoff of the speeds themselves (Hz), so a single jittery frame does not open the filter.
const SPEED_CUTOFF: f32 = 1.0;
/// A gap longer than this (seconds; the controller lost or the headset idle) starts afresh.
const MAX_GAP: f32 = 0.1;

pub struct Smoother {
    /// The cutoff at rest (Hz); `None`: off.
    rest_cutoff: Option<f32>,
    hands: [Option<Filtered>; 2],
}

/// One hand's filter state, from the last pose put out.
#[derive(Clone, Copy)]
struct Filtered {
    /// The display time it was put out for (nanoseconds).
    at: i64,
    position: Vec3,
    orientation: Quat,
    /// Filtered speed (m/s) and turn rate (rad/s).
    speed: f32,
    turn: f32,
}

impl Smoother {
    /// A smoother at `strength` (0 off, 10 strongest).
    pub fn new(strength: f32) -> Smoother {
        Smoother { rest_cutoff: (strength > 0.0).then(|| REST_CUTOFF / strength.min(10.0)), hands: [None; 2] }
    }

    /// Hand `hand`'s aim and grip poses, located for display time `at` (nanoseconds), steadied.
    /// The grip is filtered (the aim where the grip was not located) and the other follows it.
    pub fn smooth(&mut self, hand: usize, at: i64, aim: HandPose, grip: HandPose) -> (HandPose, HandPose) {
        let (Some(rest_cutoff), Some(state)) = (self.rest_cutoff, self.hands.get_mut(hand)) else { return (aim, grip) };
        let base = if grip.valid != 0 { grip } else { aim };
        if base.valid == 0 {
            *state = None;
            return (aim, grip);
        }
        let filtered = match *state {
            Some(last) if at > last.at && ((at - last.at) as f32 * 1e-9) <= MAX_GAP => step(last, at, base, rest_cutoff),
            _ => Filtered { at, position: base.position, orientation: base.orientation, speed: 0.0, turn: 0.0 },
        };
        *state = Some(filtered);
        let follow = |pose: HandPose| if pose.valid == 0 { pose } else { carried(base, pose, filtered) };
        (follow(aim), follow(grip))
    }
}

/// The next filtered pose after `last`, toward `pose` located for `at`.
fn step(last: Filtered, at: i64, pose: HandPose, rest_cutoff: f32) -> Filtered {
    let dt = (at - last.at) as f32 * 1e-9;
    let speed = low_pass(last.speed, math::distance(last.position, pose.position) / dt, alpha(SPEED_CUTOFF, dt));
    let to = same_side(last.orientation, pose.orientation);
    let turn = low_pass(last.turn, angle_between(last.orientation, to) / dt, alpha(SPEED_CUTOFF, dt));
    let moved = alpha(rest_cutoff + POSITION_BETA * speed, dt);
    let turned = alpha(rest_cutoff + TURN_BETA * turn, dt);
    Filtered {
        at,
        position: [0, 1, 2].map(|k| low_pass(last.position[k], pose.position[k], moved)),
        orientation: quat_unit([0, 1, 2, 3].map(|k| low_pass(last.orientation[k], to[k], turned))),
        speed,
        turn,
    }
}

/// `pose` moved with its controller from where `base` was located to where it was filtered to.
fn carried(base: HandPose, pose: HandPose, filtered: Filtered) -> HandPose {
    let back = conjugate(base.orientation);
    let offset = math::rotate(back, [0, 1, 2].map(|k| pose.position[k] - base.position[k]));
    let turned = math::rotate(filtered.orientation, offset);
    HandPose {
        orientation: quat_unit(multiply(filtered.orientation, multiply(back, pose.orientation))),
        position: [0, 1, 2].map(|k| filtered.position[k] + turned[k]),
        ..pose
    }
}

/// The smoothing factor of a low-pass at `cutoff` Hz over `dt` seconds.
fn alpha(cutoff: f32, dt: f32) -> f32 {
    let tau = 1.0 / (std::f32::consts::TAU * cutoff);
    1.0 / (1.0 + tau / dt)
}

fn low_pass(from: f32, to: f32, alpha: f32) -> f32 {
    from + (to - from) * alpha
}

#[cfg(test)]
mod tests;
