//! The same-frame present loop as a ready-made component ([`same_frame`]): both eyes of a pair
//! rendered in one simulation frame, the sibling of the alternate-eye loop ([`crate::feature`],
//! its `DRIVER`). A game lists `same_frame()` among its components and says once what its loop is
//! ([`PresentsPairs::LOOP`]): how its frames leave it ([`Output`]), where its frame labels are, its
//! HUD over each eye, its present entry. Its eye site (`crate::eye_site::EyeSite`, installed with the
//! loop) draws the eyes from the head the loop owns ([`Pairs::head`]); at its present entry it
//! hands each frame to the loop, which publishes the pairs.
//!
//! The ways a pair can leave a game, each with its channel:
//!
//! - [`d3d11`]: each eye presented on its own through a D3D11 swapchain, copied out of the back
//!   buffer as its present is dispatched; the left eye's present kept off the monitor; pairs over
//!   the fence channel.
//! - [`d3d12`]: each eye presented on its own through a D3D12 swapchain, the left eye kept on the
//!   game's queue until the right eye's present publishes the pair (with both eyes' depth, the HUD
//!   laid over each); pairs over the fence channel. Its pair sink ([`d3d12::sink`]) takes whole
//!   pairs from the features that make their own (per-eye frame generation, a game's own method).
//! - [`gl`]: both eyes drawn side by side in one OpenGL frame, read back at `SwapBuffers` onto a
//!   D3D11 device of the loop's own and split; pairs over the fence channel. Or one eye per
//!   frame with its depth, the other eye warped from it ([`gl::GlWarp`], `crate::afw::d3d11`).
//! - [`side_by_side`]: both eyes drawn side by side in one D3D12 or D3D11 back buffer (an engine's
//!   own stereo), its halves copied out at the present on the swapchain's device; pairs over the
//!   fence channel.
//!
//! Which eye a presented frame is comes from the game's frame labels ([`PairLoop::presenting`]), or,
//! for a frame with both eyes in it, from the loop's own tickets: the eye site issues one when it
//! builds a frame's views ([`Pairs::built`]) and the present takes it, so the pair goes out with the
//! pose it was drawn from, however the game pipelines its frames.
//!
//! Stereo pauses for a moment when a pair cannot be made (a changed scene, a menu: [`Pairs::pause`])
//! and resumes by itself; a frame without a pair lately goes out flat, the game's whole frame to the
//! viewer's flat screen. The loop numbers the pairs a game starts on its side ([`Pairs::start_pair`]),
//! keeps the eyes' turns when one eye goes out per frame ([`Pairs::turns`]) and times each stage of
//! a frame ([`Pairs::time`]), logged with its counts.

pub mod d3d11;
pub mod d3d12;
pub mod gl;
pub mod side_by_side;
#[cfg(test)]
mod tests;
pub mod timing;
pub mod turns;

use crate::driver::{Flat, Pending};
use crate::eye_site::EyeSite;
use timing::{Stage, Timings};
use turns::Turns;
use monaka_channel::pose::{HeadSource, PoseChannel, Synthetic};
use monaka_contract::protocol::HeadPose;
use monaka_core::sync::Locked;
use monaka_core::ticket::{Tickets, View};
use monaka_producer::game::{Component, Game, Lack, Start};
use monaka_producer::{Rejection, log};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::*};
use std::time::Duration;
use windows::Win32::Graphics::Direct3D12::{ID3D12CommandQueue, ID3D12Resource};

/// The same-frame loop: its head, its tickets, whether stereo runs, its counts.
pub static PAIRS: Pairs = Pairs::new();

/// How a game's frames leave it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    /// Each eye presented on its own through a D3D11 swapchain ([`d3d11`]).
    D3D11,
    /// Each eye presented on its own through a D3D12 swapchain ([`d3d12`]).
    D3D12,
    /// One OpenGL frame per pair ([`gl`]): both eyes side by side, or (`side_by_side` false) the
    /// whole frame to both eyes.
    OpenGl { side_by_side: bool },
    /// One D3D12 or D3D11 present per pair, both eyes side by side in its back buffer
    /// ([`side_by_side`]).
    SideBySide,
}

