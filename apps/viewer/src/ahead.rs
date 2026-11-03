//! How far past a frame's display time the head pose sent to the game is predicted. The pose a pair
//! is drawn with is predicted for about when that pair will be shown, so the compositor (which
//! still turns each pair to the head's pose at its display: the pair's own pose is on its layer)
//! has little left to correct, and a fast turn shows little past the image's edge.
//!
//! The whole delay is measured: each head sent is kept with the display time of the frame it went
//! out in, and a pair drawn with it (its pose record's orientation the same, bit for bit) is first
//! shown at this frame's display time; the difference covers the game's own work (its threads
//! drawing and presenting the frame) as well as the pair's way to the headset. A game that hands
//! back a pose of its own (none matches) is predicted by the pairs' age alone, which leaves the
//! game's own work out.

use std::collections::VecDeque;

/// The most predicted ahead by the pairs' age, in display periods: a guess further out wobbles.
const MAX_PERIODS: i64 = 3;
/// The most predicted ahead by the measured delay (ns).
const MAX_AHEAD_NS: i64 = 100_000_000;
/// How much each new measurement moves the estimate (about the last 30 count).
const EASE: f64 = 1.0 / 32.0;
/// The heads kept to match pairs against (at 120 Hz, about half a second).
const SENT_KEPT: usize = 64;
/// Matched pairs between the lines saying how far ahead the head is predicted.
const SAY_EVERY: u64 = 1200;

#[derive(Default)]
pub struct Ahead {
    /// The pairs' age when read (ms), eased; none before the first pair.
    age_ms: Option<f64>,
    /// The heads sent, oldest first: each one's orientation and the display time (ns) of the frame
    /// it was sent in.
    sent: VecDeque<([f32; 4], i64)>,
    /// The whole delay (ns), eased: from the frame a head was sent in to the one the pair drawn
    /// with it is first shown in; none until a pair matched.
    whole_ns: Option<f64>,
    matched: u64,
}

fn ease(was: Option<f64>, now: f64) -> f64 {
    match was {
        Some(was) => was + (now - was) * EASE,
        None => now,
    }
}

impl Ahead {
    /// A pair was read `age_ms` after it was published.
    pub fn pair(&mut self, age_ms: u64) {
        self.age_ms = Some(ease(self.age_ms, age_ms as f64));
    }

    /// A head of `orientation` went to the game in the frame shown at `display_time` (ns).
    pub fn sent(&mut self, orientation: [f32; 4], display_time: i64) {
        if self.sent.len() == SENT_KEPT {
            self.sent.pop_front();
        }
        self.sent.push_back((orientation, display_time));
    }

    /// A pair drawn with a head of `orientation` is first shown in the frame at `display_time` (ns).
    /// Whether it matched a head sent; a line to log now and then once it does.
    pub fn shown(&mut self, orientation: [f32; 4], display_time: i64) -> Option<String> {
        let at = self.sent.iter().rposition(|(sent, _)| *sent == orientation)?;
        let whole = display_time - self.sent[at].1;
        // Heads sent before this one will not be drawn now.
        self.sent.drain(..=at);
        if whole < 0 {
            return None;
        }
        self.whole_ns = Some(ease(self.whole_ns, whole as f64));
        self.matched += 1;
        (self.matched == 1 || self.matched.is_multiple_of(SAY_EVERY)).then(|| {
            let whole_ms = self.whole_ns.unwrap_or(0.0) / 1e6;
            let capped = if whole_ms * 1e6 > MAX_AHEAD_NS as f64 { format!(", at most {} ms", MAX_AHEAD_NS / 1_000_000) } else { String::new() };
            format!("Head predicted {whole_ms:.0} ms ahead: from the head sent to its pair shown{capped}")
        })
    }

    /// The time (ns) the head pose sent now is predicted for: `display_time` plus the whole delay
    /// measured (at most [`MAX_AHEAD_NS`]); before any pair matched, plus the pairs' age (at most
    /// [`MAX_PERIODS`] display periods of `period`); `display_time` itself before the first pair.
    pub fn time(&self, display_time: i64, period: i64) -> i64 {
        if let Some(whole) = self.whole_ns {
            return display_time + (whole as i64).clamp(0, MAX_AHEAD_NS);
        }
        let Some(age) = self.age_ms else { return display_time };
        display_time + ((age * 1e6) as i64).clamp(0, MAX_PERIODS * period.max(0))
    }
}

#[cfg(test)]
mod tests;
