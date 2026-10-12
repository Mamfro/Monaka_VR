//! The alternate-eye present loop, for any game that draws one eye per frame: the part of a
//! producer's `stereo.rs` that is the same from game to game. The game
//! keeps what only it knows, writing the eye camera where its engine reads it; this drives the
//! rest around DXGI's `Present`:
//!
//! - which swapchain is the game's (the first presented at a real size; every other one passes
//!   straight through);
//! - presents counted from attachment, the publisher fed before each present (the eye shown,
//!   pairs published with their pose records, the first pair and a rate line logged);
//! - after each present, the next eye and the head pose it is drawn from ([`Driver::pending`]:
//!   both eyes of a pair from one pose, sampled for its left-eye frame), the pose recorded for
//!   the pair it will complete;
//! - the end of a run: [`Driver::retire`] from the stop, [`Driver::finish`] on the present
//!   thread, [`Driver::release`] once the hooks are out.
//!
//! The game's detour is one line: `DRIVER.present(swap, interval, flags, PRESENT_ORIGINAL.get())`,
//! or, from an engine's own present call, `DRIVER.around_present(chain, || original(..))`.
//! Its camera hook reads [`Driver::pending`] and draws that eye from that pose. A per-game
//! counter of those draws goes in the rate line ([`Install::work`]). A game with more to do at
//! each present (its own publishers for some modes, a live camera to turn, controllers to read)
//! hangs that on [`Install::before`], [`Install::after`] and [`Install::finish`]; one that moves
//! the head's tracking origin (room-scale following) adjusts the pose in [`Install::adjust_pose`].
//!
//! Two things an engine may add:
//!
//! - **frames labelled by their camera:** the camera hook notes each eye camera it writes
//!   ([`Driver::camera_written`]), and the renderer, once it knows the camera it renders the coming
//!   frame from, labels that frame ([`Driver::label_frame`]): the frame then goes out as that eye
//!   with that pose, whatever the timing ([`AlternatePublisher12::show_labelled`]);
//! - **choices taken in turn** ([`Install::in_turn`]): an engine whose camera updates and presents
//!   are not in lock-step takes the eye choices one by one ([`Driver::take_in_turn`]), each drawn
//!   from a pose sampled at that update.

use crate::alternate::{AlternatePublisher, Step};
use crate::alternate12::AlternatePublisher12;
use crate::present::PresentFn;
use monaka_channel::pose::{HeadSource, PoseChannel, Synthetic};
use monaka_core::alternate::Schedule;
use monaka_core::options::Shared;
use monaka_core::math::Vec3;
use monaka_core::protocol::HeadPose;
use monaka_hook::InFlight;
use monaka_producer::{log, log_first};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, AtomicUsize, Ordering::*};
use std::sync::Mutex;
use windows::Win32::Graphics::Direct3D12::ID3D12CommandQueue;
use windows::Win32::Graphics::Dxgi::{IDXGISwapChain, IDXGISwapChain3};
use windows::core::{HRESULT, Interface};
use monaka_core::sync::Locked;

/// A swapchain narrower than this is not the game's (a probe, an overlay).
const MIN_GAME_WIDTH: u32 = 640;

/// What the next frame draws: its eye and the head pose of its pair.
#[derive(Clone, Copy, Debug)]
pub struct Pending {
    pub eye: usize,
    pub pose: HeadPose,
}

/// Where the pairs go.
pub enum Publisher {
    /// The keyed-mutex channel, copies on the swapchain's D3D11 device.
    D3D11(AlternatePublisher),
    /// The fence channel, copies on the game's direct queue (`queue`, looked up at each present
    /// for the swapchain presenting: the game may make it after the hooks go in).
    D3D12 { publisher: AlternatePublisher12, queue: fn(&IDXGISwapChain3) -> Option<ID3D12CommandQueue> },
}

