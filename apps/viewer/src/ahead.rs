//! How far past a frame's display time the head pose sent to the game is predicted. A pair is
//! shown at least its age after it was published, so the pose it is drawn with is predicted for
//! about when it will be shown. The compositor still turns each pair to the head's pose at its
//! display (the pair's own pose is on its layer), so this only shrinks that correction.

/// The most predicted ahead, in display periods: a guess further out wobbles.
const MAX_PERIODS: i64 = 3;
/// How much each new pair's age moves the estimate (about the last 30 pairs count).
const EASE: f64 = 1.0 / 32.0;

#[derive(Default)]
pub struct Ahead {
    /// The pairs' age when read (ms), eased; none before the first pair.
    age_ms: Option<f64>,
}

impl Ahead {
    /// A pair was read `age_ms` after it was published.
    pub fn pair(&mut self, age_ms: u64) {
        let age = age_ms as f64;
        self.age_ms = Some(match self.age_ms {
            Some(was) => was + (age - was) * EASE,
            None => age,
        });
    }

    /// The time (ns) the head pose sent now is predicted for: `display_time` plus the pairs' age,
    /// at most [`MAX_PERIODS`] display periods of `period` (ns); `display_time` itself before the
    /// first pair.
    pub fn time(&self, display_time: i64, period: i64) -> i64 {
        let Some(age) = self.age_ms else { return display_time };
        display_time + ((age * 1e6) as i64).clamp(0, MAX_PERIODS * period.max(0))
    }
}

#[cfg(test)]
mod tests;
