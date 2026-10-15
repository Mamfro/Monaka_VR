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
mod tests;
