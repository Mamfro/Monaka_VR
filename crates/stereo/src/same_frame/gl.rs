//! Same-frame pairs from an OpenGL game: both eyes drawn side by side in one frame (the engine's own
//! stereo layout). At the game's `SwapBuffers` (its present entry calls [`present`] on the render
//! thread, its GL context current) the back buffer is copied onto a D3D11 device of the loop's own
//! (`monaka_gl`) and published on the fence channel, each eye straight from its half, the pose record of
//! the frame's ticket first. A frame that goes flat (a full-screen video the engine draws over the
//! whole window, not per eye) goes whole, at the eye size, to both eyes without a pose. Without the
//! side-by-side layout the whole frame goes to both eyes.
//!
//! With frame warping ([`GlWarp`]) the game draws one eye per frame into a part of its window: that
//! part and the game's scene depth under it are copied out, and the frame goes to `crate::afw::d3d11`,
//! which publishes the eye with the other eye warped from it (its own channel producer, on the same
//! channel). With FSR ([`GlFsr`]) that part is smaller (the render size) and drawn with a jittered
//! projection: it is upscaled to the eye size (`monaka_fsr11`, motion from its depth and the eye's
//! cameras) before the warp, which takes the upscaled image with the jittered depth. With frame
//! generation ([`GlWarp::framegen`]) `crate::afw::d3d11` adds in-between pairs (FSR frame generation:
//! each warped pair's generated pair at once, the real pair half a frame later). All but the copy
//! runs on a thread of its own ([`worker`]): the render thread is what holds such a game's frame
//! rate.

mod worker;

use super::PAIRS;
use super::timing::Stage;
use crate::afw::{Rules, Shot};
use std::path::Path;
use crate::driver::Flat;
use monaka_gl::DepthSource;
use worker::{Job, Worker};
use monaka_channel::fence11::LazyProducer11;
use monaka_channel::{ChannelName, d3d};
use monaka_core::sync::Locked;
use monaka_gl::Bridge;
use monaka_producer::{log, wait_until};
use monaka_warp::Scaler;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::*};
use std::time::Instant;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D11::{D3D11_BOX, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D};
use windows::Win32::Graphics::Gdi::{HDC, WindowFromDC};
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;

/// Frame warping for an OpenGL game drawing one eye per frame: what it tells the loop at each
/// `SwapBuffers`.
#[derive(Clone, Copy)]
pub struct GlWarp {
    /// The frame at this `SwapBuffers`: its one eye, the pose and cameras it was drawn with (and the
    /// jitter its projection was shifted by), taken in step with the loop's tickets (one per built
    /// frame); none for a frame that is not one eye's (a menu, the game paused).
    pub shot: fn() -> Option<GlShot>,
    /// The game's scene depth now, behind the frame being presented.
    pub depth: fn() -> Option<GlDepth>,
    /// Where the eye is in the frame (x, y from the top-left, width, height: fractions). The depth
    /// texture is taken over the same part when it covers the whole frame, else whole (an engine
    /// that renders each view into a target of the view's size: [`depth_region`]).
    pub region: [f32; 4],
    /// The warps' rules (`crate::afw::Rules`).
    pub rules: Rules,
    /// A debug view in the warped eye instead of its image (`monaka_warp::Reprojector::sources`:
    /// 0 none, 1 where each pixel came from, 2 its nearness).
    pub sources: u32,
    /// FSR: the eye drawn smaller (at `region`) and upscaled to the eye size before the warp.
    pub fsr: Option<GlFsr>,
    /// Frame generation: in-between pairs of the warped pairs (`crate::afw::d3d11::Afw11::frame_generation`).
    pub framegen: Option<crate::afw::d3d11::FrameGeneration>,
}

/// A frame's shot and the jitter (render pixels, x right, y down) its projection was shifted by
/// (0 without FSR); the shot's projection is the unjittered one.
#[derive(Clone, Copy, Debug)]
pub struct GlShot {
    pub shot: Shot,
    pub jitter: [f32; 2],
}

