//! Debug drawing from inside the game ([`monaka_contract::debug_draw`]): any code, a probe included,
//! draws with one plain call, in tracking space, for the game frame it is working on (`key`: the
//! number the game's loop counts frames by, the same one its publisher names the pair's eyes with,
//! [`eye_keys`]). The viewer draws each eye with the primitives of the frame it was drawn in.
//!
//! Nothing is kept unless the viewer's menu wants the category: [`on`] is one atomic load and a
//! clock read, and every drawing call starts with it. The framework opens the block at the run's
//! start and closes it at the stop.

use monaka_channel::ChannelName;
use monaka_channel::debug::{DebugChannel, Stretch};
use monaka_core::camera::Mat34;
use monaka_contract::debug_draw::{COSTS, Categories, Category, Cost, CostTime, DebugDepth, Primitive, axes as axes_of};
use monaka_core::sync::Locked;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::*};

pub use monaka_contract::debug_draw::{BLUE, CYAN, GREEN, MAGENTA, ORANGE, RED, WHITE, YELLOW, rgba};

/// How often the viewer's wishes are read again (ms).
const REFRESH_MS: u64 = 100;
/// Frames collected at once; a newer one commits the oldest.
const OPEN_FRAMES: usize = 3;

static CHANNEL: Mutex<Option<DebugChannel>> = Mutex::new(None);
/// What the viewer wants (a [`Categories`] set), and when it was last read (tick, 0 for closed).
static WANTED: AtomicU64 = AtomicU64::new(0);
/// The depth pass the viewer wants shared (`DebugDepth::pass`).
static WANTED_PASS: AtomicU32 = AtomicU32::new(0);
/// The depth buffer the viewer wants shared (0: the producer's pick).
static WANTED_BUFFER: AtomicU32 = AtomicU32::new(0);
/// The render target the viewer wants shared (0: the producer's pick).
static WANTED_TARGET: AtomicU32 = AtomicU32::new(0);
static READ_AT: AtomicU64 = AtomicU64::new(0);
/// What this run can draw or do, said to the viewer at each refresh.
static OFFERED: AtomicU64 = AtomicU64::new(Category::Probe.bit());
/// The key of the frame the game is drawing now, as its stereo loop counts frames (0: unknown).
static DRAWING: AtomicU64 = AtomicU64::new(0);
/// Frames being collected: (key, primitives), oldest first.
static OPEN: Mutex<Vec<(u64, Vec<Primitive>)>> = Mutex::new(Vec::new());

/// Opens the run's debug block (the framework does, at the start).
pub fn open(channel: &ChannelName) {
    match DebugChannel::open(channel) {
        Ok(opened) => {
            *CHANNEL.locked() = Some(opened);
            READ_AT.store(1, Release);
        }
        Err(e) => crate::log!("debug drawing: block unavailable ({e})"),
    }
}

/// Closes it (the framework does, at the stop).
pub fn close() {
    READ_AT.store(0, Release);
    DRAWING.store(0, Release);
    WANTED.store(0, Release);
    OPEN.locked().clear();
    *TIMING.locked() = None;
    *CHANNEL.locked() = None;
}

/// The stereo loop says which frame the game draws from now on (its key; 0: unknown).
pub fn set_drawing(key: u64) {
    DRAWING.store(key, Release);
}

/// The key of the frame the game is drawing now: code that draws for the current frame without a
/// key of its own (the hand rig, a probe) uses it. Where the stereo loop does not say, the clock
/// stands in (with the top bit set, so it never meets a loop's key): its pairs name no eyes, and
/// the viewer draws the newest frame on both.
pub fn drawing() -> u64 {
    match DRAWING.load(Acquire) {
        0 => monaka_channel::tick() | 1 << 63,
        key => key,
    }
}

/// Says this run can draw `category` (or do it, for an action), so the viewer's menu offers it.
pub fn offer(category: Category) {
    OFFERED.fetch_or(category.bit(), AcqRel);
}

/// Whether the viewer wants `category` drawn (or done).
pub fn on(category: Category) -> bool {
    let read_at = READ_AT.load(Acquire);
    if read_at == 0 {
        return false;
    }
    if monaka_channel::tick().saturating_sub(read_at) >= REFRESH_MS {
        refresh();
    }
    Categories(WANTED.load(Acquire)).has(category)
}

/// Reads what the viewer wants and says what this run offers; a busy block keeps the last answer.
fn refresh() {
    let Ok(mut channel) = CHANNEL.try_lock() else { return };
    let Some(channel) = channel.as_mut() else { return };
    READ_AT.store(monaka_channel::tick().max(1), Release);
    channel.offer(Categories(OFFERED.load(Acquire)));
    let wanted = channel.wanted();
    WANTED.store(wanted.categories.0, Release);
    WANTED_PASS.store(wanted.pass, Release);
    WANTED_BUFFER.store(wanted.buffer, Release);
    WANTED_TARGET.store(wanted.target, Release);
}