/// What a game gives the driver at start.
pub struct Install {
    pub shared: Shared,
    pub publisher: Publisher,
    /// The viewer's pose channel (none: a synthetic head, or no head at all).
    pub poses: Option<PoseChannel>,
    /// Once per present on the game's swapchain, before anything else (a HUD recorder's frame
    /// end).
    pub frame_end: Option<fn(&IDXGISwapChain)>,
    /// The pose each eye of a published pair goes out with (the field of view it was rendered
    /// with, the eye distance used). None: [`Driver::report_centred`], the centred frustum around
    /// each eye's from the headset's, or the game's own field of view ([`Driver::note_game_fov`]).
    pub report: Option<fn(&Driver, &mut HeadPose)>,
    /// The game's draws of eye views, for the rate line: its name and counter.
    pub work: (&'static str, &'static AtomicU64),
    /// Before each present `n` (the game's swapchain, capturing), ahead of the publisher: true
    /// when the game published this frame itself (another mode's path), so the publisher is not
    /// fed. May use [`Driver::with_publisher11`].
    pub before: Option<fn(&Driver, &IDXGISwapChain, u64) -> bool>,
    /// After each present `n`, with the pair's head pose, once it is recorded and before the next
    /// eye is announced ([`Driver::pending`]): the game's own per-present work.
    pub after: Option<fn(&Driver, u64, &HeadPose)>,
    /// The head pose each pair is drawn from, for present `n`, adjusted before it is recorded and
    /// announced (a tracking origin moved under the player, a PC check's turn).
    pub adjust_pose: Option<fn(u64, &mut HeadPose)>,
    /// Whether a menu is up: its frames go to both eyes flat.
    pub menu: Option<fn() -> bool>,
    /// The camera updates take the eye choices in turn ([`Driver::take_in_turn`]), each sampling
    /// its pose: none is sampled at the present.
    pub in_turn: bool,
    /// With choices taken in turn: whether each frame is drawn from the newest pose rather than
    /// its pair's (frame generation: a generated eye stands for the other eye's moment).
    pub pose_each_frame: Option<fn() -> bool>,
    /// The end of a run, before the publisher lets its eye copies go: what the game changed in
    /// the engine, given back; its tallies.
    pub finish: Option<fn(&Driver)>,
}

struct State {
    shared: Shared,
    publisher: Publisher,
    frame_end: Option<fn(&IDXGISwapChain)>,
    report: Option<fn(&Driver, &mut HeadPose)>,
    work: (&'static str, &'static AtomicU64),
    before: Option<fn(&Driver, &IDXGISwapChain, u64) -> bool>,
    after: Option<fn(&Driver, u64, &HeadPose)>,
    adjust_pose: Option<fn(u64, &mut HeadPose)>,
    menu: Option<fn() -> bool>,
    in_turn: bool,
    pose_each_frame: Option<fn() -> bool>,
    finish: Option<fn(&Driver)>,
    pair_pose: HeadPose,
}

/// A frame recognised by its camera: its eye and the pose it was drawn from.
#[derive(Clone, Copy, Debug)]
pub struct Label {
    pub eye: usize,
    pub pose: HeadPose,
}

/// A frame taken in turn ([`Driver::take_in_turn`]): the present it follows, its eye, its pose.
#[derive(Clone, Copy, Debug)]
pub struct Taken {
    pub n: u64,
    pub eye: usize,
    pub pose: HeadPose,
}

/// The eye cameras written lately.
const WRITTEN_KEPT: usize = 8;
/// A rendered camera this close (world units, metres) to a written one is that one.
const SAME_CAMERA: f32 = 0.005;

/// What the labels and the choices in turn keep.
struct Frames {
    written: [Option<(Vec3, Label)>; WRITTEN_KEPT],
    next: usize,
    label: Option<Label>,
    /// Frames have been labelled this run, so a frame without a label is not guessed.
    labelling: bool,
    sequencer: Sequencer,
    pair_pose: HeadPose,
}

impl Frames {
    const NEW: Frames = Frames { written: [None; WRITTEN_KEPT], next: 0, label: None, labelling: false, sequencer: Sequencer::NEW, pair_pose: HeadPose::EMPTY };
}

/// The frames' tallies, for the summary.
struct Tally {
    labelled: AtomicU64,
    unmatched: AtomicU64,
    unlabelled: AtomicU64,
    disagreed: AtomicU64,
    early: AtomicU64,
    late: AtomicU64,
    resynced: AtomicU64,
    flat: AtomicU64,
}

struct Cell(Option<State>);
// SAFETY: the D3D objects and shared-memory views inside are used by one thread at a time, under
// the mutex: the game's present thread, or the stop after its hooks are off.
unsafe impl Send for Cell {}

