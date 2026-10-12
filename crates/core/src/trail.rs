//! A hand's recent way: each new controller sample kept a short while, for what a hand does over
//! time rather than at one moment (a throw leaving the hand, a thrust, a draw, a reload gesture).
//! [`crate::gesture`] reads it; a producer feeds it where it reads the controllers, in whatever
//! space it acts in (tracking space for gestures, the game world for a throw's velocity).
//!
//! The trail follows one point fixed to the hand ([`Trail::of_point`]): the palm by default, or
//! the tip of what it holds (a sword's tip, `reach` along its blade). Times are seconds on any
//! steady clock, as [`crate::swing`] takes them.
//!
//! Speeds are read over a span of at least a few frames ([`Trail::DEFAULT_SPAN`]): one frame's
//! step is too short for tracking noise to average out, and a game that reads the controllers
//! twice in a frame gives two samples of one pose ([`Trail::SAME_UPDATE`]).

use crate::math::{IDENTITY, Quat, Vec3, dot, length, rotate};
use std::collections::VecDeque;

/// One sample: when, where the followed point was, and the hand's orientation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    pub time: f64,
    pub position: Vec3,
    pub orientation: Quat,
}

/// The followed point's motion over a window: its average speed (m/s), how far it moved (a vector)
/// and over how long (seconds).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Motion {
    pub speed: f32,
    pub moved: Vec3,
    pub seconds: f32,
}

impl Motion {
    /// How much of the motion went along `direction` (unit): 1 straight along it, 0 across, -1
    /// straight against. 0 for no motion.
    pub fn share_along(&self, direction: Vec3) -> f32 {
        let length = length(self.moved);
        if length < 1e-6 { 0.0 } else { dot(self.moved, direction) / length }
    }
}

/// The fastest moment in a window ([`Trail::fastest`]): the velocity there (m/s) and the sample it
/// was measured at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Peak {
    pub velocity: Vec3,
    pub time: f64,
    index: usize,
}

/// One hand's recent samples, oldest first.
#[derive(Clone, Debug)]
pub struct Trail {
    samples: VecDeque<Sample>,
    /// How long samples are kept (seconds).
    kept: f64,
    /// The followed point in the hand's frame (x right, y up, z back; metres).
    offset: Vec3,
}

impl Trail {
    /// Samples closer together than this (seconds) are one update's: the first is kept.
    pub const SAME_UPDATE: f64 = 0.002;
    /// A span speeds are read over when nothing else is asked for (seconds): a few game frames.
    pub const DEFAULT_SPAN: f64 = 0.06;

    /// A trail of the palm, keeping `kept` seconds.
    pub const fn new(kept: f64) -> Self {
        Self { samples: VecDeque::new(), kept, offset: [0.0; 3] }
    }

    /// A trail of the point `offset` in the hand's frame instead of the palm, keeping `kept`
    /// seconds: a held weapon's tip is `direction * reach`, as [`crate::swing::SwingRules`] places it.
    pub const fn of_point(kept: f64, offset: Vec3) -> Self {
        Self { samples: VecDeque::new(), kept, offset }
    }

    /// A new controller sample: the hand at `position` turned `orientation` at `time`. Ignored when
    /// not finite, when not newer than the latest sample, or within [`Trail::SAME_UPDATE`] of it.
    /// Samples older than the kept span go. Returns whether it was kept.
    pub fn push(&mut self, time: f64, position: Vec3, orientation: Quat) -> bool {
        let out = rotate(orientation, self.offset);
        let point = [0, 1, 2].map(|k| position[k] + out[k]);
        let finite = time.is_finite() && point.iter().chain(orientation.iter()).all(|v| v.is_finite());
        let newer = self.samples.back().is_none_or(|last| time - last.time > Self::SAME_UPDATE);
        let kept = finite && newer;
        if kept {
            self.samples.push_back(Sample { time, position: point, orientation });
        }
        if let Some(newest) = self.samples.back().map(|s| s.time) {
            self.expire(newest);
        }
        kept
    }

    /// A sample where only the followed point's place is known, not the hand's turn (a palm read
    /// back from the game's arms, a controller's place in the world): for a trail of the palm
    /// ([`Trail::new`]), whose speeds and strokes read places alone.
    pub fn push_position(&mut self, time: f64, position: Vec3) -> bool {
        self.push(time, position, IDENTITY)
    }

