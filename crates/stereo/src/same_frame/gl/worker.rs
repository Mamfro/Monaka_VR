//! Frame warping's D3D11 work on a thread of its own, so the game's render thread (often what
//! holds an OpenGL game's frame rate) only copies the frame out. At `SwapBuffers` the render thread
//! copies the eye and its depth into one of [`SLOTS`] ring slots (`monaka_gl::Bridge::capture_eye`)
//! and hands them over ([`Job`]); this thread upscales the eye (FSR), captures it for the warp and
//! publishes the pair, owning the D3D11 device's immediate context. A slot goes back to the render
//! thread once its job's work is submitted; a whole frame (one going flat) is handed over without a
//! slot, and the render thread waits for its job. With frame generation the real pair behind each
//! generated pair waits here for its time: the thread wakes for it between jobs.

use super::{DESCRIBED_AT, GlFsr, GlShot, GlWarp, describe_warp};
use crate::afw::Shot;
use crate::afw::d3d11::{Afw11, At};
use crate::driver::{Flat, Pending};
use crate::same_frame::PAIRS;
use crate::same_frame::timing::Stage;
use crate::upscaler::fsr11::{fov_vertical, frame_time_and_cut};
use monaka_channel::{ChannelName, d3d};
use monaka_fsr11::{EyeFrame, Fsr11, Settings};
use monaka_producer::log;
use monaka_warp::{EyeView, Scaler};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::*};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, SyncSender, sync_channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11DeviceContext, ID3D11Multithread, ID3D11Texture2D};
use windows::core::Interface;

/// Ring slots the render thread copies eyes into: one being copied while others wait or are read.
pub(super) const SLOTS: usize = 3;
/// How long the render thread waits for a slot (the warp thread behind) before it drops the frame.
const SLOT_WAIT: Duration = Duration::from_millis(2);
/// How long the render thread waits for a whole frame's job.
const WHOLE_WAIT: Duration = Duration::from_millis(50);

/// Pairs this thread published, flat ones among them, and ones warped from a frame.
pub(super) static PUBLISHED: AtomicU64 = AtomicU64::new(0);
pub(super) static PUBLISHED_FLAT: AtomicU64 = AtomicU64::new(0);
pub(super) static WARPED: AtomicU64 = AtomicU64::new(0);

/// One frame for the warp thread.
pub(super) struct Job {
    pub frame: u64,
    /// The ring slot `image` and `depth` are in, given back when done; none for a whole frame.
    pub slot: Option<usize>,
    /// The eye at its render size, or the whole frame when it goes flat.
    pub image: ID3D11Texture2D,
    /// The eye's depth, when the frame is one eye's and its depth was copied.
    pub depth: Option<ID3D11Texture2D>,
    pub shot: Option<GlShot>,
    pub flat: Option<Flat>,
    /// The eye's size as shown (FSR's output size).
    pub display: (u32, u32),
    /// For the log: the most built frames seen waiting, and the depth copies that failed.
    pub waiting_max: usize,
    pub depth_failures: u64,
}

/// A job and, for a whole frame, where to say it is done.
struct Sent(Job, Option<SyncSender<()>>);
// SAFETY: the textures go to the warp thread and are used only there until the job is done (the
// slot's back to the render thread only after); D3D11 objects are free-threaded for reference counts.
unsafe impl Send for Sent {}

/// The D3D11 device and its immediate context, used only on the warp thread once it started.
struct Gpu(ID3D11Device, ID3D11DeviceContext);
// SAFETY: the render thread hands them over and no longer uses the context (the bridge uses only
// the device, which is free-threaded).
unsafe impl Send for Gpu {}

/// The render thread's end: the ring and the thread.
pub(super) struct Worker {
    sender: Option<Sender<Sent>>,
    thread: Option<JoinHandle<()>>,
    busy: Arc<[AtomicBool; SLOTS]>,
    next: usize,
    broken: Arc<AtomicBool>,
    /// Frames dropped with no slot free.
    pub dropped: u64,
}