pub struct Driver {
    capture: AtomicBool,
    /// Set once [`Driver::finish`] gave back what the run changed: a present still inside the
    /// game's call at that moment must not start another frame.
    finished: AtomicBool,
    /// The eye the next frame draws; -1 when not running.
    eye: AtomicI32,
    presents: AtomicU64,
    /// Present count at attachment; presents are numbered from it.
    attached_at: AtomicU64,
    game_chain: AtomicUsize,
    pending: Mutex<Option<Pending>>,
    /// The game's own field of view (full angles, radians, f32 bits), reported for frames drawn
    /// without a headset pose.
    game_fov: [AtomicU32; 2],
    unpaired: AtomicU64,
    /// Work and presents at the last rate line.
    rate_mark: Mutex<(u64, u64)>,
    /// The settings the driver was installed with, apart from `state`: the publisher's report runs
    /// while `state` is held and asks for them ([`Driver::report_centred`]).
    settings: Mutex<Option<Shared>>,
    state: Mutex<Cell>,
    head: HeadSource,
    frames: Mutex<Frames>,
    /// Eye choices made, one per present (taken in turn by [`Driver::take_in_turn`]).
    choices: AtomicU64,
    tally: Tally,
}

impl Driver {
    pub const fn new() -> Self {
        Self {
            capture: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            eye: AtomicI32::new(-1),
            presents: AtomicU64::new(0),
            attached_at: AtomicU64::new(0),
            game_chain: AtomicUsize::new(0),
            pending: Mutex::new(None),
            game_fov: [AtomicU32::new(0), AtomicU32::new(0)],
            unpaired: AtomicU64::new(0),
            rate_mark: Mutex::new((0, 0)),
            settings: Mutex::new(None),
            state: Mutex::new(Cell(None)),
            head: HeadSource::new(),
            frames: Mutex::new(Frames::NEW),
            choices: AtomicU64::new(0),
            tally: Tally {
                labelled: AtomicU64::new(0),
                unmatched: AtomicU64::new(0),
                unlabelled: AtomicU64::new(0),
                disagreed: AtomicU64::new(0),
                early: AtomicU64::new(0),
                late: AtomicU64::new(0),
                resynced: AtomicU64::new(0),
                flat: AtomicU64::new(0),
            },
        }
    }

    /// At start, before the hooks go in.
    pub fn install(&self, install: Install) {
        let synthetic: Option<Synthetic> = install.shared.synthetic_head().map(|pose| Box::new(move || pose) as Synthetic);
        self.head.open(install.poses, synthetic);
        *self.settings.locked() = Some(install.shared);
        let mut cell = self.state.locked();
        cell.0 = Some(State {
            shared: install.shared,
            publisher: install.publisher,
            frame_end: install.frame_end,
            report: install.report,
            work: install.work,
            before: install.before,
            after: install.after,
            adjust_pose: install.adjust_pose,
            menu: install.menu,
            in_turn: install.in_turn,
            pose_each_frame: install.pose_each_frame,
            finish: install.finish,
            pair_pose: HeadPose::default(),
        });
    }

    /// Starts choosing eyes at the next present (the hooks are in).
    pub fn start(&self) {
        *self.frames.locked() = Frames::NEW;
        self.choices.store(0, Relaxed);
        self.finished.store(false, Release);
        self.capture.store(true, Release);
    }

    /// The camera hook wrote eye `eye`'s camera at `position`, drawn from `pose`: a frame rendered
    /// from it can be recognised ([`Driver::label_frame`]).
    pub fn camera_written(&self, position: Vec3, eye: usize, pose: HeadPose) {
        let mut frames = self.frames.locked();
        let slot = frames.next % WRITTEN_KEPT;
        frames.written[slot] = Some((position, Label { eye, pose }));
        frames.next += 1;
    }

    /// The renderer reports the camera position it renders the coming frame with: the frame is
    /// labelled with the eye and pose of the camera written there. Returns the eye, if recognised.
    pub fn label_frame(&self, position: Vec3) -> Option<usize> {
        if !self.capture.load(Acquire) || !position.iter().all(|v| v.is_finite()) {
            return None;
        }
        let mut frames = self.frames.locked();
        let distance = |p: &Vec3| (0..3).map(|i| (p[i] - position[i]).abs()).fold(0.0, f32::max);
        let found = frames.written.iter().flatten().filter(|(p, _)| distance(p) < SAME_CAMERA).min_by(|a, b| distance(&a.0).total_cmp(&distance(&b.0))).map(|(_, label)| *label);
        match found {
            Some(label) => {
                frames.label = Some(label);
                frames.labelling = true;
                self.tally.labelled.fetch_add(1, Relaxed);
                Some(label.eye)
            }
            None => {
                self.tally.unmatched.fetch_add(1, Relaxed);
                None
            }
        }
    }