/// FSR for an OpenGL game drawing one eye per frame.
#[derive(Clone, Copy, Debug)]
pub struct GlFsr {
    /// The eye's size the frame is upscaled to (fractions of the frame's width and height).
    pub display: [f32; 2],
    /// RCAS sharpening 0..1, or none.
    pub sharpness: Option<f32>,
    /// 1 or -1: whether the dispatch is told the content shift or its opposite.
    pub jitter_sign: f32,
    /// The game's depth convention (`monaka_ffx::upscale::create` flags).
    pub depth_flags: u32,
    /// Where AMD's DLLs are.
    pub dlls: &'static Path,
}

/// The upscaled eye's size in a `width` x `height` frame for `display` (fractions).
pub fn display_size(width: u32, height: u32, display: [f32; 2]) -> (u32, u32) {
    region_size(width, height, [0.0, 0.0, display[0], display[1]])
}

/// A GL depth texture: its name, size and whether it is multisampled (its sample 0 is read).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GlDepth {
    pub texture: u32,
    pub width: u32,
    pub height: u32,
    pub multisampled: bool,
}

/// The part of a depth texture `depth_width` wide holding the eye at `region` of a frame
/// `frame_width` wide: the same part when the texture covers the frame (at least three quarters of
/// its width: a render scale below one leaves it smaller), else the whole texture (rendered at the
/// view's own size).
pub fn depth_region(region: [f32; 4], frame_width: u32, depth_width: u32) -> [f32; 4] {
    if depth_width as f32 >= frame_width as f32 * 0.75 { region } else { [0.0, 0.0, 1.0, 1.0] }
}

/// The eye's size in a `width` x `height` frame for `region` (fractions).
pub fn region_size(width: u32, height: u32, region: [f32; 4]) -> (u32, u32) {
    (((width as f32 * region[2]).round() as u32).max(1), ((height as f32 * region[3]).round() as u32).max(1))
}

/// The warped frames whose depth and cameras go beside the log once a run ([`describe_warp`]: this one
/// and the next, one of each eye when they take turns), when the run has settled.
const DESCRIBED_AT: u64 = 200;

/// The last frame's size at `SwapBuffers` (width and height packed; 0 before the first).
static FRAME_SIZE: AtomicU64 = AtomicU64::new(0);

/// The size of the game's frames at `SwapBuffers` (its window's client area), once one was seen: a
/// game drawing an eye into part of it knows that part's size in pixels.
pub fn frame_size() -> Option<(u32, u32)> {
    let packed = FRAME_SIZE.load(Relaxed);
    (packed != 0).then_some(((packed >> 32) as u32, packed as u32))
}

/// Set while the render thread holds GL objects; the render thread clears it after freeing them.
static ACTIVE: AtomicBool = AtomicBool::new(false);
/// Frames seen at `SwapBuffers` while publishing.
static FRAMES: AtomicU64 = AtomicU64::new(0);


struct State {
    /// Each half of the frame is one eye.
    side_by_side: bool,
    /// Frame warping: one eye per frame, the other warped ([`GlWarp`]).
    warp: Option<GlWarp>,
    channel: ChannelName,
    /// Frame warping's thread, started at the first frame.
    worker: Option<Worker>,
    /// The warped frames' depth copies that failed.
    depth_failures: u64,
    bridge: Option<Bridge>,
    context: Option<ID3D11DeviceContext>,
    /// Why the last frame went flat, if it did.
    flat: Option<Flat>,
    /// Resamples a flat frame to the eye size, made at the first one.
    scaler: Option<Scaler>,
    writer: LazyProducer11,
    /// Set after a failure that retrying every frame would not fix.
    broken: bool,
    pairs: u64,
    /// Pairs that went to the flat screen.
    flat_pairs: u64,
    failures: u64,
    window_start: Option<Instant>,
    window_pairs: u64,
    /// Pairs published with a pose record, and the most built frames seen waiting at a swap.
    posed: u64,
    waiting_max: usize,
}

