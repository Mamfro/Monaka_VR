//! How long each stage of the same-frame loop's frames takes ([`Timings`]): a frame's scene calls,
//! each eye's present dispatched, and the spacing of the pairs, averaged between reports.

use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::{Duration, Instant};

/// A stage of a frame on the same-frame loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// A scene call drawing no eye (a menu, a frame without a pair).
    MonoScene,
    /// The left eye's scene call.
    LeftScene,
    /// The left eye's present, from its request until it is dispatched.
    LeftDrain,
    /// The right eye's scene call.
    RightScene,
    /// The right eye's present, from its request until it is dispatched.
    RightDrain,
    /// From one pair finished to the next.
    FrameInterval,
    /// OpenGL: the frame copied out at `SwapBuffers` (with frame warping the eye and its depth;
    /// the interop lock included).
    GlColour,
    /// OpenGL: the eye upscaled (FSR; the warp thread).
    GlUpscale,
    /// OpenGL: the frame captured for the warp, the pair warped and published (the warp thread).
    GlWarp,
    /// OpenGL: all of the loop's work at `SwapBuffers`.
    GlPresent,
    /// The game's own `SwapBuffers`, after the loop's work.
    GameSwap,
    /// The game's render thread from one `SwapBuffers` returning to the next one called.
    GameRender,
    /// The render thread's CPU time from one `SwapBuffers` to the next (the loop's and the game's).
    RenderCpu,
}

impl Stage {
    pub const ALL: [Stage; 13] = [
        Stage::MonoScene,
        Stage::LeftScene,
        Stage::LeftDrain,
        Stage::RightScene,
        Stage::RightDrain,
        Stage::FrameInterval,
        Stage::GlColour,
        Stage::GlUpscale,
        Stage::GlWarp,
        Stage::GlPresent,
        Stage::GameSwap,
        Stage::GameRender,
        Stage::RenderCpu,
    ];

    /// Its name in the report.
    pub const fn name(self) -> &'static str {
        match self {
            Stage::MonoScene => "mono scene",
            Stage::LeftScene => "left scene",
            Stage::LeftDrain => "left drain",
            Stage::RightScene => "right scene",
            Stage::RightDrain => "right drain",
            Stage::FrameInterval => "frame interval",
            Stage::GlColour => "GL copy out",
            Stage::GlUpscale => "FSR",
            Stage::GlWarp => "warp and publish",
            Stage::GlPresent => "Monaka at SwapBuffers",
            Stage::GameSwap => "game's SwapBuffers",
            Stage::GameRender => "game's render thread between swaps",
            Stage::RenderCpu => "render thread CPU per frame",
        }
    }
}

/// The run's clock for the pair spacing.
static START: LazyLock<Instant> = LazyLock::new(Instant::now);

/// Each stage's time and count since the last report, and when the last pair finished.
pub struct Timings {
    nanos: [AtomicU64; Stage::ALL.len()],
    counts: [AtomicU64; Stage::ALL.len()],
    /// When the last pair finished (nanoseconds on the run's clock; 0: none yet).
    last_pair: AtomicU64,
}

impl Timings {
    pub const fn new() -> Self {
        Self { nanos: [const { AtomicU64::new(0) }; Stage::ALL.len()], counts: [const { AtomicU64::new(0) }; Stage::ALL.len()], last_pair: AtomicU64::new(0) }
    }

    /// `stage` took `took`.
    pub fn time(&self, stage: Stage, took: Duration) {
        self.nanos[stage as usize].fetch_add(took.as_nanos() as u64, Relaxed);
        self.counts[stage as usize].fetch_add(1, Relaxed);
    }

    /// A pair finished: the time since the last one.
    pub fn pair_done(&self) {
        // At least 1: 0 is "none yet" (the clock may start at this very call).
        let now = (START.elapsed().as_nanos() as u64).max(1);
        let last = self.last_pair.swap(now, Relaxed);
        if last != 0 {
            self.time(Stage::FrameInterval, Duration::from_nanos(now.saturating_sub(last)));
        }
    }

    /// The average of each stage timed since the last report (each started afresh):
    /// `timing (average ms): left scene 4.20 (x600); ..`; empty when none was.
    pub fn report(&self) -> String {
        let mut stages = String::new();
        for stage in Stage::ALL {
            let count = self.counts[stage as usize].swap(0, Relaxed);
            let nanos = self.nanos[stage as usize].swap(0, Relaxed);
            if count > 0 {
                stages += &format!(" {} {:.2} (x{count});", stage.name(), nanos as f64 / count as f64 / 1e6);
            }
        }
        if stages.is_empty() { stages } else { format!("timing (average ms):{stages}") }
    }
}

impl Default for Timings {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