    /// The label of the frame about to present, left for the present loop.
    pub fn peek_label(&self) -> Option<Label> {
        self.frames.locked().label
    }

    /// The label of the frame about to present, taken (a game's own publishing path).
    pub fn take_label(&self) -> Option<Label> {
        self.frames.locked().label.take()
    }

    /// For [`Install::in_turn`]: the next camera update's frame, taking the choices made at the
    /// presents one by one ([`Sequencer`]); its pose sampled now for a pair's left eye (or every
    /// frame, [`Install::pose_each_frame`]), recorded for the pair it completes. None while not
    /// running.
    pub fn take_in_turn(&self) -> Option<Taken> {
        if !self.capture.load(Acquire) || self.eye.load(Acquire) < 0 {
            return None;
        }
        let (choice, taken) = self.frames.locked().sequencer.next(self.choices.load(Acquire));
        let counter = match taken {
            Turn::InStep => None,
            Turn::Early => Some(&self.tally.early),
            Turn::Late => Some(&self.tally.late),
            Turn::Resynced => Some(&self.tally.resynced),
        };
        if let Some(counter) = counter {
            counter.fetch_add(1, Relaxed);
        }
        // The camera written for choice `choice` is the one drawn after present `choice - 1`.
        let n = choice.saturating_sub(1);
        let mut cell = self.state.locked();
        let state = cell.0.as_mut()?;
        let each_frame = state.pose_each_frame.is_some_and(|f| f());
        let pose = {
            let mut frames = self.frames.locked();
            if Schedule::starts_pair(n) || each_frame {
                let tracked = state.shared.head_tracking || state.shared.synthetic.is_some();
                let mut pose = if tracked { self.head.current().unwrap_or_default() } else { HeadPose::default() };
                if let Some(adjust) = state.adjust_pose {
                    adjust(n, &mut pose);
                }
                frames.pair_pose = pose;
            }
            frames.pair_pose
        };
        match &mut state.publisher {
            Publisher::D3D11(publisher) => publisher.record_pose(n, pose),
            Publisher::D3D12 { publisher, .. } => publisher.record_pose(n, pose),
        }
        Some(Taken { n, eye: Schedule::camera_eye(n), pose })
    }

    /// Whether the run's end has given everything back ([`Driver::finish`] ran).
    pub fn finished(&self) -> bool {
        self.finished.load(Acquire)
    }

    /// The D3D11 publisher, for a game's own publishing path (none on D3D12 or before install).
    pub fn with_publisher11<R>(&self, f: impl FnOnce(&mut AlternatePublisher) -> R) -> Option<R> {
        let mut cell = self.state.locked();
        match cell.0.as_mut().map(|s| &mut s.publisher) {
            Some(Publisher::D3D11(publisher)) => Some(f(publisher)),
            _ => None,
        }
    }

    /// The pose the next frame is drawn from, while running (the default pose otherwise).
    pub fn pending_pose(&self) -> HeadPose {
        self.pending().map_or_else(HeadPose::default, |p| p.pose)
    }

    /// Stops at the next present, which retires the eye and gives the game its own frames back;
    /// waits up to `wait_ms` for it, else finishes from the caller's thread (the game stopped
    /// presenting: a pause, a loading screen).
    pub fn retire(&self, wait_ms: u64) {
        self.capture.store(false, Release);
        monaka_producer::wait_until(wait_ms, || self.eye.load(Acquire) == -1);
        if self.eye.swap(-1, AcqRel) != -1 {
            log!("no present while stopping; finishing from the loader thread");
            self.finish();
        }
    }

    pub fn capturing(&self) -> bool {
        self.capture.load(Acquire)
    }

    /// The eye the next frame draws and its pose, while running.
    pub fn pending(&self) -> Option<Pending> {
        if !self.capture.load(Acquire) || self.eye.load(Acquire) < 0 {
            return None;
        }
        *self.pending.locked()
    }