struct Cell(Option<State>);
// SAFETY: the state is only used under the mutex, and its GL objects only on the render thread
// (the bridge is made, used and released inside the game's SwapBuffers).
unsafe impl Send for Cell {}

static STATE: Mutex<Cell> = Mutex::new(Cell(None));

pub(crate) fn install(channel: ChannelName, side_by_side: bool, warp: Option<GlWarp>) {
    if warp.is_some() {
        log!("frame warping: one eye per frame, the other warped from it with its depth");
    }
    let state = State {
        side_by_side,
        warp,
        channel: channel.clone(),
        worker: None,
        depth_failures: 0,
        bridge: None,
        context: None,
        flat: None,
        scaler: None,
        writer: LazyProducer11::new(channel),
        broken: false,
        pairs: 0,
        flat_pairs: 0,
        failures: 0,
        window_start: None,
        window_pairs: 0,
        posed: 0,
        waiting_max: 0,
    };
    STATE.locked().0 = Some(state);
}

/// The frames seen at `SwapBuffers` while publishing.
pub fn frames() -> u64 {
    FRAMES.load(Relaxed)
}

/// At the game's `SwapBuffers` (`dc` its device context), before the game's own: the frame's pair
/// published while the loop publishes; after the stop, the GL objects freed (on this thread, with
/// the game's context current).
pub fn present(dc: HDC) {
    if PAIRS.publishing() {
        ACTIVE.store(true, Release);
        let started = Instant::now();
        if let Some(state) = STATE.locked().0.as_mut() {
            state.frame(dc);
        }
        PAIRS.time(Stage::GlPresent, started.elapsed());
    } else if ACTIVE.load(Acquire) {
        release_here();
    }
}

/// The render thread frees the GL objects at its next `SwapBuffers`, once the loop stopped.
pub(crate) fn give_back() {
    wait_until(2000, || !ACTIVE.load(Acquire));
}

/// After the hooks are out: what the render thread never came back to free is left to the game's
/// context (leaked rather than freed off its thread).
pub(crate) fn release() {
    if ACTIVE.load(Acquire) {
        log!("no SwapBuffers while stopping; the GL objects stay with the game's context");
        if let Some(mut state) = STATE.locked().0.take() {
            if let Some(mut worker) = state.worker.take() {
                worker.stop();
            }
            std::mem::forget(state.bridge.take());
        }
    }
}

/// Frees the GL objects; called on the render thread with the game's context current.
fn release_here() {
    if let Some(state) = STATE.locked().0.as_mut() {
        // The warp thread finishes first: the bridge's textures are what it reads.
        if let Some(mut worker) = state.worker.take() {
            worker.stop();
            log!("frame warping: {} depth copies failed, {} frames dropped with the warp thread behind", state.depth_failures, worker.dropped);
        }
        if let Some(bridge) = state.bridge.take() {
            bridge.release();
        }
        state.scaler = None;
        state.context = None;
        let (pairs, flat_pairs) = (state.pairs + worker::PUBLISHED.load(Relaxed), state.flat_pairs + worker::PUBLISHED_FLAT.load(Relaxed));
        log!("capture released after {} frames, {pairs} pairs ({flat_pairs} flat), {} failures", frames(), state.failures);
    }
    ACTIVE.store(false, Release);
}

fn client_size(dc: HDC) -> Option<(u32, u32)> {
    // SAFETY: plain queries on the game's own device context and window.
    unsafe {
        let window = WindowFromDC(dc);
        if window.is_invalid() {
            return None;
        }
        let mut rect = RECT::default();
        GetClientRect(window, &mut rect).ok()?;
        let (w, h) = ((rect.right - rect.left) as u32, (rect.bottom - rect.top) as u32);
        monaka_contract::protocol::dimensions_ok(w, h).then_some((w, h))
    }
}

