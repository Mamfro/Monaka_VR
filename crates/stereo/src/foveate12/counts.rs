//! The counts: what was shaded at which rate, the hooks' calls and the viewport sizes seen, for the
//! periodic report and the stop's line.

use super::*;

pub(super) struct Tally {
    /// Frames VRS was set in, and of them those placed by a live gaze.
    pub(super) frames: AtomicU64,
    pub(super) gaze_frames: AtomicU64,
    /// Bindings at the scene's size, told by their viewport (0) and by their targets' size (1).
    pub(super) passes: [AtomicU64; 2],
    /// Image writes.
    pub(super) writes: AtomicU64,
    /// Frames whose eye came from their label before their first scene pass, frames whose eye was
    /// foreseen, and of those, the label later disagreeing (0) or agreeing (1).
    pub(super) from_label: AtomicU64,
    pub(super) predicted: AtomicU64,
    pub(super) predicted_right: [AtomicU64; 2],
}

impl Tally {
    pub(super) fn all(&self) -> [&AtomicU64; 9] {
        [
            &self.frames,
            &self.gaze_frames,
            &self.passes[0],
            &self.passes[1],
            &self.writes,
            &self.from_label,
            &self.predicted,
            &self.predicted_right[0],
            &self.predicted_right[1],
        ]
    }
}

pub(super) static TALLY: Tally = Tally {
    frames: AtomicU64::new(0),
    gaze_frames: AtomicU64::new(0),
    passes: [AtomicU64::new(0), AtomicU64::new(0)],
    writes: AtomicU64::new(0),
    from_label: AtomicU64::new(0),
    predicted: AtomicU64::new(0),
    predicted_right: [AtomicU64::new(0), AtomicU64::new(0)],
};

/// The counts as a line.
pub(super) fn counts() -> String {
    let get = |c: &AtomicU64| c.load(Relaxed);
    let frames = get(&TALLY.frames);
    let gaze = get(&TALLY.gaze_frames);
    format!(
        "VRS in {frames} frames ({gaze} at the gaze, {} straight ahead); scene-size bindings by target size {}, by viewport {}; image writes {}; \
         the frame's eye from its label {}, foreseen {} (label agreed {}, disagreed {})",
        frames - gaze,
        get(&TALLY.passes[1]),
        get(&TALLY.passes[0]),
        get(&TALLY.writes),
        get(&TALLY.from_label),
        get(&TALLY.predicted),
        get(&TALLY.predicted_right[1]),
        get(&TALLY.predicted_right[0]),
    )
}

/// The game's calls foveation sees: `OMSetRenderTargets`, `RSSetViewports`, `BeginRenderPass`,
/// `Reset` (on its command lists) and `CreateRenderTargetView` (on its device).
pub(super) static CALLS: [AtomicU64; 5] = [const { AtomicU64::new(0) }; 5];
pub(super) const C_TARGETS: usize = 0;
pub(super) const C_VIEWPORTS: usize = 1;
pub(super) const C_PASSES: usize = 2;
pub(super) const C_RESETS: usize = 3;
pub(super) const C_VIEWS: usize = 4;

/// The calls as a line.
pub(super) fn calls() -> String {
    let get = |at: usize| CALLS[at].load(Relaxed);
    format!(
        "calls: render targets {}, viewports {}, render passes {}, resets {}, views made {}",
        get(C_TARGETS),
        get(C_VIEWPORTS),
        get(C_PASSES),
        get(C_RESETS),
        get(C_VIEWS)
    )
}

/// The viewport sizes set since the last report, each with how often (the first sizes seen).
pub(super) static VIEWPORT_SIZES: Mutex<Vec<((u32, u32), u64)>> = Mutex::new(Vec::new());
pub(super) const VIEWPORT_SIZES_KEPT: usize = 64;

pub(super) fn note_viewport(size: (u32, u32)) {
    let mut sizes = VIEWPORT_SIZES.locked();
    if let Some((_, count)) = sizes.iter_mut().find(|(s, _)| *s == size) {
        *count += 1;
    } else if sizes.len() < VIEWPORT_SIZES_KEPT {
        sizes.push((size, 1));
    }
}

/// The viewport size set most often since the last report, as text; the count starts afresh.
pub(super) fn most_frequent_viewport() -> String {
    let sizes = std::mem::take(&mut *VIEWPORT_SIZES.locked());
    let total: u64 = sizes.iter().map(|(_, count)| count).sum();
    match sizes.iter().max_by_key(|(_, count)| *count) {
        Some(((w, h), count)) => format!("{w}x{h} ({count} of {total} since the last report)"),
        None => "none since the last report".to_owned(),
    }
}

/// The counts, logged when any changed: the game's calls, the viewport set most often and the
/// scene's size, then what foveation did.
pub(super) fn report(_: f64) {
    static LAST: [AtomicU64; 7] = [const { AtomicU64::new(u64::MAX) }; 7];
    let get = |c: &AtomicU64| c.load(Relaxed);
    let now = [get(&CALLS[0]), get(&CALLS[1]), get(&CALLS[2]), get(&CALLS[3]), get(&CALLS[4]), get(&TALLY.passes[0]), get(&TALLY.passes[1])];
    let changed = now.iter().zip(&LAST).fold(false, |changed, (now, last)| last.swap(*now, Relaxed) != *now || changed);
    let viewport = most_frequent_viewport();
    if changed {
        let scene = match SCENE.load(Relaxed) {
            0 => "unknown".to_owned(),
            packed => format!("{}x{}", packed >> 32, packed & u32::MAX as u64),
        };
        log!("foveation: {}; most frequent viewport {viewport}; scene {scene}; {}", calls(), counts());
    }
}

pub(super) fn summary() -> String {
    format!("foveation: {}; {}", calls(), counts())
}