    /// Whether eye cameras are being written.
    pub fn running(&self) -> bool {
        self.eye.load(Acquire) >= 0
    }

    /// The eye the next frame draws (-1 when not running), for a game's own logs.
    pub fn eye(&self) -> i32 {
        self.eye.load(Acquire)
    }

    pub fn head(&self) -> &HeadSource {
        &self.head
    }

    /// The game's field of view at its latest frame (full horizontal and vertical angles,
    /// radians), for the pose report of frames drawn without a headset pose.
    pub fn note_game_fov(&self, fov: [f32; 2]) {
        for (bits, value) in self.game_fov.iter().zip(fov) {
            bits.store(value.to_bits(), Relaxed);
        }
    }

    pub fn game_fov(&self) -> [f32; 2] {
        self.game_fov.each_ref().map(|bits| f32::from_bits(bits.load(Relaxed)))
    }

    /// Presents since the game started (a frame number for once-a-frame work).
    pub fn presents(&self) -> u64 {
        self.presents.load(Relaxed)
    }

    /// Present number `n` counted from attachment of the frame being drawn (the one the next
    /// present shows), once attached.
    pub fn drawing_present(&self) -> Option<u64> {
        let attached = self.attached_at.load(Relaxed);
        (attached != 0).then(|| self.presents.load(Relaxed) + 1 - attached)
    }

    pub fn schedule(&self) -> Option<Schedule> {
        self.shared().map(|s| s.schedule)
    }

    /// The settings the driver was installed with (safe to ask from a report or any hook: it
    /// never waits on the publisher's state).
    pub fn shared(&self) -> Option<Shared> {
        *self.settings.locked()
    }

    /// The default pose report: the eye distance used, and each eye's field of view as rendered,
    /// the centred frustum around the headset's for that eye, or the game's own.
    pub fn report_centred(&self, pose: &mut HeadPose) {
        let Some(shared) = self.shared() else { return };
        pose.ipd = shared.separation_for(pose);
        let game_fov = self.game_fov();
        for eye in 0..2 {
            if let Some(frustum) = shared.rendered_frustum(pose, eye, true, Some(game_fov)) {
                pose.fov[eye] = frustum.fov();
            }
        }
    }

    /// The latest pair published (D3D12): its sequence and the poses it went out with.
    pub fn last_pair(&self) -> Option<(u64, [HeadPose; 2])> {
        match self.state.locked().0.as_ref().map(|s| &s.publisher) {
            Some(Publisher::D3D12 { publisher, .. }) => publisher.last_pair(),
            _ => None,
        }
    }

    /// Pairs published, failures, presents, unpaired right eyes: the tail of a run's summary.
    pub fn summary(&self) -> String {
        let cell = self.state.locked();
        let (pairs, failures) = match cell.0.as_ref().map(|s| &s.publisher) {
            Some(Publisher::D3D11(p)) => (p.pairs(), p.failures()),
            Some(Publisher::D3D12 { publisher, .. }) => (publisher.pairs(), publisher.failures()),
            None => (0, 0),
        };
        let t = &self.tally;
        let count = |c: &AtomicU64| c.load(Relaxed);
        let mut line = format!(
            "{} presents, {pairs} pairs published, {failures} failures, {} unpaired",
            self.presents.load(Relaxed).saturating_sub(self.attached_at.load(Relaxed)),
            self.unpaired.load(Relaxed)
        );
        if count(&t.labelled) + count(&t.unmatched) > 0 {
            line += &format!("; frames labelled {}, unmatched {}, unlabelled {}, label differed from timing {}", count(&t.labelled), count(&t.unmatched), count(&t.unlabelled), count(&t.disagreed));
        }
        if count(&t.early) + count(&t.late) + count(&t.resynced) > 0 {
            line += &format!("; camera updates early {}, late {}, resynced {}", count(&t.early), count(&t.late), count(&t.resynced));
        }
        if count(&t.flat) > 0 {
            line += &format!("; menu frames flat {}", count(&t.flat));
        }
        line
    }