impl Worker {
    /// Starts the thread, which takes `context` (the render thread no longer uses it).
    pub fn start(device: ID3D11Device, context: ID3D11DeviceContext, warp: GlWarp, channel: ChannelName) -> Result<Self, String> {
        // The bridge's interop and the warp thread share the device: the runtime keeps the context's
        // calls whole.
        let multithread: ID3D11Multithread = context.cast().map_err(|e| format!("no ID3D11Multithread: {e}"))?;
        // SAFETY: a flag on our own context.
        let _ = unsafe { multithread.SetMultithreadProtected(true) };
        let (sender, receiver) = std::sync::mpsc::channel();
        let busy: Arc<[AtomicBool; SLOTS]> = Arc::new([const { AtomicBool::new(false) }; SLOTS]);
        let broken = Arc::new(AtomicBool::new(false));
        let gpu = Gpu(device, context);
        let (thread_busy, thread_broken) = (busy.clone(), broken.clone());
        let thread = std::thread::Builder::new()
            .name("monaka warp".into())
            .spawn(move || {
                let gpu = gpu;
                Warping::new(gpu.0, gpu.1, warp, channel).run(&receiver, &thread_busy, &thread_broken);
            })
            .map_err(|e| format!("warp thread: {e}"))?;
        log!("frame warping on its own thread ({SLOTS} slots)");
        Ok(Self { sender: Some(sender), thread: Some(thread), busy, next: 0, broken, dropped: 0 })
    }

    /// Whether the thread stopped for good (frame warping unavailable).
    pub fn broken(&self) -> bool {
        self.broken.load(Acquire)
    }

    /// The next ring slot, taken; none when it is still read after a short wait.
    pub fn slot(&mut self) -> Option<usize> {
        let slot = self.next;
        let started = Instant::now();
        while self.busy[slot].load(Acquire) {
            if started.elapsed() > SLOT_WAIT {
                self.dropped += 1;
                return None;
            }
            std::thread::yield_now();
        }
        self.busy[slot].store(true, Release);
        self.next = (slot + 1) % SLOTS;
        Some(slot)
    }

    /// A slot taken but not handed over (its copy failed).
    pub fn give_back(&self, slot: usize) {
        self.busy[slot].store(false, Release);
    }

    /// Hands `job` over; a whole frame (no slot) is waited for, since its texture is reused.
    pub fn send(&mut self, job: Job) {
        let Some(sender) = self.sender.as_ref() else { return };
        let slot = job.slot;
        if slot.is_some() {
            if sender.send(Sent(job, None)).is_err()
                && let Some(slot) = slot
            {
                self.busy[slot].store(false, Release);
            }
            return;
        }
        let (done, wait) = sync_channel(1);
        if sender.send(Sent(job, Some(done))).is_ok() {
            let _ = wait.recv_timeout(WHOLE_WAIT);
        }
    }