/// The eye size of a frame of `size`: half its width side by side, else all of it.
pub(crate) fn eye_size(size: (u32, u32), side_by_side: bool) -> (u32, u32) {
    if side_by_side { (size.0 / 2, size.1) } else { size }
}

impl State {
    fn fail(&mut self, why: impl std::fmt::Display) {
        if self.failures < 5 {
            log!("frame {}: {why}", frames());
        }
        self.failures += 1;
    }

    fn frame(&mut self, dc: HDC) {
        let frame = FRAMES.fetch_add(1, Relaxed) + 1;
        PAIRS.present();
        if self.broken {
            return;
        }
        if self.bridge.is_none() {
            let made = d3d::create_device(None).map_err(|e| e.to_string()).and_then(|(device, context, _)| Bridge::new(&device).map(|bridge| (bridge, context)).map_err(|e| e.to_string()));
            match made {
                Ok((bridge, context)) => {
                    log!("GL interop open on the render thread");
                    self.bridge = Some(bridge);
                    self.context = Some(context);
                }
                Err(e) => {
                    log!("no GL interop: {e}");
                    self.broken = true;
                    return;
                }
            }
        }
        let Some((width, height)) = client_size(dc) else { return self.fail("no window size") };
        FRAME_SIZE.store(u64::from(width) << 32 | u64::from(height), Relaxed);
        let (eye_width, eye_height) = eye_size((width, height), self.side_by_side);
        if let Some(warp) = self.warp {
            return self.warped(frame, warp, (width, height));
        }
        let (Some(bridge), Some(context)) = (self.bridge.as_mut(), self.context.clone()) else { return };
        let device = bridge.device().clone();
        let copying = Instant::now();
        let image = match bridge.capture(0, width, height) {
            Ok(texture) => texture.clone(),
            Err(e) => return self.fail(format!("capture: {e}")),
        };
        PAIRS.time(Stage::GlColour, copying.elapsed());

        // A frame the engine draws over the whole window (a video) is not per eye: it goes flat.
        let flat = if self.side_by_side { PAIRS.flat_now(PAIRS.settings().and_then(|s| s.flat)) } else { None };
        if flat != self.flat {
            match flat {
                Some(reason) => log!("frame {frame}: {}: whole frames to the flat screen", reason.label()),
                None => log!("frame {frame}: eyes again"),
            }
            self.flat = flat;
        }
        // Each eye: a texture and its region (each the eye size).
        let whole = D3D11_BOX { left: 0, top: 0, front: 0, right: eye_width, bottom: eye_height, back: 1 };
        let eyes: [(ID3D11Texture2D, D3D11_BOX); 2] = match flat {
            Some(_) => {
                let scaler = match self.scaler.as_mut() {
                    Some(scaler) => scaler,
                    None => match Scaler::new(&device) {
                        Ok(made) => self.scaler.insert(made),
                        Err(e) => return self.fail(format!("flat frame scaler: {e}")),
                    },
                };
                match scaler.run(&context, &image, (eye_width, eye_height)) {
                    Ok(scaled) => [(scaled.clone(), whole), (scaled.clone(), whole)],
                    Err(e) => return self.fail(format!("flat frame: {e}")),
                }
            }
            None if self.side_by_side => [0, 1].map(|eye| {
                let left = eye * eye_width;
                (image.clone(), D3D11_BOX { left, right: left + eye_width, ..whole })
            }),
            None => [(image.clone(), whole), (image, whole)],
        };

        // The channel is made at the first frame's eye size; without it, or at another size (the
        // viewer keeps the size it opened), publishing stops.
        let format = d3d::texture_desc(&eyes[0].0).Format;
        let Some(writer) = self.writer.get(&device, eye_width, eye_height, format).ready(|note| log!("{note} (frame {width}x{height})")) else {
            self.broken = true;
            return;
        };
        // The pose record goes out first under the sequence this pair will get; a pair without one
        // is shown flat.
        let (pose, waiting) = PAIRS.shown();
        if let Some(pose) = pose.filter(|_| flat.is_none()) {
            PAIRS.head().write_pair(writer.next_sequence(), [pose, pose]);
            self.posed += 1;
        }
        self.waiting_max = self.waiting_max.max(waiting);
        match writer.publish_eyes(&context, [(&eyes[0].0, eyes[0].1), (&eyes[1].0, eyes[1].1)], monaka_channel::tick()) {
            Ok(true) => {
                self.pairs += 1;
                self.flat_pairs += u64::from(flat.is_some());
                self.window_pairs += 1;
                PAIRS.tally(true);
            }
            Ok(false) => PAIRS.tally(false),
            Err(e) => self.fail(format!("publish: {e}")),
        }
        let start = *self.window_start.get_or_insert_with(Instant::now);
        let elapsed = start.elapsed().as_secs_f64();
        if elapsed >= 10.0 {
            log!(
                "{:.1} pairs/s over {elapsed:.0} s ({} frames, {} failures, {} with poses, at most {} built frames waiting)",
                self.window_pairs as f64 / elapsed,
                frames(),
                self.failures,
                self.posed,
                self.waiting_max,
            );
            if let Some(rate) = PAIRS.settings().and_then(|s| s.rate) {
                rate();
            }
            self.window_start = Some(Instant::now());
            self.window_pairs = 0;
        }
    }
}