/// A segment from `a` to `b` (tracking space) in frame `key`.
pub fn line(key: u64, category: Category, a: [f32; 3], b: [f32; 3], colour: u32) {
    if on(category) {
        add(key, &[Primitive::segment(category, a, b, colour)]);
    }
}

/// A point at `at` (tracking space) in frame `key`.
pub fn point(key: u64, category: Category, at: [f32; 3], colour: u32) {
    if on(category) {
        add(key, &[Primitive::point(category, at, colour)]);
    }
}

/// The axes of `matrix` (tracking space, row-major 3x4, columns right, up, back, position) in
/// frame `key`, each `size` long: x red, y green, z blue.
pub fn axes(key: u64, category: Category, matrix: &Mat34, size: f32) {
    if on(category) {
        add(key, &axes_of(category, matrix, size));
    }
}

/// Primitives made by the caller, in frame `key`; the caller checked [`on`] for their categories.
pub fn add(key: u64, primitives: &[Primitive]) {
    let mut open = OPEN.locked();
    if let Some((_, frame)) = open.iter_mut().find(|(k, _)| *k == key) {
        frame.extend_from_slice(primitives);
        return;
    }
    if open.len() >= OPEN_FRAMES {
        let (oldest, frame) = open.remove(0);
        drop(open);
        write(oldest, &frame);
        open = OPEN.locked();
    }
    open.push((key, primitives.to_vec()));
}

/// Frame `key` is drawn: its primitives go to the viewer now.
pub fn commit(key: u64) {
    let frame = {
        let mut open = OPEN.locked();
        let Some(at) = open.iter().position(|(k, _)| *k == key) else { return };
        open.remove(at).1
    };
    write(key, &frame);
}

/// The eyes of published pair `sequence` were drawn in frames `keys` (left, right): the publisher
/// says so, and those frames and any older still open are committed.
pub fn eye_keys(sequence: u64, keys: [u64; 2]) {
    // Kept while anything is drawn, or for the Stats panel's eye ages.
    let wanted = Categories(WANTED.load(Acquire));
    if READ_AT.load(Acquire) == 0 || !(wanted.draws() || wanted.has(Category::Stats)) {
        return;
    }
    let newest = keys[0].max(keys[1]);
    let done: Vec<(u64, Vec<Primitive>)> = {
        let mut open = OPEN.locked();
        let (done, kept) = std::mem::take(&mut *open).into_iter().partition(|(k, _)| *k <= newest);
        *open = kept;
        done
    };
    let mut channel = CHANNEL.locked();
    let Some(channel) = channel.as_mut() else { return };
    for (key, frame) in &done {
        channel.write_frame(*key, frame);
    }
    channel.write_eye_keys(sequence, keys);
}

/// The depth pass the viewer wants shared: 1 the first of the frame, 0 what the depth holds at
/// the present.
pub fn wanted_pass() -> u32 {
    WANTED_PASS.load(Acquire)
}

/// The depth buffer the viewer wants shared: 0 the producer's own pick, else that candidate (1
/// the first, [`share_candidates`]).
pub fn wanted_buffer() -> u32 {
    WANTED_BUFFER.load(Acquire)
}

/// Lists the depth buffers the game drew with, and which is shared (1 the first, 0 none).
pub fn share_candidates(candidates: &[monaka_contract::debug_draw::DepthCandidate], chosen: u32) {
    if let Ok(mut channel) = CHANNEL.try_lock()
        && let Some(channel) = channel.as_mut()
    {
        channel.write_candidates(candidates, chosen);
    }
}

/// The render target the viewer wants shared: 0 the producer's own pick, else that one of its
/// list (1 the first, [`share_targets`]).
pub fn wanted_target() -> u32 {
    WANTED_TARGET.load(Acquire)
}

/// Lists the render targets the game drew into, and which is shared (1 the first, 0 none).
pub fn share_targets(targets: &[monaka_contract::debug_draw::DepthCandidate], chosen: u32) {
    if let Ok(mut channel) = CHANNEL.try_lock()
        && let Some(channel) = channel.as_mut()
    {
        channel.write_targets(targets, chosen);
    }
}

/// Says which render target the run shares now.
pub fn share_target(target: DebugDepth) {
    if let Ok(mut channel) = CHANNEL.try_lock()
        && let Some(channel) = channel.as_mut()
    {
        channel.write_target(target);
    }
}

/// Says which depth the run shares now ([`monaka_channel::debug_depth`]).
pub fn share_depth(depth: DebugDepth) {
    if let Ok(mut channel) = CHANNEL.try_lock()
        && let Some(channel) = channel.as_mut()
    {
        channel.write_depth(depth);
    }
}