    /// Whether `swap` is the game's swapchain (the first one presented at a real size).
    fn is_game_chain(&self, swap: usize, chain: &IDXGISwapChain) -> bool {
        match self.game_chain.load(Acquire) {
            0 => {
                // SAFETY: a description query on the swapchain being presented.
                let Ok(desc) = (unsafe { chain.GetDesc() }) else { return false };
                if desc.BufferDesc.Width < MIN_GAME_WIDTH {
                    return false;
                }
                if self.game_chain.compare_exchange(0, swap, AcqRel, Acquire).is_ok() {
                    log!("game swapchain {swap:#x}: {}x{} format {}", desc.BufferDesc.Width, desc.BufferDesc.Height, desc.BufferDesc.Format.0);
                }
                self.game_chain.load(Acquire) == swap
            }
            game => game == swap,
        }
    }

    /// `IDXGISwapChain::Present`, for every swapchain in the process: the game's detour forwards
    /// here with the original.
    ///
    /// # Safety
    /// `swap` is the swapchain being presented and `original` DXGI's own `Present`.
    pub unsafe fn present(&self, swap: *mut core::ffi::c_void, interval: u32, flags: u32, original: PresentFn) -> HRESULT {
        let _flight = InFlight::enter();
        // SAFETY: the caller passes a live swapchain.
        let chain = unsafe { IDXGISwapChain::from_raw_borrowed(&swap) };
        let Some(chain) = chain.filter(|chain| self.is_game_chain(swap as usize, chain)) else {
            // SAFETY: forwards the caller's own call.
            return unsafe { original(swap, interval, flags) };
        };
        // SAFETY: forwards the caller's own call.
        self.around_present(chain, || unsafe { original(swap, interval, flags) })
    }

    /// One present of the game's swapchain `chain`, made by `call` (DXGI's Present, or an engine's
    /// own present call that ends in it): the frame's eye kept or its pair published before it, the
    /// next eye chosen after it.
    pub fn around_present<R>(&self, chain: &IDXGISwapChain, call: impl FnOnce() -> R) -> R {
        let _flight = InFlight::enter();
        if let Some(frame_end) = self.state.lock().ok().and_then(|cell| cell.0.as_ref().and_then(|s| s.frame_end)) {
            frame_end(chain);
        }
        let count = self.presents.fetch_add(1, Relaxed) + 1;
        let capturing = self.capture.load(Acquire);
        if capturing {
            if self.attached_at.load(Relaxed) == 0 {
                self.attached_at.store(count, Relaxed);
            }
            self.before_present(chain, count - self.attached_at.load(Relaxed));
        }
        let result = call();
        if capturing {
            self.after_present(count - self.attached_at.load(Relaxed));
        } else if self.eye.swap(-1, AcqRel) != -1 {
            self.finish();
        }
        result
    }

    fn before_present(&self, chain: &IDXGISwapChain, n: u64) {
        let before = self.state.lock().ok().and_then(|cell| cell.0.as_ref().and_then(|s| s.before));
        if let Some(before) = before
            && before(self, chain, n)
        {
            return;
        }
        let mut cell = self.state.locked();
        let Some(state) = cell.0.as_mut() else { return };
        let report_fn = state.report;
        let report = |pose: &mut HeadPose| match report_fn {
            Some(report) => report(self, pose),
            None => self.report_centred(pose),
        };
        let menu = state.menu.is_some_and(|menu| menu());
        let label = self.take_label();
        let labelling = self.frames.locked().labelling;
        let step = match &mut state.publisher {
            Publisher::D3D11(publisher) if menu => {
                self.tally.flat.fetch_add(1, Relaxed);
                monaka_channel::d3d::swapchain_parts(chain).ok().map(|(image, device, context)| publisher.publish_flat(&device, &context, &image, n, &self.head))
            }
            Publisher::D3D11(publisher) => Some(publisher.before_present(chain, n, &self.head, report)),
            Publisher::D3D12 { publisher, queue } => chain.cast::<IDXGISwapChain3>().ok().and_then(|swap| match (queue(&swap), label) {
                // A menu (the game's whole screen): flat to both eyes, on the viewer's screen.
                (Some(queue), _) if menu => {
                    self.tally.flat.fetch_add(1, Relaxed);
                    Some(publisher.show_flat(&swap, &queue))
                }
                // The frame's own label: its eye and pose exactly, whatever the timing.
                (Some(queue), Some(label)) => {
                    if publisher.schedule().shown_eye(n).is_some_and(|s| s != label.eye) {
                        self.tally.disagreed.fetch_add(1, Relaxed);
                    }
                    Some(publisher.show_labelled(&swap, &queue, label.eye, label.pose, &self.head, report))
                }
                // Labelled before but not this frame: its eye is unknown.
                (Some(_), None) if labelling => {
                    self.tally.unlabelled.fetch_add(1, Relaxed);
                    None
                }
                (Some(queue), None) => Some(publisher.before_present(&swap, &queue, n, &self.head, report)),
                (None, _) => {
                    log_first!(3, "present {n}: the game's command queue is not known");
                    None
                }
            }),
        };
        match step {
            Some(Step::Unpaired) => {
                self.unpaired.fetch_add(1, Relaxed);
            }
            Some(Step::Published) => {
                let (pairs, (w, h)) = match &state.publisher {
                    Publisher::D3D11(p) => (p.pairs(), p.size()),
                    Publisher::D3D12 { publisher, .. } => (publisher.pairs(), publisher.size()),
                };
                if pairs == 1 {
                    log!("first pair published at present {n}: {w}x{h} per eye");
                }
            }
            _ => {}
        }
        if n.is_multiple_of(600) {
            let (name, counter) = state.work;
            let work = counter.load(Relaxed);
            let mut mark = self.rate_mark.locked();
            log!("present {n}: {} {name} over the last {} presents (1 per present keeps the eyes in step)", work - mark.0, n - mark.1);
            *mark = (work, n);
        }
    }