    /// Stops the thread once it finished what it was given.
    pub fn stop(&mut self) {
        self.sender = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The warp thread's own state.
struct Warping {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    warp: GlWarp,
    channel: ChannelName,
    afw: Option<Afw11>,
    /// FSR's upscaler (made at the first frame, again at another size), each eye's last upscaled
    /// frame (when, the camera's position), and its counts.
    fsr: Option<Fsr11>,
    fsr_last: [Option<(Instant, [f32; 3])>; 2],
    fsr_broken: bool,
    upscaled: u64,
    upscale_failures: u64,
    /// Resamples a frame to the eye size (a flat one, or one FSR did not upscale).
    scaler: Option<Scaler>,
    window_start: Option<Instant>,
    window_pairs: u64,
    failures: u64,
}

impl Warping {
    fn new(device: ID3D11Device, context: ID3D11DeviceContext, warp: GlWarp, channel: ChannelName) -> Self {
        Self {
            device,
            context,
            warp,
            channel,
            afw: None,
            fsr: None,
            fsr_last: [None, None],
            fsr_broken: false,
            upscaled: 0,
            upscale_failures: 0,
            scaler: None,
            window_start: None,
            window_pairs: 0,
            failures: 0,
        }
    }

    fn run(mut self, jobs: &Receiver<Sent>, busy: &[AtomicBool; SLOTS], broken: &AtomicBool) {
        loop {
            // The next job, or the waiting real pair's time first (frame generation).
            let received = match self.afw.as_ref().and_then(Afw11::due) {
                Some(at) => jobs.recv_timeout(at.saturating_duration_since(Instant::now())),
                None => jobs.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            let Sent(job, done) = match received {
                Ok(sent) => sent,
                Err(RecvTimeoutError::Timeout) => {
                    if let Some(afw) = self.afw.as_mut() {
                        afw.send_due(&self.device, &self.context, PAIRS.head());
                    }
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => break,
            };
            let slot = job.slot;
            if !self.job(job) {
                broken.store(true, Release);
            }
            // SAFETY: submits this thread's work on its own context.
            unsafe { self.context.Flush() };
            if let Some(slot) = slot {
                busy[slot].store(false, Release);
            }
            if let Some(done) = done {
                let _ = done.send(());
            }
        }
        if let Some(afw) = self.afw.take() {
            log!("{}", afw.report_line());
        }
        if self.fsr.take().is_some() {
            log!("FSR: {} frames upscaled, {} failed", self.upscaled, self.upscale_failures);
        }
    }

    /// One frame: false when frame warping is unavailable.
    fn job(&mut self, job: Job) -> bool {
        let (device, context) = (self.device.clone(), self.context.clone());
        if self.afw.is_none() {
            match Afw11::with_rules(&device, self.channel.clone(), self.warp.rules) {
                Ok(mut afw) => {
                    afw.show_sources(self.warp.sources);
                    if let Some(framegen) = self.warp.framegen {
                        afw.frame_generation(framegen);
                    }
                    self.afw = Some(afw);
                }
                Err(e) => {
                    log!("frame warping unavailable: {e}");
                    return false;
                }
            }
        }
        let mut shown = match job.flat {
            Some(_) => match self.scaled(&job.image, job.display) {
                Some(scaled) => scaled,
                None => {
                    self.failures += 1;
                    return true;
                }
            },
            None => job.image.clone(),
        };
        let shot = job.shot.filter(|_| job.flat.is_none());
        if let (Some(GlShot { shot, jitter }), Some(depth)) = (shot, job.depth.as_ref()) {
            let posed = WARPED.load(Relaxed);
            if posed + 1 == DESCRIBED_AT || posed == DESCRIBED_AT {
                describe_warp(&device, &context, depth, &job.image, &shot);
            }
            match self.warp.fsr {
                Some(fsr) => {
                    let upscaling = Instant::now();
                    let upscaled = self.upscale(fsr, &shot, &job.image, depth, jitter, job.display);
                    PAIRS.time(Stage::GlUpscale, upscaling.elapsed());
                    match upscaled {
                        Some(upscaled) => {
                            let warping = Instant::now();
                            self.afw.as_mut().expect("made above").capture_jittered(&context, &shot, &upscaled, depth, jitter);
                            PAIRS.time(Stage::GlWarp, warping.elapsed());
                            shown = upscaled;
                        }
                        None => shown = self.scaled(&job.image, job.display).unwrap_or(shown),
                    }
                }
                None => {
                    let warping = Instant::now();
                    self.afw.as_mut().expect("made above").capture(&context, &shot, &job.image, depth);
                    PAIRS.time(Stage::GlWarp, warping.elapsed());
                }
            }
        }
        // A frame not upscaled goes out at the eye size all the same.
        if self.warp.fsr.is_some() && d3d::texture_desc(&shown).Width != job.display.0 {
            shown = self.scaled(&shown, job.display).unwrap_or(shown);
        }
        let report = PAIRS.settings();
        let label = job.shot.map(|s| Pending { eye: s.shot.eye, pose: s.shot.pose });
        let at = At { head: PAIRS.head(), hud_rects: [[0.0; 4]; 2], flat: job.flat.is_some(), label, present: job.frame };
        let afw = self.afw.as_mut().expect("made above");
        let publishing = Instant::now();
        let published = afw.publish(&device, &context, &shown, &at, |pose| {
            if let Some(settings) = report {
                settings.report(pose);
            }
        });
        PAIRS.time(Stage::GlWarp, publishing.elapsed());
        PAIRS.tally(published);
        if published {
            PUBLISHED.fetch_add(1, Relaxed);
            PUBLISHED_FLAT.fetch_add(u64::from(job.flat.is_some()), Relaxed);
            WARPED.fetch_add(u64::from(job.shot.is_some()), Relaxed);
            self.window_pairs += 1;
        }
        let start = *self.window_start.get_or_insert_with(Instant::now);
        let elapsed = start.elapsed().as_secs_f64();
        if elapsed >= 10.0 {
            log!(
                "{:.1} pairs/s over {elapsed:.0} s ({} frames, {} failures, {} warped from a frame, {} depth copies failed, at most {} built frames waiting)",
                self.window_pairs as f64 / elapsed,
                super::frames(),
                self.failures,
                WARPED.load(Relaxed),
                job.depth_failures,
                job.waiting_max,
            );
            log!("{}", afw.report_line());
            let timing = PAIRS.timing_report();
            if !timing.is_empty() {
                log!("{timing}");
            }
            if let Some(rate) = PAIRS.settings().and_then(|s| s.rate) {
                rate();
            }
            self.window_start = Some(Instant::now());
            self.window_pairs = 0;
        }
        true
    }

    /// `image` resampled to `size` (a flat frame, or one FSR did not upscale).
    fn scaled(&mut self, image: &ID3D11Texture2D, size: (u32, u32)) -> Option<ID3D11Texture2D> {
        if self.scaler.is_none() {
            self.scaler = Scaler::new(&self.device).ok();
        }
        self.scaler.as_mut()?.run(&self.context, image, size).ok().cloned()
    }

    /// The eye `colour` (render size) with its `depth`, drawn as `shot` with `jitter`, upscaled to
    /// `display`; none when FSR is unavailable or failed this frame.
    fn upscale(&mut self, fsr: GlFsr, shot: &Shot, colour: &ID3D11Texture2D, depth: &ID3D11Texture2D, jitter: [f32; 2], display: (u32, u32)) -> Option<ID3D11Texture2D> {
        if self.fsr_broken {
            return None;
        }
        let desc = d3d::texture_desc(colour);
        let render = (desc.Width, desc.Height);
        if self.fsr.as_ref().is_none_or(|made| made.render() != render || made.display() != display) {
            // With frame generation, its provider is loaded beside the upscaler's, before the loader.
            let api = match self.warp.framegen {
                Some(_) => monaka_fsr11::load_with_framegen(fsr.dlls).or_else(|_| monaka_fsr11::load(fsr.dlls)),
                None => monaka_fsr11::load(fsr.dlls),
            };
            let made = api.and_then(|api| {
                Fsr11::new(&self.device, api, Settings { render, display, sharpness: fsr.sharpness, jitter_sign: fsr.jitter_sign, depth_flags: fsr.depth_flags })
            });
            match made {
                Ok(made) => {
                    log!("FSR {}: {}x{} upscaled to {}x{}, {} jitter phases", made.version(), render.0, render.1, display.0, display.1, made.phases());
                    self.fsr = Some(made);
                    self.fsr_last = [None, None];
                }
                Err(e) => {
                    log!("FSR unavailable: {e}; the eye is resampled instead");
                    self.fsr_broken = true;
                    return None;
                }
            }
        }
        let rendered = &shot.rendered;
        let camera = rendered.camera;
        let position = [camera[3], camera[7], camera[11]];
        let now = Instant::now();
        let eye = shot.eye.min(1);
        let (frame_time_ms, cut) = frame_time_and_cut(self.fsr_last[eye], now, position);
        self.fsr_last[eye] = Some((now, position));
        let frame = EyeFrame {
            color: colour,
            depth,
            object: None,
            view: EyeView { camera, projection: rendered.projection },
            jitter,
            // The near plane in the camera's units: nearness = a * depth + b, a = near / P[11].
            near: rendered.mapping.a * rendered.projection[11],
            fov_vertical: fov_vertical(&rendered.projection),
            frame_time_ms,
            reset: cut,
        };
        match self.fsr.as_mut()?.upscale(&self.context, eye, &frame) {
            Ok(image) => {
                self.upscaled += 1;
                Some(image)
            }
            Err(e) => {
                if self.upscale_failures < 5 {
                    log!("FSR upscale failed: {e}");
                }
                self.upscale_failures += 1;
                None
            }
        }
    }
}
