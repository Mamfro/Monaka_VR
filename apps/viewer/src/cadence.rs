//! How the pairs meet the headset's refreshes, said every [`EVERY`]: how many refreshes each pair
//! was shown for (a game paced to half the refresh shows each for two; a one or a three is a pair
//! that came late or early), and how far the compositor must turn each pair to the head (the
//! angle between the pair's pose and the head's for this refresh): even turns from one refresh to
//! the next are a smooth view, a jump between them is a stutter however well the pairs are drawn.

use std::time::{Duration, Instant};

const EVERY: Duration = Duration::from_secs(10);

pub struct Cadence {
    /// The pair shown now and for how many refreshes so far.
    current: Option<u64>,
    shown: u32,
    /// Pairs shown for one, two, three, four or more refreshes.
    held: [u64; 4],
    refreshes: u64,
    angle_sum: f64,
    angle_max: f32,
    /// The turn the refresh before needed, and the most the turn changed between two refreshes.
    last_angle: Option<f32>,
    change_max: f32,
    /// The most the viewer's own work took in a refresh (from its frame wait to its hand-in).
    work_max: Duration,
    since: Instant,
}

/// The angle (degrees) between two unit quaternions (xyzw).
pub fn angle_between(a: [f32; 4], b: [f32; 4]) -> f32 {
    let dot = (0..4).map(|i| a[i] * b[i]).sum::<f32>().abs().min(1.0);
    2.0 * dot.acos().to_degrees()
}

impl Cadence {
    pub fn new(now: Instant) -> Self {
        Self { current: None, shown: 0, held: [0; 4], refreshes: 0, angle_sum: 0.0, angle_max: 0.0, last_angle: None, change_max: 0.0, work_max: Duration::ZERO, since: now }
    }

    /// Each refresh: the pair shown (its sequence and pose's orientation) and the head's for it;
    /// none shown (a flat screen) ends the pair.
    pub fn refresh(&mut self, shown: Option<(u64, [f32; 4])>, head: Option<[f32; 4]>, now: Instant) {
        match shown {
            Some((sequence, _)) if self.current == Some(sequence) => self.shown += 1,
            _ => {
                self.end_pair();
                self.current = shown.map(|(sequence, _)| sequence);
                self.shown = u32::from(shown.is_some());
            }
        }
        if let (Some((_, pair)), Some(head)) = (shown, head) {
            let angle = angle_between(pair, head);
            self.refreshes += 1;
            self.angle_sum += f64::from(angle);
            self.angle_max = self.angle_max.max(angle);
            if let Some(last) = self.last_angle {
                self.change_max = self.change_max.max((angle - last).abs());
            }
            self.last_angle = Some(angle);
        } else {
            self.last_angle = None;
        }
        if now.duration_since(self.since) >= EVERY {
            if let Some(line) = self.line() {
                println!("{line}");
            }
            *self = Self { current: self.current, shown: self.shown, ..Self::new(now) };
        }
    }

    /// The viewer's own work this refresh took, from its frame wait to its hand-in.
    pub fn work(&mut self, took: Duration) {
        self.work_max = self.work_max.max(took);
    }

    fn end_pair(&mut self) {
        if self.shown > 0 {
            self.held[(self.shown as usize - 1).min(3)] += 1;
        }
    }

    /// The tally so far, if a pair was shown.
    pub fn line(&self) -> Option<String> {
        if self.refreshes == 0 {
            return None;
        }
        let [one, two, three, more] = self.held;
        Some(format!(
            "Pairs at the headset: shown for 1 refresh {one}, 2 {two}, 3 {three}, 4+ {more}; turned to the head {:.2} degrees on average, at most {:.2}, the turn changing at most {:.2} between refreshes; the viewer's own work at most {:.2} ms",
            self.angle_sum / self.refreshes as f64,
            self.angle_max,
            self.change_max,
            self.work_max.as_secs_f64() * 1000.0
        ))
    }
}

#[cfg(test)]
mod tests;