impl State {
    /// A frame of the game drawing one eye per frame (the whole frame `size`): the eye's part and
    /// its depth copied out and handed to the warp thread, which publishes the pair.
    fn warped(&mut self, frame: u64, warp: GlWarp, size: (u32, u32)) {
        let (eye_width, eye_height) = region_size(size.0, size.1, warp.region);
        let display = warp.fsr.map_or((eye_width, eye_height), |fsr| display_size(size.0, size.1, fsr.display));
        if self.worker.is_none() {
            let (Some(bridge), Some(context)) = (self.bridge.as_ref(), self.context.clone()) else { return };
            match Worker::start(bridge.device().clone(), context, warp, self.channel.clone()) {
                Ok(worker) => self.worker = Some(worker),
                Err(e) => {
                    log!("frame warping unavailable: {e}");
                    self.broken = true;
                    return;
                }
            }
        }
        if self.worker.as_ref().is_some_and(Worker::broken) {
            self.broken = true;
            return;
        }

        // A frame the engine draws over the whole window (a video) goes flat, whole.
        let flat = PAIRS.flat_now(PAIRS.settings().and_then(|s| s.flat));
        if flat != self.flat {
            match flat {
                Some(reason) => log!("frame {frame}: {}: whole frames to the flat screen", reason.label()),
                None => log!("frame {frame}: eyes again"),
            }
            self.flat = flat;
        }
        // The loop's ticket and the game's shot go in step: one of each per built frame.
        let (_, waiting) = PAIRS.shown();
        self.waiting_max = self.waiting_max.max(waiting);
        let shot = (warp.shot)();

        let copying = Instant::now();
        let bridge = self.bridge.as_mut().expect("made at the first frame");
        let worker = self.worker.as_mut().expect("started above");
        let (slot, image, depth) = match flat {
            Some(_) => match bridge.capture(0, size.0, size.1) {
                Ok(image) => (None, image.clone(), None),
                Err(e) => return self.fail(format!("capture: {e}")),
            },
            None => {
                let Some(slot) = worker.slot() else { return };
                let left = (size.0 as f32 * warp.region[0]).round() as u32;
                let top = (size.1 as f32 * warp.region[1]).round() as u32;
                let rect = [left, top, eye_width, eye_height];
                let depth = shot.map(|_| {
                    (warp.depth)().map(|d| DepthSource {
                        texture: d.texture,
                        size: (d.width, d.height),
                        multisampled: d.multisampled,
                        region: depth_region(warp.region, size.0, d.width),
                    })
                });
                let copied = match depth.flatten() {
                    Some(source) => bridge.capture_eye(slot, 0, size, rect, Some(source)).or_else(|e| {
                        // Without its depth the eye still goes out, unwarped.
                        if self.depth_failures < 5 {
                            log!("frame {frame}: depth for frame warping: {e}");
                        }
                        self.depth_failures += 1;
                        bridge.capture_eye(slot, 0, size, rect, None)
                    }),
                    None => {
                        if depth.is_some() {
                            if self.depth_failures < 5 {
                                log!("frame {frame}: depth for frame warping: no scene depth");
                            }
                            self.depth_failures += 1;
                        }
                        bridge.capture_eye(slot, 0, size, rect, None)
                    }
                };
                match copied {
                    Ok((image, depth)) => (Some(slot), image, depth),
                    Err(e) => {
                        worker.give_back(slot);
                        return self.fail(format!("eye capture: {e}"));
                    }
                }
            }
        };
        PAIRS.time(Stage::GlColour, copying.elapsed());
        worker.send(Job { frame, slot, image, depth, shot, flat, display, waiting_max: self.waiting_max, depth_failures: self.depth_failures });
    }
}