    fn after_present(&self, n: u64) {
        if self.finished.load(Acquire) {
            // The stop gave everything back while this present was in the game's own call.
            return;
        }
        let mut cell = self.state.locked();
        let Some(state) = cell.0.as_mut() else { return };
        self.choices.fetch_add(1, AcqRel);
        // Both eyes of a pair are drawn from one head pose, sampled for its left-eye frame (by the
        // camera update instead, with choices taken in turn).
        if !state.in_turn {
            if Schedule::starts_pair(n) {
                let tracked = state.shared.head_tracking || state.shared.synthetic.is_some();
                let mut pose = if tracked { self.head.current().unwrap_or_default() } else { HeadPose::default() };
                if let Some(adjust) = state.adjust_pose {
                    adjust(n, &mut pose);
                }
                state.pair_pose = pose;
            }
            let pose = state.pair_pose;
            match &mut state.publisher {
                Publisher::D3D11(publisher) => publisher.record_pose(n, pose),
                Publisher::D3D12 { publisher, .. } => publisher.record_pose(n, pose),
            }
        }
        let pose = state.pair_pose;
        let after = state.after;
        drop(cell);
        if let Some(after) = after {
            after(self, n, &pose);
        }
        let eye = Schedule::camera_eye(n);
        *self.pending.locked() = Some(Pending { eye, pose });
        self.eye.store(eye as i32, Release);
    }

    /// The end of a run, on the present thread (or the stop's, when the game stopped presenting).
    /// A D3D12 channel is not closed here: closing waits for its copies on the GPU, which would
    /// hold the game's present thread; [`Driver::release`] closes it once the hooks are off.
    pub fn finish(&self) {
        self.finished.store(true, Release);
        self.pending.locked().take();
        let finish = self.state.lock().ok().and_then(|cell| cell.0.as_ref().and_then(|s| s.finish));
        if let Some(finish) = finish {
            finish(self);
        }
        let mut cell = self.state.locked();
        if let Some(Publisher::D3D11(publisher)) = cell.0.as_mut().map(|s| &mut s.publisher) {
            publisher.drop_eyes();
        }
    }

    /// Lets go of the channels (after the hooks are off, on the loader thread).
    pub fn release(&self) {
        let mut cell = self.state.locked();
        if let Some(mut state) = cell.0.take()
            && let Publisher::D3D12 { publisher, .. } = &mut state.publisher
        {
            publisher.close();
        }
        self.head.close();
    }
}

/// Hands eye choices to the camera updates strictly in turn. The game thread's updates and the
/// presents match one to one, but which comes first wobbles: an update can arrive before the
/// present that makes its choice, or after the next one. Taking the choice after the last one used,
/// rather than the newest made, keeps every frame on its own eye through that wobble; only a
/// lasting offset (an update or a present that had no partner) moves it back into step.
struct Sequencer {
    used: u64,
    run: u32,
    run_sign: i64,
}