    /// Drops the samples more than the kept span before `now`: a trail not fed for a while (its
    /// controller lost) empties as time goes on, and its old way is not read as a recent one.
    pub fn expire(&mut self, now: f64) {
        while self.samples.front().is_some_and(|s| now - s.time > self.kept) {
            self.samples.pop_front();
        }
    }

    /// Forgets the samples (the controller went away: its next way starts afresh).
    pub fn clear(&mut self) {
        self.samples.clear();
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn samples(&self) -> impl DoubleEndedIterator<Item = &Sample> {
        self.samples.iter()
    }

    pub fn latest(&self) -> Option<&Sample> {
        self.samples.back()
    }

    /// The latest sample if it is at most `max_age` seconds before `now`.
    pub fn fresh(&self, now: f64, max_age: f64) -> Option<&Sample> {
        self.samples.back().filter(|s| now - s.time <= max_age)
    }

    /// The newest sample at or before `time`, else the oldest kept: where the hand was a moment
    /// ago (where a cut began, the guard a swing came out of).
    pub fn at(&self, time: f64) -> Option<&Sample> {
        self.samples.iter().rev().find(|s| s.time <= time).or(self.samples.front())
    }

    /// The velocity (m/s) at sample `index` (oldest first, as [`Trail::samples`]), read back over at
    /// least `span` seconds; `None` without a sample that long before it.
    pub fn velocity_at(&self, index: usize, span: f64) -> Option<Vec3> {
        let end = self.samples.get(index)?;
        let start = self.samples.iter().take(index).rev().find(|s| end.time - s.time >= span)?;
        let dt = (end.time - start.time) as f32;
        Some([0, 1, 2].map(|k| (end.position[k] - start.position[k]) / dt))
    }

    /// The point's velocity now (m/s), read over at least `span` seconds.
    pub fn velocity(&self, span: f64) -> Option<Vec3> {
        self.velocity_at(self.samples.len().checked_sub(1)?, span)
    }

    /// How the point moved over the last `window` seconds: from the newest sample at least that old
    /// to the newest. `None` until the trail is that long.
    pub fn motion(&self, window: f64) -> Option<Motion> {
        let end = self.samples.back()?;
        let start = self.samples.iter().rev().find(|s| end.time - s.time >= window)?;
        let moved = [0, 1, 2].map(|k| end.position[k] - start.position[k]);
        let seconds = (end.time - start.time) as f32;
        Some(Motion { speed: length(moved) / seconds.max(1e-3), moved, seconds })
    }

    /// The fastest velocity in the last `within` seconds, each read over `span`. An action's speed
    /// is its peak: by the time a button is let go the hand may already be slowing.
    pub fn fastest(&self, within: f64, span: f64) -> Option<Peak> {
        let newest = self.samples.back()?.time;
        let mut best: Option<Peak> = None;
        for index in (0..self.samples.len()).rev() {
            let time = self.samples[index].time;
            if newest - time > within {
                break;
            }
            // No sample a span before this one: none before the older ones either.
            let Some(velocity) = self.velocity_at(index, span) else { break };
            if best.is_none_or(|b| length(velocity) > length(b.velocity)) {
                best = Some(Peak { velocity, time, index });
            }
        }
        best
    }

    /// Which way the point travelled through its fastest moment in the last `within` seconds
    /// (unit): from `before` seconds before that moment to `after` seconds after it (the newest
    /// sample at most). Steadier than one span's velocity, and a flick that stopped before a button
    /// was let go keeps its way. `None` when it hardly moved (5 mm).
    pub fn stroke(&self, within: f64, span: f64, before: f64, after: f64) -> Option<Vec3> {
        let peak = self.fastest(within, span)?;
        let from = self.samples.iter().take(peak.index + 1).find(|s| peak.time - s.time <= before)?;
        let to = self.samples.iter().skip(peak.index).take_while(|s| s.time - peak.time <= after).last()?;
        let moved = [0, 1, 2].map(|k| to.position[k] - from.position[k]);
        let length = length(moved);
        (length > 0.005).then(|| moved.map(|c| c / length))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 0.02 s steps: 0.1 m each along x (5 m/s), then slowing to 0.02 m each (1 m/s) while
    /// drifting down along y.
    fn throw() -> Trail {
        let mut trail = Trail::new(1.0);
        let (mut x, mut y) = (0.0, 0.0);
        for i in 0..20 {
            if i < 10 {
                x += 0.1;
            } else {
                x += 0.02;
                y -= 0.05;
            }
            trail.push(0.02 * i as f64, [x, y, 0.0], IDENTITY);
        }
        trail
    }

    #[test]
    fn the_peak_is_the_fastest_stretch_and_the_stroke_its_way() {
        let trail = throw();
        let peak = trail.fastest(1.0, 0.06).unwrap();
        assert!((peak.velocity[0] - 5.0).abs() < 0.01 && peak.velocity[1].abs() < 0.01, "{peak:?}");
        let late = trail.fastest(0.1, 0.06).unwrap();
        assert!((late.velocity[0] - 1.0).abs() < 0.01, "only the slow end is recent: {late:?}");
        // Through the peak the hand went along x: the slow drift after it is not the throw.
        let way = trail.stroke(1.0, 0.06, 0.1, 0.04).unwrap();
        assert!(way[0] > 0.95, "{way:?}");
    }

    #[test]
    fn one_update_read_twice_is_one_sample_and_old_samples_go() {
        let mut trail = Trail::new(0.5);
        assert!(trail.push(1.0, [0.0; 3], IDENTITY));
        assert!(!trail.push(1.001, [0.1, 0.0, 0.0], IDENTITY), "the same update");
        assert!(!trail.push(0.9, [0.0; 3], IDENTITY), "older than the latest");
        assert!(!trail.push(1.1, [f32::NAN, 0.0, 0.0], IDENTITY));
        assert!(trail.push(1.6, [0.0; 3], IDENTITY));
        assert_eq!(trail.len(), 1, "the sample at 1.0 is past the kept half second");
        assert!(trail.fresh(1.65, 0.1).is_some() && trail.fresh(1.8, 0.1).is_none());
    }

    #[test]
    fn a_tip_follows_the_hand_s_turn() {
        // The tip half a metre ahead (-z) of a palm that turns a quarter about y (to face -x): the
        // palm stays put, the tip sweeps.
        let mut trail = Trail::of_point(1.0, [0.0, 0.0, -0.5]);
        let quarter = [0.0, std::f32::consts::FRAC_1_SQRT_2, 0.0, std::f32::consts::FRAC_1_SQRT_2];
        trail.push(0.0, [0.0; 3], IDENTITY);
        trail.push(0.1, [0.0; 3], quarter);
        let motion = trail.motion(0.1).unwrap();
        assert!((motion.moved[0] + 0.5).abs() < 1e-5 && (motion.moved[2] - 0.5).abs() < 1e-5, "{motion:?}");
        assert!((motion.speed - 0.5f32.hypot(0.5) / 0.1).abs() < 1e-3);
        assert!((motion.share_along([-1.0, 0.0, 0.0]) - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5);
    }

    #[test]
    fn a_trail_no_longer_fed_empties_as_time_goes_on() {
        let mut trail = Trail::new(0.5);
        for i in 0..5 {
            assert!(trail.push_position(0.1 * i as f64, [0.1 * i as f32, 0.0, 0.0]));
        }
        let speed = trail.velocity_at(4, 0.06).unwrap();
        assert!((speed[0] - 1.0).abs() < 1e-4, "{speed:?}");
        assert!(trail.velocity_at(0, 0.06).is_none(), "nothing before the first");
        trail.expire(0.75);
        assert_eq!(trail.len(), 2, "the samples at 0.3 and 0.4 are within half a second");
        trail.expire(2.0);
        assert!(trail.is_empty() && trail.fastest(1.0, 0.06).is_none());
    }

    #[test]
    fn where_the_hand_was_a_moment_ago() {
        let trail = throw();
        assert_eq!(trail.at(0.05).unwrap().time, 0.04);
        assert_eq!(trail.at(-1.0).unwrap().time, 0.0, "before the trail: its oldest");
        assert!(trail.motion(10.0).is_none(), "not that long yet");
    }
}