/// How long the game's frames are measured before the Stats panel is told (ms).
const TIMING_MS: f64 = 250.0;
/// Frame times kept for the viewer while its block is busy, at most.
const KEPT_FRAMES: usize = 512;

/// The frames since the last write: when the stretch began, the last present, how many, their
/// total and longest time (ms), and each one's time not yet written.
struct Timing {
    since: std::time::Instant,
    last: std::time::Instant,
    frames: u32,
    total_ms: f64,
    worst_ms: f64,
    times: Vec<f32>,
}

impl Timing {
    fn new(now: std::time::Instant) -> Self {
        Self { since: now, last: now, frames: 0, total_ms: 0.0, worst_ms: 0.0, times: Vec::new() }
    }
}

static TIMING: Mutex<Option<Timing>> = Mutex::new(None);
/// Monaka's own work this stretch, by [`Cost`]: the total and the longest (ns).
static COST_NS: [AtomicU64; COSTS] = [const { AtomicU64::new(0) }; COSTS];
static COST_WORST_NS: [AtomicU64; COSTS] = [const { AtomicU64::new(0) }; COSTS];

/// Whether Monaka's own work is timed now: the viewer shows the Stats panel. One atomic load and a
/// clock read, as [`on`].
pub fn timing() -> bool {
    on(Category::Stats)
}

/// Monaka's own work at `cost` took `took`, for the Stats panel; callers time it only while
/// [`timing`] says so.
pub fn cost(cost: Cost, took: std::time::Duration) {
    let ns = took.as_nanos().min(u128::from(u64::MAX)) as u64;
    COST_NS[cost as usize].fetch_add(ns, Relaxed);
    COST_WORST_NS[cost as usize].fetch_max(ns, Relaxed);
}

/// One present of the game's frame, for the Stats panel's game timing: every quarter second the
/// frame rate, each frame's time and what Monaka's own work cost go to the viewer, with the size
/// `size` gives (the game's swapchain), asked for only then. Never waits: a busy block keeps the
/// frames for the next write.
pub fn note_present(size: impl FnOnce() -> [u32; 2]) {
    if READ_AT.load(Acquire) == 0 {
        return;
    }
    // Read here too: the viewer's wishes are refreshed even in a game that draws nothing.
    let timed = timing();
    let now = std::time::Instant::now();
    let Ok(mut timing) = TIMING.try_lock() else { return };
    let Some(t) = timing.as_mut() else {
        *timing = Some(Timing::new(now));
        return;
    };
    let frame_ms = now.duration_since(t.last).as_secs_f64() * 1000.0;
    t.last = now;
    t.frames += 1;
    t.total_ms += frame_ms;
    t.worst_ms = t.worst_ms.max(frame_ms);
    if t.times.len() < KEPT_FRAMES {
        t.times.push(frame_ms as f32);
    }
    let elapsed_ms = now.duration_since(t.since).as_secs_f64() * 1000.0;
    if elapsed_ms < TIMING_MS {
        return;
    }
    let frames = t.frames;
    let costs = Cost::ALL.map(|c| {
        let (total, worst) = (COST_NS[c as usize].swap(0, Relaxed), COST_WORST_NS[c as usize].swap(0, Relaxed));
        if timed { CostTime { mean_ms: (total as f64 / 1e6 / f64::from(frames)) as f32, worst_ms: (worst as f64 / 1e6) as f32 } } else { CostTime::default() }
    });
    let Ok(mut channel) = CHANNEL.try_lock() else { return };
    let Some(channel) = channel.as_mut() else { return };
    let stretch = Stretch { fps: (f64::from(frames) * 1000.0 / elapsed_ms) as f32, frame_ms: (t.total_ms / f64::from(frames)) as f32, worst_frame_ms: t.worst_ms as f32, size: size(), frames: &t.times, costs };
    if channel.write_game(&stretch) {
        let times = std::mem::take(&mut t.times);
        *t = Timing { times, ..Timing::new(now) };
        t.times.clear();
    }
}

/// The run's rendering settings for the Stats panel, one `Label: value` per line (the framework
/// writes them at the start).
pub fn game_settings(text: &str) {
    if let Some(channel) = CHANNEL.locked().as_mut() {
        channel.write_game_text(text);
    }
}

/// Whether `key` is a stereo loop's frame number (not the clock standing in for one).
pub fn is_loop_key(key: u64) -> bool {
    key & 1 << 63 == 0
}

fn write(key: u64, frame: &[Primitive]) {
    if let Some(channel) = CHANNEL.locked().as_mut() {
        channel.write_frame(key, frame);
    }
}