#[derive(Debug, PartialEq, Eq)]
enum Turn {
    InStep,
    Early,
    Late,
    Resynced,
}

impl Sequencer {
    const NEW: Sequencer = Sequencer { used: 0, run: 0, run_sign: 0 };
    /// Updates in a row offset the same way before the sequence follows the presents again.
    const RESYNC_AFTER: u32 = 8;

    /// The choice (1-based) for the next camera update, `made` choices having been made so far.
    fn next(&mut self, made: u64) -> (u64, Turn) {
        if self.used == 0 {
            self.used = made;
            return (made, Turn::InStep);
        }
        let choice = self.used + 1;
        let offset = (made as i64 - choice as i64).signum();
        if offset != 0 && offset == self.run_sign {
            self.run += 1;
        } else {
            self.run = u32::from(offset != 0);
            self.run_sign = offset;
        }
        if self.run >= Self::RESYNC_AFTER {
            *self = Self { used: made, run: 0, run_sign: 0 };
            return (made, Turn::Resynced);
        }
        self.used = choice;
        (choice, if offset < 0 { Turn::Early } else if offset > 0 { Turn::Late } else { Turn::InStep })
    }
}

impl Default for Driver {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use monaka_core::options::Options;

    #[test]
    fn updates_take_choices_in_turn_through_the_wobble() {
        let mut s = Sequencer::NEW;
        assert_eq!(s.next(1), (1, Turn::InStep));
        assert_eq!(s.next(2), (2, Turn::InStep));
        // Early: a second update before the present making choice 3.
        assert_eq!(s.next(2), (3, Turn::Early));
        // Late: two presents before this update; it still takes the next choice, not the newest.
        assert_eq!(s.next(4), (4, Turn::InStep));
        assert_eq!(s.next(6), (5, Turn::Late));
        assert_eq!(s.next(6), (6, Turn::InStep));
    }

    #[test]
    fn a_lasting_offset_is_followed() {
        let mut s = Sequencer::NEW;
        assert_eq!(s.next(1).0, 1);
        // A present had no camera update: from here every update is one late.
        let mut made = 2;
        let mut last = (0, Turn::InStep);
        for _ in 0..Sequencer::RESYNC_AFTER {
            made += 1;
            last = s.next(made);
        }
        assert_eq!(last, (made, Turn::Resynced));
        assert_eq!(s.next(made + 1), (made + 1, Turn::InStep));
    }

    #[test]
    fn a_frame_is_labelled_by_its_camera() {
        let driver = Driver::new();
        driver.capture.store(true, Release);
        let pose = HeadPose { valid: 1, ..HeadPose::default() };
        driver.camera_written([1.0, 2.0, 3.0], 0, pose);
        driver.camera_written([1.1, 2.0, 3.0], 1, pose);
        assert_eq!(driver.label_frame([1.1, 2.0, 3.001]), Some(1));
        assert_eq!(driver.take_label().map(|l| l.eye), Some(1));
        assert!(driver.take_label().is_none(), "taken once");
        assert_eq!(driver.label_frame([5.0, 2.0, 3.0]), None, "no camera written there");
    }

    #[test]
    fn the_default_report_runs_while_the_state_is_held() {
        // The publisher calls the report inside the state lock: the report must not take it again
        // (that deadlocks the present thread at the first published pair, freezing the game).
        let driver = Driver::new();
        use monaka_core::manifest::{Launch, Manifest, Status};
        let manifest = Manifest {
            id: "test",
            name: "Test",
            status: Status::Experimental,
            executables: &["test.exe"],
            steam_app: 0,
            producer: "monaka_test",
            log: "monaka_test.log",
            options_file: "test-options.txt",
            run_prefix: "test",
            launch: Launch::Manual,
            refused_arguments: &[],
            before: "",
            options: &[],
            licences: &[],
            left_hand_aims: false,
            weapon_ring: false,
        };
        *driver.settings.lock().unwrap() = Some(Shared::from_options(&Options::parse("test_separation=0.3"), &manifest));
        let held = driver.state.lock().unwrap();
        let mut pose = HeadPose::default();
        driver.report_centred(&mut pose);
        assert_eq!(pose.ipd, 0.3);
        drop(held);
    }
}
