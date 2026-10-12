//! Same-frame pairs from an OpenGL game: both eyes drawn side by side in one frame (the engine's own
//! stereo layout). At the game's `SwapBuffers` (its present entry calls [`present`] on the render
//! thread, its GL context current) the back buffer is copied onto a D3D11 device of the loop's own
//! (`monaka_gl`), split into the eyes and published on the keyed-mutex channel, the pose record of
//! the frame's ticket first. A frame that goes flat (a full-screen video the engine draws over the
//! whole window, not per eye) goes whole, at the eye size, to both eyes without a pose. Without the
//! side-by-side layout the whole frame goes to both eyes.

use super::PAIRS;
use crate::driver::Flat;
use monaka_channel::keyed::LazyWriter;
use monaka_channel::{ChannelName, d3d};
use monaka_core::sync::Locked;
use monaka_gl::Bridge;
use monaka_producer::{log, wait_until};
use monaka_warp::Scaler;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::*};
use std::time::Instant;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D11::{D3D11_BIND_SHADER_RESOURCE, D3D11_BOX, ID3D11DeviceContext, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM;
use windows::Win32::Graphics::Gdi::{HDC, WindowFromDC};
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;

/// Set while the render thread holds GL objects; the render thread clears it after freeing them.
static ACTIVE: AtomicBool = AtomicBool::new(false);
/// Frames seen at `SwapBuffers` while publishing.
static FRAMES: AtomicU64 = AtomicU64::new(0);

struct Eyes {
    textures: [ID3D11Texture2D; 2],
    width: u32,
    height: u32,
}

struct State {
    /// Each half of the frame is one eye.
    side_by_side: bool,
    bridge: Option<Bridge>,
    context: Option<ID3D11DeviceContext>,
    eyes: Option<Eyes>,
    /// Why the last frame went flat, if it did.
    flat: Option<Flat>,
    /// Resamples a flat frame to the eye size, made at the first one.
    scaler: Option<Scaler>,
    writer: LazyWriter,
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

pub(crate) fn install(channel: ChannelName, side_by_side: bool) {
    let state = State {
        side_by_side,
        bridge: None,
        context: None,
        eyes: None,
        flat: None,
        scaler: None,
        writer: LazyWriter::new(channel),
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
        if let Some(state) = STATE.locked().0.as_mut() {
            state.frame(dc);
        }
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
            std::mem::forget(state.bridge.take());
        }
    }
}

/// Frees the GL objects; called on the render thread with the game's context current.
fn release_here() {
    if let Some(state) = STATE.locked().0.as_mut() {
        if let Some(bridge) = state.bridge.take() {
            bridge.release();
        }
        state.eyes = None;
        state.scaler = None;
        state.context = None;
        log!("capture released after {} frames, {} pairs ({} flat), {} failures", frames(), state.pairs, state.flat_pairs, state.failures);
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
        monaka_core::protocol::dimensions_ok(w, h).then_some((w, h))
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
        let (eye_width, eye_height) = eye_size((width, height), self.side_by_side);
        let (Some(bridge), Some(context)) = (self.bridge.as_mut(), self.context.clone()) else { return };
        let device = bridge.device().clone();
        let image = match bridge.capture(0, width, height) {
            Ok(texture) => texture.clone(),
            Err(e) => return self.fail(format!("capture: {e}")),
        };

        if self.side_by_side && self.eyes.as_ref().is_none_or(|e| (e.width, e.height) != (eye_width, eye_height)) {
            let mut desc = d3d::simple_texture_desc(eye_width, eye_height, DXGI_FORMAT_R8G8B8A8_UNORM);
            desc.BindFlags = D3D11_BIND_SHADER_RESOURCE.0 as u32;
            match (d3d::create_texture(&device, &desc), d3d::create_texture(&device, &desc)) {
                (Ok(left), Ok(right)) => self.eyes = Some(Eyes { textures: [left, right], width: eye_width, height: eye_height }),
                _ => return self.fail("eye textures"),
            }
        }
        // A frame the engine draws over the whole window (a video) is not per eye: it goes flat.
        let flat = if self.side_by_side { PAIRS.flat_now(PAIRS.settings().and_then(|s| s.flat)) } else { None };
        if flat != self.flat {
            match flat {
                Some(reason) => log!("frame {frame}: {}: whole frames to the flat screen", reason.label()),
                None => log!("frame {frame}: eyes again"),
            }
            self.flat = flat;
        }
        let eyes: [ID3D11Texture2D; 2] = match &self.eyes {
            _ if flat.is_some() => {
                let scaler = match self.scaler.as_mut() {
                    Some(scaler) => scaler,
                    None => match Scaler::new(&device) {
                        Ok(made) => self.scaler.insert(made),
                        Err(e) => return self.fail(format!("flat frame scaler: {e}")),
                    },
                };
                match scaler.run(&context, &image, (eye_width, eye_height)) {
                    Ok(scaled) => [scaled.clone(), scaled.clone()],
                    Err(e) => return self.fail(format!("flat frame: {e}")),
                }
            }
            Some(eyes) if self.side_by_side => {
                for (eye, target) in eyes.textures.iter().enumerate() {
                    let left = eye as u32 * eye_width;
                    let region = D3D11_BOX { left, top: 0, front: 0, right: left + eye_width, bottom: eye_height, back: 1 };
                    // SAFETY: copies a region inside the frame into a same-format texture on our device.
                    unsafe { context.CopySubresourceRegion(target, 0, 0, 0, 0, &image, 0, Some(&region)) };
                }
                eyes.textures.clone()
            }
            _ => [image.clone(), image],
        };

        // The channel is made at the first frame's eye size; without it, or at another size (the
        // viewer keeps the size it opened), publishing stops.
        let Some(writer) = self.writer.get(&device, eye_width, eye_height).ready(|note| log!("{note} (frame {width}x{height})")) else {
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
        match writer.publish(&context, [&eyes[0], &eyes[1]], monaka_channel::tick()) {
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

pub(crate) fn report() -> String {
    let guard = STATE.locked();
    match guard.0.as_ref() {
        Some(state) => format!("{} frames read back, {} pairs ({} flat), {} with poses, {} failures", frames(), state.pairs, state.flat_pairs, state.posed, state.failures),
        None => String::new(),
    }
}
