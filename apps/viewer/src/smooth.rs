//! Controller smoothing (`--controller-smoothing`, 0 off to 10): steadies the small shakes in the
//! controllers' tracking before any game reads them, without slowing real movement. Each hand's
//! grip pose goes through a One Euro filter (Casiez, Roussel and Vogel): a low-pass
//! whose cutoff rises with the hand's speed, so a hand held still is steadied hard while a swing or
//! a quick aim passes almost as tracked. Position and orientation are filtered apart, each by its
//! own speed. The aim pose keeps its offset from the grip (both come from one tracked controller),
//! so the two never drift apart.

use monaka_core::math::{self, Quat, Vec3};
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
        orientation: normalized([0, 1, 2, 3].map(|k| low_pass(last.orientation[k], to[k], turned))),
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
        orientation: normalized(multiply(filtered.orientation, multiply(back, pose.orientation))),
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

/// `q`, or its negation (the same rotation) on `reference`'s side, so blending takes the short way.
fn same_side(reference: Quat, q: Quat) -> Quat {
    if dot(reference, q) < 0.0 { q.map(|v| -v) } else { q }
}

/// The angle (radians) turning `a` into `b`.
fn angle_between(a: Quat, b: Quat) -> f32 {
    2.0 * dot(a, b).abs().min(1.0).acos()
}

fn dot(a: Quat, b: Quat) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3]
}

fn normalized(q: Quat) -> Quat {
    let length = dot(q, q).sqrt();
    if length > 0.0 { q.map(|v| v / length) } else { math::IDENTITY }
}

fn conjugate(q: Quat) -> Quat {
    [-q[0], -q[1], -q[2], q[3]]
}

/// The rotation `b` followed by `a` (xyzw).
fn multiply(a: Quat, b: Quat) -> Quat {
    let [ax, ay, az, aw] = a;
    let [bx, by, bz, bw] = b;
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: i64 = 11_111_111;

    fn at(position: Vec3, orientation: Quat) -> HandPose {
        HandPose { orientation, position, valid: 1, received_tick: 0 }
    }

    fn about_y(angle: f32) -> Quat {
        [0.0, (angle * 0.5).sin(), 0.0, (angle * 0.5).cos()]
    }

    #[test]
    fn off_passes_poses_through() {
        let mut off = Smoother::new(0.0);
        let pose = at([0.1, 1.0, -0.3], about_y(0.2));
        for frame in 0..3 {
            assert_eq!(off.smooth(1, frame * FRAME, pose, pose), (pose, pose));
        }
    }

    #[test]
    fn jitter_at_rest_is_steadied() {
        let mut smoother = Smoother::new(3.0);
        let mut worst = 0.0f32;
        for frame in 0..200 {
            // Half a millimetre and a tenth of a degree of shake about a hand held still.
            let shake = if frame % 2 == 0 { 0.0005 } else { -0.0005 };
            let raw = at([shake, 1.0, -0.3], about_y(shake * 3.5));
            let (_, grip) = smoother.smooth(0, frame * FRAME, raw, raw);
            if frame > 100 {
                worst = worst.max(grip.position[0].abs());
            }
        }
        assert!(worst < 0.0002, "shake left: {worst}");
    }

    #[test]
    fn a_swing_keeps_up() {
        let mut smoother = Smoother::new(3.0);
        let mut lag = 0.0f32;
        for frame in 0..60 {
            // 3 m/s across, after a moment held still.
            let x = if frame < 20 { 0.0 } else { (frame - 20) as f32 * 3.0 / 90.0 };
            let raw = at([x, 1.0, -0.3], math::IDENTITY);
            let (_, grip) = smoother.smooth(1, frame * FRAME, raw, raw);
            if frame > 30 {
                lag = lag.max(x - grip.position[0]);
            }
        }
        // Under 2.5 cm behind at 3 m/s: under 10 ms.
        assert!(lag < 0.025, "lag {lag}");
    }

    #[test]
    fn the_aim_keeps_its_offset_from_the_grip() {
        let mut smoother = Smoother::new(10.0);
        for frame in 0..30 {
            let turn = about_y(frame as f32 * 0.05);
            let grip = at([0.0, 1.0, -0.3 - frame as f32 * 0.01], turn);
            let offset = [0.0, 0.02, -0.05];
            let aim_at = math::rotate(turn, offset);
            let aim = at([0, 1, 2].map(|k| grip.position[k] + aim_at[k]), multiply(turn, about_y(0.7)));
            let (aim_out, grip_out) = smoother.smooth(0, frame * FRAME, aim, grip);
            let back = math::rotate(conjugate(grip_out.orientation), [0, 1, 2].map(|k| aim_out.position[k] - grip_out.position[k]));
            assert!(math::distance(back, offset) < 1e-4, "offset {back:?}");
            assert!(angle_between(multiply(conjugate(grip_out.orientation), aim_out.orientation), about_y(0.7)) < 1e-3);
        }
    }

    #[test]
    fn a_gap_or_a_lost_controller_starts_afresh() {
        let mut smoother = Smoother::new(10.0);
        let here = at([0.0, 1.0, 0.0], math::IDENTITY);
        let there = at([1.0, 1.0, 0.0], math::IDENTITY);
        smoother.smooth(0, 0, here, here);
        assert_eq!(smoother.smooth(0, 1_000_000_000, there, there).1, there, "a second later");
        let lost = HandPose::default();
        assert_eq!(smoother.smooth(0, 1_000_000_000 + FRAME, lost, lost), (lost, lost));
        assert_eq!(smoother.smooth(0, 1_000_000_000 + 2 * FRAME, here, here).1, here);
    }
}
