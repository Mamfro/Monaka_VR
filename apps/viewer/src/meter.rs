//! The run measured for the Stats panel ([`monaka_overlay::stats`]): the frames shown (the
//! headset's or the 2D window's), the pairs among them and which game frame each of their eyes
//! came from, and the poses sent back, counted over stretches of a quarter second; the game's side
//! (its frames, what Monaka costs it) comes from its producer.

use monaka_core::debug_draw::GameReport;
use monaka_overlay::stats::{EyeAges, GameView, PoseFlow, StatsView};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

#[cfg(test)]
mod tests;

/// How long each stretch is.
const STRETCH: Duration = Duration::from_millis(250);
/// Frame times kept for each graph: a few seconds of a headset's frames.
const GRAPHED: usize = 400;

/// What one stretch counted.
#[derive(Clone, Copy, Debug, Default)]
struct Counts {
    frames: u32,
    frame_total_ms: f64,
    worst_frame_ms: f64,
    pairs: u32,
    age_total_ms: u64,
    worst_age_ms: u64,
    repeated: u32,
    skipped: u64,
    head: u32,
    hands: u32,
    fingers: u32,
    gaze: u32,
    /// Pairs whose eyes' frames are named, each eye's count of being the newer and its total age
    /// behind (frames), and how many were from one frame.
    named: u32,
    newest: [u32; 2],
    behind: [u64; 2],
    together: u32,
}

pub struct Meter {
    since: Instant,
    last_frame: Option<Instant>,
    last_sequence: Option<u64>,
    counts: Counts,
    tracked: PoseFlow,
    /// The latest frame times: the display's, and the game's (with the count of the game's frames
    /// taken so far).
    display_times: VecDeque<f32>,
    game_times: VecDeque<f32>,
    game_frames: u64,
    view: StatsView,
}

impl Meter {
    /// A meter for pairs of `eye_size` shown by `display` ("Headset", "Window").
    pub fn new(display: &'static str, eye_size: [u32; 2], now: Instant) -> Self {
        Self {
            since: now,
            last_frame: None,
            last_sequence: None,
            counts: Counts::default(),
            tracked: PoseFlow::default(),
            display_times: VecDeque::with_capacity(GRAPHED),
            game_times: VecDeque::with_capacity(GRAPHED),
            game_frames: 0,
            view: StatsView { display, eye_size, ..StatsView::default() },
        }
    }

    /// A frame shown at `now`, with the pair first shown in it (its sequence and publish tick) and
    /// the tick now, or none (the last pair shown again).
    pub fn frame(&mut self, now: Instant, new_pair: Option<(u64, u64)>, tick: u64) {
        if let Some(last) = self.last_frame.replace(now) {
            let ms = now.duration_since(last).as_secs_f64() * 1000.0;
            self.counts.frames += 1;
            self.counts.frame_total_ms += ms;
            self.counts.worst_frame_ms = self.counts.worst_frame_ms.max(ms);
            keep(&mut self.display_times, ms as f32);
        }
        let Some((sequence, published)) = new_pair else {
            self.counts.repeated += 1;
            return;
        };
        self.counts.pairs += 1;
        let age = tick.saturating_sub(published);
        self.counts.age_total_ms += age;
        self.counts.worst_age_ms = self.counts.worst_age_ms.max(age);
        if let Some(last) = self.last_sequence.replace(sequence) {
            self.counts.skipped += sequence.saturating_sub(last + 1);
        }
    }

    /// The game frames a new pair's eyes were drawn in (left, right), where its publisher named
    /// them.
    pub fn eyes(&mut self, keys: [u64; 2]) {
        let newer = keys[0].max(keys[1]);
        let c = &mut self.counts;
        c.named += 1;
        for ((behind, newest), key) in c.behind.iter_mut().zip(&mut c.newest).zip(keys) {
            *behind += newer - key;
            *newest += u32::from(key == newer);
        }
        c.together += u32::from(keys[0] == keys[1]);
    }

    /// A head pose sent (`tracked`: the headset located it).
    pub fn head(&mut self, tracked: bool) {
        self.counts.head += 1;
        self.tracked.head_tracked = tracked;
    }

    /// Controller poses sent, each side tracked or not.
    pub fn hands(&mut self, tracked: [bool; 2]) {
        self.counts.hands += 1;
        self.tracked.controllers = tracked;
    }

    /// Finger poses sent, each side tracked or not.
    pub fn fingers(&mut self, tracked: [bool; 2]) {
        self.counts.fingers += 1;
        self.tracked.fingers_tracked = tracked;
    }

    /// An eye gaze sent with the head pose.
    pub fn gaze(&mut self) {
        self.counts.gaze += 1;
    }

    /// The panel's numbers as of `now`: the last whole stretch's, with the game's side as its
    /// producer says it.
    pub fn view(&mut self, now: Instant, game: Option<GameReport>) -> &StatsView {
        let elapsed = now.duration_since(self.since);
        if elapsed >= STRETCH {
            let seconds = elapsed.as_secs_f64();
            let c = std::mem::take(&mut self.counts);
            let rate = |n: f64| (n / seconds) as f32;
            let view = &mut self.view;
            view.display_fps = rate(f64::from(c.frames));
            view.display_ms = if c.frames > 0 { (c.frame_total_ms / f64::from(c.frames)) as f32 } else { 0.0 };
            view.worst_display_ms = c.worst_frame_ms as f32;
            view.display_times = self.display_times.iter().copied().collect();
            view.pairs_per_second = rate(f64::from(c.pairs));
            view.pair_age_ms = if c.pairs > 0 { c.age_total_ms as f32 / c.pairs as f32 } else { 0.0 };
            view.worst_pair_age_ms = c.worst_age_ms as f32;
            view.repeated_per_second = rate(f64::from(c.repeated));
            view.skipped_per_second = rate(c.skipped as f64);
            view.eyes = (c.named > 0).then(|| {
                let share = |n: u32| n as f32 / c.named as f32;
                EyeAges { newest: c.newest.map(share), age: c.behind.map(|b| b as f32 / c.named as f32), together: share(c.together) }
            });
            view.poses = PoseFlow { head: rate(f64::from(c.head)), hands: rate(f64::from(c.hands)), fingers: rate(f64::from(c.fingers)), gaze: rate(f64::from(c.gaze)), ..self.tracked };
            self.since = now;
        }
        // Timing is written only by a game presenting through Monaka's driver (or telling it from
        // its own loop); the settings by all.
        let game = game.filter(|g| g.tick != 0 || g.text_len != 0);
        if let Some(g) = &game {
            // A new run's frames start from 0 again.
            if g.frames < self.game_frames {
                self.game_frames = 0;
                self.game_times.clear();
            }
            let (times, count) = g.frames_since(self.game_frames);
            self.game_frames = count;
            for time in times {
                keep(&mut self.game_times, time);
            }
        }
        self.view.game = game.map(|g| GameView {
            frames: (g.tick != 0).then_some([g.fps, g.frame_ms, g.worst_frame_ms]),
            times: self.game_times.iter().copied().collect(),
            costs: g.costs,
            size: [g.width, g.height],
            settings: g.text().to_owned(),
        });
        &self.view
    }
}

/// Adds a frame time to a graph's, the oldest let go past [`GRAPHED`].
fn keep(times: &mut VecDeque<f32>, ms: f32) {
    if times.len() == GRAPHED {
        times.pop_front();
    }
    times.push_back(ms);
}