/// Once a run: the warp's inputs beside the log, to check them without a headset. The depth copy
/// as an image (nearness, white near), its range, and the shot's cameras and projection.
fn describe_warp(device: &ID3D11Device, context: &ID3D11DeviceContext, depth: &ID3D11Texture2D, colour: &ID3D11Texture2D, shot: &Shot) {
    let rendered = &shot.rendered;
    log!(
        "frame warping inputs: eye {}, projection {:?}, depth mapping {:?}, camera {:?}, other camera {:?}",
        shot.eye,
        rendered.projection,
        rendered.mapping,
        rendered.camera,
        shot.other.camera
    );
    let Ok((width, height, texels)) = d3d::read_image(device, context, depth) else { return log!("frame warping inputs: depth not readable") };
    let values: Vec<f32> = texels.chunks_exact(4).map(|t| f32::from_le_bytes([t[0], t[1], t[2], t[3]])).collect();
    let (low, high) = values.iter().fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
    let middle = values[(height as usize / 2) * width as usize + width as usize / 2];
    log!("frame warping inputs: depth {width}x{height}, from {low} to {high}, {middle} at the centre (nearness {})", rendered.mapping.nearness(middle));
    let Some(session) = monaka_producer::Session::get() else { return };
    let bgra: Vec<u8> = values.iter().flat_map(|v| {
        let near = (rendered.mapping.nearness(*v).sqrt() * 255.0) as u8;
        [near, near, near, 255]
    }).collect();
    let path = session.path(&format!("warp-depth-{}.bmp", ["left", "right"][shot.eye.min(1)]));
    match std::fs::write(&path, monaka_core::bmp::bgra(width, height, &bgra)) {
        Ok(()) => log!("frame warping inputs: depth written to {}", path.display()),
        Err(e) => log!("frame warping inputs: depth not written: {e}"),
    }
    // The frame's own image, as captured with that depth.
    if let Ok((width, height, rgba)) = d3d::read_image(device, context, colour) {
        let bgra: Vec<u8> = rgba.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0], 255]).collect();
        let _ = std::fs::write(session.path(&format!("warp-colour-{}.bmp", ["left", "right"][shot.eye.min(1)])), monaka_core::bmp::bgra(width, height, &bgra));
    }
}

pub(crate) fn report() -> String {
    let guard = STATE.locked();
    match guard.0.as_ref() {
        Some(state) => {
            let pairs = state.pairs + worker::PUBLISHED.load(Relaxed);
            let flat = state.flat_pairs + worker::PUBLISHED_FLAT.load(Relaxed);
            let posed = state.posed + worker::WARPED.load(Relaxed);
            format!("{} frames read back, {pairs} pairs ({flat} flat), {posed} with poses, {} failures", frames(), state.failures)
        }
        None => String::new(),
    }
}

#[cfg(test)]
mod tests;