/// An eye's HUD at its present: the game's HUD layer (in COMMON, the frame's size) and how it goes
/// over that eye (`monaka_warp::warp12`'s lay-over: its place and disparity, the aim point, the
/// pieces left out, the world markers).
pub struct EyeHud {
    pub layer: ID3D12Resource,
    pub params: monaka_warp::warp12::Params<'static>,
}

/// A D3D12 game's HUD layer laid over each eye of a pair (the HUD kept out of the frame).
#[derive(Clone, Copy)]
pub struct PairHud {
    /// At eye `eye`'s present (the back buffer, in PRESENT): its HUD, or none (the frame goes out as
    /// presented). The game takes its layer here and does its work on the frame before the HUD.
    pub take: fn(&ID3D12CommandQueue, &ID3D12Resource, usize) -> Option<EyeHud>,
    /// After that eye's images were made (`hud` as taken): the layer over the features' own images
    /// of the frame, and the layer given back. Called after every `take`.
    pub done: fn(&ID3D12CommandQueue, Option<&EyeHud>),
}

/// What a game's same-frame loop is.
pub struct PairLoop<G: Game> {
    /// How its frames leave it, for this run.
    pub output: fn(&Start<'_, G>) -> Output,
    /// Its present entry, hooked (each frame handed to the loop there: [`d3d11::around_eye_present`],
    /// [`d3d12::present`], [`gl::present`]).
    pub entry: crate::eye_site::InstallFn<G>,
    /// Whether the run follows the head (by default `head_tracking`): the loop's head source opened.
    pub follows_head: fn(&Start<'_, G>) -> bool,
    /// Its frame labels at a present: the eye presenting (0 left) and the pose its pair is drawn
    /// from (`crate::providers::FrameLabels::presenting`). `None`: each frame holds both eyes, its
    /// pose the loop's ticket ([`Pairs::built`]).
    pub presenting: Option<fn() -> Option<Pending>>,
    /// The pair the frame presenting belongs to, in the game's own numbering (both eyes of a pair
    /// the same number). `None`: the loop numbers them, a left eye starting a pair ([`pair_of`]).
    pub pair: Option<fn() -> u64>,
    /// Whether a frame of this size (width, height) may be published: the channel is made at the
    /// first one's size (`crate::eye_size::ready`, while the game is asked for the eye size).
    pub ready: fn(u32, u32) -> bool,
    /// D3D12: the game's own way to its direct queue, where it executes through one its swapchain
    /// does not hold (Streamline's proxy); None: the swapchain's ([`crate::queue12::game_queue`]).
    pub queue: Option<fn() -> Option<ID3D12CommandQueue>>,
    /// PC checks: the game's own synthetic head for the run's `synthetic_yaw` and `synthetic_pitch`
    /// (degrees); asked at each sample (it may move). By default a fixed one.
    pub synthetic: Option<fn(f32, f32) -> HeadPose>,
    /// With a synthetic head, the pairs' pose records still go to the viewer.
    pub synthetic_records: bool,
    /// The pose each eye of a pair goes out with, adjusted (`crate::eye_site::report`). By default
    /// as drawn.
    pub report: Option<fn(&mut HeadPose)>,
    /// D3D12: its HUD layer over each eye.
    pub hud: Option<PairHud>,
    /// Why a frame goes to the flat screen, as the game knows it, beside the run's situations.
    pub flat: Option<fn() -> Option<Flat>>,
    /// The game's own counts, logged with the loop's.
    pub rate: Option<fn()>,
    /// What the game starts afresh when stereo pauses ([`Pairs::pause`]): each eye's history.
    pub paused: Option<fn()>,
    /// OpenGL: frame warping for this run (the game draws one eye per frame), or none.
    pub warp: fn(&Start<'_, G>) -> Option<gl::GlWarp>,
    /// OpenGL, both eyes side by side: each eye's depth with its pair for this run, or none.
    pub depth: fn(&Start<'_, G>) -> Option<gl::GlPairDepth>,
    /// The game presents each frame only once it has started drawing the next (its swap at the
    /// start of the next frame's drawing): the frame presented is the one before the frame drawing,
    /// whose ticket is already issued ([`monaka_core::ticket::Tickets::show_behind_drawing`]).
    pub presents_behind_drawing: bool,
}

impl<G: Game> PairLoop<G> {
    /// A loop putting out its frames as `output` says, entered where `entry` hooks, with no step of
    /// its own.
    pub const fn new(output: fn(&Start<'_, G>) -> Output, entry: crate::eye_site::InstallFn<G>) -> Self {
        PairLoop {
            output,
            entry,
            follows_head: |start| start.shared.head_tracking,
            presenting: None,
            pair: None,
            ready: |_, _| true,
            queue: None,
            synthetic: None,
            synthetic_records: false,
            report: None,
            hud: None,
            flat: None,
            rate: None,
            paused: None,
            warp: |_| None,
            depth: |_| None,
            presents_behind_drawing: false,
        }
    }
}

/// A game whose pairs go out by the same-frame loop.
pub trait PresentsPairs: Game {
    const LOOP: PairLoop<Self>;
}

/// The same-frame loop component for game `G`: the head opened, the channel's publisher made, the
/// present entry hooked (the game's, and DXGI's Present on D3D11), the eye site installed
/// ([`EyeSite::install`]).
pub const fn same_frame<G: PresentsPairs + EyeSite>() -> Component<G> {
    Component { name: "present", needs_channel: true, install: install::<G>, arm, quiet, give_back, release, summary: Some(summary), ..Component::NONE }
}

/// What the loop reads of its game at each present.
#[derive(Clone, Copy)]
pub(crate) struct Settings {
    pub output: Output,
    pub presenting: Option<fn() -> Option<Pending>>,
    pub pair: Option<fn() -> u64>,
    pub ready: fn(u32, u32) -> bool,
    pub report: Option<fn(&mut HeadPose)>,
    pub hud: Option<PairHud>,
    pub flat: Option<fn() -> Option<Flat>>,
    pub rate: Option<fn()>,
    pub paused: Option<fn()>,
    pub presents_behind_drawing: bool,
}

impl Settings {
    /// The frame labels' eye presenting now, with its pair's pose.
    pub fn presenting(&self) -> Option<Pending> {
        self.presenting.and_then(|presenting| presenting())
    }

    /// The pair eye `eye`'s frame belongs to: the game's number, or the loop's own on `counter`.
    pub fn pair(&self, eye: usize, counter: &AtomicU64) -> u64 {
        match self.pair {
            Some(pair) => pair(),
            None => pair_of(eye, counter),
        }
    }

    /// Each eye's pose as it goes out.
    pub fn report(&self, pose: &mut HeadPose) {
        if let Some(report) = self.report {
            report(pose);
        }
    }
}

fn install<G: PresentsPairs + EyeSite>(start: &mut Start<'_, G>) -> Result<(), Lack> {
    let shared = *start.shared;
    let channel = start.channel()?.clone();
    let game = G::LOOP;
    let output = (game.output)(start);
    if (game.follows_head)(start) {
        let synthetic: Option<Synthetic> = match (shared.synthetic, game.synthetic) {
            (Some((yaw, pitch)), Some(make)) => Some(Box::new(move || make(yaw, pitch))),
            _ => shared.synthetic_head().map(|pose| Box::new(move || pose) as Synthetic),
        };
        let poses = match synthetic.is_none() || game.synthetic_records {
            true => match PoseChannel::open(&channel) {
                Ok(poses) => Some(poses),
                Err(e) if synthetic.is_some() => {
                    log!("pose channel: {e}; the pairs go without pose records");
                    None
                }
                Err(e) => return Err(Rejection::not_ready(format!("pose channel: {e}")).into()),
            },
            false => None,
        };
        PAIRS.head.open(poses, synthetic);
    }
    crate::queue12::use_own(game.queue);
    *PAIRS.settings.locked() = Some(Settings {
        output,
        presenting: game.presenting,
        pair: game.pair,
        ready: game.ready,
        report: game.report,
        hud: game.hud,
        flat: game.flat,
        rate: game.rate,
        paused: game.paused,
        presents_behind_drawing: game.presents_behind_drawing,
    });
    match output {
        // SAFETY: called at start, before the hooks are enabled.
        Output::D3D11 => unsafe { d3d11::install(start.hooks, channel)? },
        Output::D3D12 => d3d12::install(channel),
        Output::OpenGl { side_by_side } => gl::install(channel, side_by_side, (game.warp)(start), (game.depth)(start)),
        Output::SideBySide => side_by_side::install(channel),
    }
    (game.entry)(start)?;
    G::install(start)
}

fn arm() {
    PAIRS.tickets.locked().clear();
    PAIRS.capture.store(true, Release);
}

/// Publishing stops; a pair in progress gives the game its camera back itself.
fn quiet() {
    PAIRS.capture.store(false, Release);
    PAIRS.halt();
}

fn give_back() {
    gl::give_back();
}

fn release() {
    d3d11::close();
    d3d12::close();
    side_by_side::close();
    gl::release();
    PAIRS.head.close();
    let mut tickets = PAIRS.tickets.locked();
    if tickets.missing() + tickets.lost() > 0 || PAIRS.built.load(Relaxed) > 0 {
        log!("frames built to shown: {}", tickets.summary());
    }
    tickets.clear();
    drop(tickets);
    PAIRS.log_rates();
}

fn summary() -> String {
    PAIRS.summary()
}

/// The stages' average times since the last report (for a game's stop line; empty when none was
/// timed).
pub fn timing_report() -> String {
    PAIRS.timing_report()
}

/// The pair an eye's frame belongs to, numbered on `pairs`: a left eye (0) starts a new one; a right
/// eye ends the newest, so a right eye with no left eye since the last right one belongs to none
/// (its pair is never complete). A left eye whose right eye never came still pairs with the next
/// right eye that comes alone: a game whose eyes' presents can go missing numbers its pairs itself
/// ([`PairLoop::pair`]).
pub(crate) fn pair_of(eye: usize, pairs: &AtomicU64) -> u64 {
    match eye {
        0 => pairs.fetch_add(1, AcqRel) + 1,
        _ => pairs.fetch_add(1, AcqRel),
    }
}

/// More frames than this between building and showing means the pairing is lost.
const MAX_IN_FLIGHT: usize = 4;
/// Without a pair this recently (ms), frames go out flat.
pub const RECENT_PAIR_MS: u64 = 250;
/// A paused stereo resumes after this long (ms).
pub const PAUSE_MS: u64 = 100;

/// The same-frame loop.
pub struct Pairs {
    /// Set while the loop publishes (from the arm to the stop's first step).
    capture: AtomicBool,
    head: HeadSource,
    tickets: Mutex<Tickets>,
    /// Frames built with a ticket.
    built: AtomicU64,
    settings: Mutex<Option<Settings>>,
    /// Stereo runs: the eye site draws eyes.
    running: AtomicBool,
    /// Stopped for the rest of the run (something the pair relies on was not as expected).
    blocked: AtomicBool,
    /// Stereo resumes no earlier than this tick.
    resume_at: AtomicU64,
    paused: AtomicU64,
    /// Pairs finished, and when the last one did.
    done: AtomicU64,
    last_pair: AtomicU64,
    presents: AtomicU64,
    published: AtomicU64,
    skipped: AtomicU64,
    /// Frames that went flat, by reason ([`Flat::ALL`]'s order).
    flat: [AtomicU64; Flat::ALL.len()],
    /// Pairs the game started on its side ([`Pairs::start_pair`]).
    started: AtomicU64,
    /// The eyes' turns, one eye per frame.
    turns: Turns,
    /// Each stage of a frame timed.
    timings: Timings,
}

impl Pairs {
    pub const fn new() -> Self {
        Self {
            capture: AtomicBool::new(false),
            head: HeadSource::new(),
            tickets: Mutex::new(Tickets::new(MAX_IN_FLIGHT)),
            built: AtomicU64::new(0),
            settings: Mutex::new(None),
            running: AtomicBool::new(false),
            blocked: AtomicBool::new(false),
            resume_at: AtomicU64::new(0),
            paused: AtomicU64::new(0),
            done: AtomicU64::new(0),
            last_pair: AtomicU64::new(0),
            presents: AtomicU64::new(0),
            published: AtomicU64::new(0),
            skipped: AtomicU64::new(0),
            flat: [const { AtomicU64::new(0) }; Flat::ALL.len()],
            started: AtomicU64::new(0),
            turns: Turns::new(),
            timings: Timings::new(),
        }
    }

    pub(crate) fn settings(&self) -> Option<Settings> {
        *self.settings.locked()
    }

    /// Where the head pose comes from: the viewer's, or a synthetic one on PC runs.
    pub const fn head(&self) -> &HeadSource {
        &self.head
    }

    /// Whether the loop publishes (from the arm to the stop's first step).
    pub fn publishing(&self) -> bool {
        self.capture.load(Acquire)
    }

    /// Whether stereo runs: the eye site draws eyes.
    pub fn running(&self) -> bool {
        self.running.load(Acquire)
    }

    /// Stereo stops drawing eyes (the stop; a pair in progress finishes the game's way).
    pub fn halt(&self) {
        self.running.store(false, Release);
    }

    /// A pair could not be made (a changed scene, a menu, loading): stereo pauses for
    /// [`PAUSE_MS`] and resumes by itself, the eyes' turns and the game's own state
    /// ([`PairLoop::paused`]) afresh. Logged with `reason` now and then.
    pub fn pause(&self, reason: &str) {
        self.running.store(false, Release);
        self.resume_at.store(monaka_channel::tick() + PAUSE_MS, Release);
        let n = self.paused.fetch_add(1, Relaxed) + 1;
        if n <= 8 || n.is_multiple_of(100) {
            log!("stereo paused: {reason} (count {n})");
        }
        if let Some(paused) = self.settings().and_then(|s| s.paused) {
            paused();
        }
        self.turns.start_over();
    }

    /// Something a pair relies on was not as expected: stereo stops for the rest of the run.
    pub fn stop_for_run(&self, reason: &str) {
        self.running.store(false, Release);
        self.blocked.store(true, Release);
        log!("stereo stopped for this run: {reason}");
    }

    /// At the start of a frame: stereo runs again once a pause is over, while publishing and not
    /// stopped for the run. Whether it runs.
    pub fn resume(&self) -> bool {
        if !self.running() && !self.blocked.load(Acquire) && monaka_channel::tick() >= self.resume_at.load(Acquire) && self.publishing() {
            self.running.store(true, Release);
        }
        self.running()
    }

    /// A pair the game starts on its side (both its eyes' frames carry the number): its number.
    pub fn start_pair(&self) -> u64 {
        self.started.fetch_add(1, Relaxed) + 1
    }

    /// The eyes' turns, for a game drawing one eye per frame.
    pub fn turns(&self) -> &Turns {
        &self.turns
    }

    /// `stage` of a frame took `took`.
    pub fn time(&self, stage: Stage, took: Duration) {
        self.timings.time(stage, took);
    }

    /// The stages' average times since the last report, each started afresh (empty when none was
    /// timed).
    pub fn timing_report(&self) -> String {
        self.timings.report()
    }

    /// A pair finished (`kind`: how it was made, `counter`: the game's frame): the pairs' spacing
    /// timed, the stereo clock set, a log line now and then.
    pub fn pair_done(&self, kind: &str, counter: u32) {
        self.timings.pair_done();
        self.last_pair.store(monaka_channel::tick(), Release);
        let n = self.done.fetch_add(1, Relaxed) + 1;
        if n <= 3 || n.is_multiple_of(600) {
            log!("{kind} pair {n} counter={counter}");
        }
    }

    /// Whether a pair finished lately ([`RECENT_PAIR_MS`]): frames between pairs are not shown flat.
    pub fn recent_pair(&self) -> bool {
        monaka_channel::tick().saturating_sub(self.last_pair.load(Acquire)) <= RECENT_PAIR_MS
    }

    /// Pairs finished, and pauses, so far.
    pub fn pairs_done(&self) -> (u64, u64) {
        (self.done.load(Relaxed), self.paused.load(Relaxed))
    }

    /// Presents of the game's frames so far.
    pub fn presents(&self) -> u64 {
        self.presents.load(Relaxed)
    }

    /// A present of the game's frame: its number.
    pub(crate) fn present(&self) -> u64 {
        self.presents.fetch_add(1, Relaxed) + 1
    }

    /// The eye site built a frame's views from `pose` (`view`: both eyes, for a frame holding the
    /// pair): its ticket.
    pub fn built(&self, view: View, pose: HeadPose) {
        self.built.fetch_add(1, Relaxed);
        self.tickets.locked().issue(view, pose, ());
    }

    /// The pose of the frame being drawn now (the oldest built and not shown).
    pub fn rendering(&self) -> Option<HeadPose> {
        self.tickets.locked().drawing().map(|ticket| ticket.pose)
    }

    /// The frame being shown now: its pose, and how many built frames were waiting (1 or 2 when the
    /// pairing holds).
    pub(crate) fn shown(&self) -> (Option<HeadPose>, usize) {
        let behind = self.settings().is_some_and(|s| s.presents_behind_drawing);
        let mut tickets = self.tickets.locked();
        let shown = if behind { tickets.show_behind_drawing() } else { tickets.show() };
        (shown.ticket.map(|ticket| ticket.pose), shown.waiting)
    }

    /// Why the frame about to present goes flat: the game's detector first, then the run's
    /// situations (a video, no world drawn). Counted.
    pub(crate) fn flat_now(&self, detector: Option<fn() -> Option<Flat>>) -> Option<Flat> {
        let (situations, now) = monaka_producer::situation::snapshot();
        let flat = Flat::of_frame(detector, None, &situations, now);
        if let Some(reason) = flat {
            self.flat[reason as usize].fetch_add(1, Relaxed);
        }
        flat
    }

    /// One publish attempt: counted; at the first present and every 600 the loop's counts logged
    /// (every 90 with diagnostic logging).
    pub(crate) fn count(&self, ok: bool) {
        self.tally(ok);
        let n = self.presents();
        let every = if monaka_producer::log::detailed() { 90 } else { 600 };
        if n == 1 || n.is_multiple_of(every) {
            self.log_counts();
        }
    }

    /// One publish attempt, counted only.
    pub(crate) fn tally(&self, ok: bool) {
        if ok { &self.published } else { &self.skipped }.fetch_add(1, Relaxed);
    }

    fn log_counts(&self) {
        log!("{}", self.counts_line());
        self.log_rates();
    }

    /// The game's own counts, then the stages' times.
    fn log_rates(&self) {
        if let Some(rate) = self.settings().and_then(|s| s.rate) {
            rate();
        }
        let timing = self.timing_report();
        if !timing.is_empty() {
            log!("{timing}");
        }
    }

    /// `presents=.. published=.. skipped=.. stereo=.. paused=..`.
    fn counts_line(&self) -> String {
        let (done, paused) = self.pairs_done();
        format!("presents={} published={} skipped={} stereo={done} paused={paused}", self.presents(), self.published.load(Relaxed), self.skipped.load(Relaxed))
    }

    /// The stop's line: the counts, the frames shown flat, what the output adds.
    pub fn summary(&self) -> String {
        let mut line = self.counts_line();
        let flat: Vec<String> = Flat::ALL.iter().zip(&self.flat).filter(|(_, c)| c.load(Relaxed) > 0).map(|(f, c)| format!("{} {}", f.label(), c.load(Relaxed))).collect();
        if !flat.is_empty() {
            line += &format!("; frames shown flat: {}", flat.join(", "));
        }
        let output = match self.settings().map(|s| s.output) {
            Some(Output::D3D11) => d3d11::report(),
            Some(Output::D3D12) => d3d12::report(),
            Some(Output::OpenGl { .. }) => gl::report(),
            Some(Output::SideBySide) => side_by_side::report(),
            None => String::new(),
        };
        if !output.is_empty() {
            line += &format!("; {output}");
        }
        line
    }
}

impl Default for Pairs {
    fn default() -> Self {
        Self::new()
    }
}
