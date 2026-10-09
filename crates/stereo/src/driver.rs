//! The alternate-eye present loop, for any game that draws one eye per frame: the part of a
//! producer's `stereo.rs` that had been copied from game to game (Dishonored to Control). The game
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
//! The game's detour is one line: `DRIVER.present(swap, interval, flags, PRESENT_ORIGINAL.get())`.
//! Its camera hook reads [`Driver::pending`] and draws that eye from that pose. A per-game
//! counter of those draws goes in the rate line ([`Install::work`]). A game with more to do at
//! each present (its own publishers for some modes, a live camera to turn, controllers to read)
//! hangs that on [`Install::before`], [`Install::after`] and [`Install::finish`]; one that moves
//! the head's tracking origin (room-scale following) adjusts the pose in [`Install::adjust_pose`].

use crate::alternate::{AlternatePublisher, Step};
use crate::alternate12::AlternatePublisher12;
use crate::present::PresentFn;
use monaka_channel::pose::{HeadSource, PoseChannel, Synthetic};
use monaka_core::alternate::Schedule;
use monaka_core::options::Shared;
use monaka_core::protocol::HeadPose;
use monaka_hook::InFlight;
use monaka_producer::{log, log_first};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, AtomicUsize, Ordering::*};
use std::sync::Mutex;
use windows::Win32::Graphics::Direct3D12::ID3D12CommandQueue;
use windows::Win32::Graphics::Dxgi::{IDXGISwapChain, IDXGISwapChain3};
use windows::core::{HRESULT, Interface};

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
    /// The fence channel, copies on the game's direct queue (`queue`, looked up at each present:
    /// the game may make it after the hooks go in).
    D3D12 { publisher: AlternatePublisher12, queue: fn() -> Option<ID3D12CommandQueue> },
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
    /// The head pose each pair is drawn from, adjusted before it is recorded and announced (a
    /// tracking origin moved under the player).
    pub adjust_pose: Option<fn(&mut HeadPose)>,
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
    adjust_pose: Option<fn(&mut HeadPose)>,
    finish: Option<fn(&Driver)>,
    pair_pose: HeadPose,
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
        }
    }

    /// At start, before the hooks go in.
    pub fn install(&self, install: Install) {
        let synthetic: Option<Synthetic> = install.shared.synthetic_head().map(|pose| Box::new(move || pose) as Synthetic);
        self.head.open(install.poses, synthetic);
        *self.settings.lock().unwrap_or_else(|e| e.into_inner()) = Some(install.shared);
        let mut cell = self.state.lock().unwrap_or_else(|e| e.into_inner());
        cell.0 = Some(State {
            shared: install.shared,
            publisher: install.publisher,
            frame_end: install.frame_end,
            report: install.report,
            work: install.work,
            before: install.before,
            after: install.after,
            adjust_pose: install.adjust_pose,
            finish: install.finish,
            pair_pose: HeadPose::default(),
        });
    }

    /// Starts choosing eyes at the next present (the hooks are in).
    pub fn start(&self) {
        self.finished.store(false, Release);
        self.capture.store(true, Release);
    }

    /// Whether the run's end has given everything back ([`Driver::finish`] ran).
    pub fn finished(&self) -> bool {
        self.finished.load(Acquire)
    }

    /// The D3D11 publisher, for a game's own publishing path (none on D3D12 or before install).
    pub fn with_publisher11<R>(&self, f: impl FnOnce(&mut AlternatePublisher) -> R) -> Option<R> {
        let mut cell = self.state.lock().unwrap_or_else(|e| e.into_inner());
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
        *self.pending.lock().unwrap_or_else(|e| e.into_inner())
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
        *self.settings.lock().unwrap_or_else(|e| e.into_inner())
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

    /// Pairs published, failures, presents, unpaired right eyes: the tail of a run's summary.
    pub fn summary(&self) -> String {
        let cell = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let (pairs, failures) = match cell.0.as_ref().map(|s| &s.publisher) {
            Some(Publisher::D3D11(p)) => (p.pairs(), p.failures()),
            Some(Publisher::D3D12 { publisher, .. }) => (publisher.pairs(), publisher.failures()),
            None => (0, 0),
        };
        format!(
            "{} presents, {pairs} pairs published, {failures} failures, {} unpaired",
            self.presents.load(Relaxed).saturating_sub(self.attached_at.load(Relaxed)),
            self.unpaired.load(Relaxed)
        )
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
        // SAFETY: forwards the game's own call.
        let result = unsafe { original(swap, interval, flags) };
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
        let mut cell = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(state) = cell.0.as_mut() else { return };
        let report_fn = state.report;
        let report = |pose: &mut HeadPose| match report_fn {
            Some(report) => report(self, pose),
            None => self.report_centred(pose),
        };
        let step = match &mut state.publisher {
            Publisher::D3D11(publisher) => Some(publisher.before_present(chain, n, &self.head, report)),
            Publisher::D3D12 { publisher, queue } => match (chain.cast::<IDXGISwapChain3>().ok(), queue()) {
                (Some(swap), Some(queue)) => Some(publisher.before_present(&swap, &queue, n, &self.head, report)),
                (_, None) => {
                    log_first!(3, "present {n}: the game's command queue is not known");
                    None
                }
                _ => None,
            },
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
            let mut mark = self.rate_mark.lock().unwrap_or_else(|e| e.into_inner());
            log!("present {n}: {} {name} over the last {} presents (1 per present keeps the eyes in step)", work - mark.0, n - mark.1);
            *mark = (work, n);
        }
    }

    fn after_present(&self, n: u64) {
        if self.finished.load(Acquire) {
            // The stop gave everything back while this present was in the game's own call.
            return;
        }
        let mut cell = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(state) = cell.0.as_mut() else { return };
        // Both eyes of a pair are drawn from one head pose, sampled for its left-eye frame.
        if Schedule::starts_pair(n) {
            let tracked = state.shared.head_tracking || state.shared.synthetic.is_some();
            let mut pose = if tracked { self.head.current().unwrap_or_default() } else { HeadPose::default() };
            if let Some(adjust) = state.adjust_pose {
                adjust(&mut pose);
            }
            state.pair_pose = pose;
        }
        let pose = state.pair_pose;
        match &mut state.publisher {
            Publisher::D3D11(publisher) => publisher.record_pose(n, pose),
            Publisher::D3D12 { publisher, .. } => publisher.record_pose(n, pose),
        }
        let after = state.after;
        drop(cell);
        if let Some(after) = after {
            after(self, n, &pose);
        }
        let eye = Schedule::camera_eye(n);
        *self.pending.lock().unwrap_or_else(|e| e.into_inner()) = Some(Pending { eye, pose });
        self.eye.store(eye as i32, Release);
    }

    /// The end of a run, on the present thread (or the stop's, when the game stopped presenting).
    /// A D3D12 channel is not closed here: closing waits for its copies on the GPU, which would
    /// hold the game's present thread; [`Driver::release`] closes it once the hooks are off.
    pub fn finish(&self) {
        self.finished.store(true, Release);
        self.pending.lock().unwrap_or_else(|e| e.into_inner()).take();
        let finish = self.state.lock().ok().and_then(|cell| cell.0.as_ref().and_then(|s| s.finish));
        if let Some(finish) = finish {
            finish(self);
        }
        let mut cell = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(Publisher::D3D11(publisher)) = cell.0.as_mut().map(|s| &mut s.publisher) {
            publisher.drop_eyes();
        }
    }

    /// Lets go of the channels (after the hooks are off, on the loader thread).
    pub fn release(&self) {
        let mut cell = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(mut state) = cell.0.take()
            && let Publisher::D3D12 { publisher, .. } = &mut state.publisher
        {
            publisher.close();
        }
        self.head.close();
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
    fn the_default_report_runs_while_the_state_is_held() {
        // The publisher calls the report inside the state lock: the report must not take it again
        // (it once did, and the first published pair froze the game).
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
