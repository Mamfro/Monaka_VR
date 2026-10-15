//! One eye's frame generation: its context, its output, and the step that makes the frame halfway
//! between the eye's previous frame and its current one ([`EyeStream::generate`]). Dying Light 2
//! (same-frame stereo) and The Beast (alternate-eye) keep one per eye, and share the glue around
//! it: the inputs check ([`FrameInputs::fresh`]), the stream remade when the frames change
//! ([`fitting_stream`]), the count of eyes not generated ([`skip_eye`]) and the release at stop
//! ([`free_when_idle`]). How the eyes pair up stays in each game.

use monaka_ffx::{self as ffx, Camera, FrameGen, Resource, Size2};
use monaka_channel::d3d12::{self, Recorder};
use monaka_producer::log;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::Instant;
use windows::Win32::Graphics::Direct3D12::{D3D12_RESOURCE_DESC, D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS, ID3D12Device, ID3D12GraphicsCommandList, ID3D12Resource};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;

static API: OnceLock<ffx::Api> = OnceLock::new();

/// Frame generation is on for this run (set at start, once the game's loader is in).
pub fn enable(api: ffx::Api) {
    let _ = API.set(api);
}

pub fn enabled() -> bool {
    API.get().is_some()
}

/// One eye's frame-generation context and the texture its generated frames go to.
pub struct EyeStream {
    generator: FrameGen,
    pub output: ID3D12Resource,
    render_size: Size2,
    frame_id: u64,
    last: Option<Instant>,
    /// The previous frame was given to it (so the next generated frame has a real neighbour).
    pub primed: bool,
}

impl EyeStream {
    /// A context for frames like `frame` (the back buffer) rendered at `render_size`, with depth
    /// inverted and infinite (both Dying Lights).
    pub fn new(device: &ID3D12Device, frame: &D3D12_RESOURCE_DESC, render_size: Size2) -> Result<Self, String> {
        let display = Size2 { width: frame.Width as u32, height: frame.Height };
        let flags = ffx::create::DEPTH_INVERTED | ffx::create::DEPTH_INFINITE;
        let generator = FrameGen::new(*API.get().ok_or("frame generation off")?, device, display, render_size, frame.Format, flags).map_err(|e| e.to_string())?;
        let output = d3d12::texture(device, display.width, display.height, frame.Format, D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS).map_err(|e| e.to_string())?;
        log!("frame generation context for an eye: {}x{} from {}x{} (provider {})", display.width, display.height, render_size.width, render_size.height, generator.version());
        Ok(Self { generator, output, render_size, frame_id: 0, last: None, primed: false })
    }

    /// Whether this context serves frames like `frame` rendered at `render_size`.
    pub fn fits(&self, frame: &D3D12_RESOURCE_DESC, render_size: Size2) -> bool {
        // SAFETY: reads a descriptor.
        let out = unsafe { self.output.GetDesc() };
        self.render_size == render_size && out.Width == frame.Width && out.Height == frame.Height && out.Format == frame.Format
    }

    /// Records on `list` the frame halfway between the eye's previous frame and `current` into
    /// [`EyeStream::output`], from this frame's `camera` (already [`Camera::for_render`]), depth
    /// (R32) and motion vectors (read as `motion_view`, or their own format); all in the common
    /// state. `reset` drops the history (a cut). `Ok(true)` when the generated frame is usable,
    /// `Ok(false)` when it only started the history again.
    #[allow(clippy::too_many_arguments)]
    pub fn generate(
        &mut self,
        list: &ID3D12GraphicsCommandList,
        camera: &Camera,
        depth: &ID3D12Resource,
        motion: &ID3D12Resource,
        motion_view: Option<DXGI_FORMAT>,
        current: &ID3D12Resource,
        reset: bool,
        default_frame_ms: f32,
    ) -> Result<bool, String> {
        let reset = reset || !self.primed;
        let now = Instant::now();
        let frame_ms = self.last.map_or(default_frame_ms, |t| (now - t).as_secs_f32() * 1000.0);
        self.last = Some(now);
        self.frame_id += 1;
        let common = ffx::state::COMMON;
        let result = self
            .generator
            .configure(self.frame_id, None)
            .and_then(|()| {
                self.generator.prepare(
                    list,
                    self.frame_id,
                    camera,
                    Resource::texture(depth, common, Some(windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R32_FLOAT)),
                    Resource::texture(motion, common, motion_view),
                    frame_ms,
                )
            })
            .and_then(|()| self.generator.generate(list, self.frame_id, Resource::texture(current, common, None), Resource::texture(&self.output, common, None), reset));
        self.primed = result.is_ok();
        result.map(|()| !reset).map_err(|e| e.to_string())
    }
}

/// Real frames kept per eye, in a ring: frame generation reads the previous frame from the texture
/// it was given (with one texture rewritten each frame, the "generated" frame is the newest one),
/// so the last two must stay as they were while the next is written.
pub const SLOTS: usize = 3;

static EYES_SKIPPED: AtomicU64 = AtomicU64::new(0);

/// An eye whose frame was not generated: counted ([`eyes_skipped`]) and logged with `why` the first
/// times. Always false (not generated), to return as the eye's outcome.
pub fn skip_eye(why: &str) -> bool {
    EYES_SKIPPED.fetch_add(1, Relaxed);
    monaka_producer::log_first(12, format_args!("frame generation: an eye not generated: {why}"));
    false
}

/// Eyes not generated so far ([`skip_eye`]).
pub fn eyes_skipped() -> u64 {
    EYES_SKIPPED.load(Relaxed)
}

/// What one eye's generated frame is made from, all of the frame being presented.
pub struct FrameInputs {
    /// The depth (R32) and motion vectors, at render size.
    pub depth: ID3D12Resource,
    pub motion: ID3D12Resource,
    /// The camera, for the render size ([`Camera::for_render`]).
    pub camera: Camera,
    /// The render size: the depth's.
    pub render_size: Size2,
}

impl FrameInputs {
    /// The eye's inputs when this frame's depth, motion vectors and camera all came (`fresh`: the
    /// producer's flag that they were written since the eye's last present). Otherwise the eye is
    /// skipped ([`skip_eye`]) and its `stream` unprimed, so its next frame starts the history again
    /// instead of interpolating across the gap.
    pub fn fresh(fresh: bool, depth: Option<&ID3D12Resource>, motion: Option<&ID3D12Resource>, camera: Option<Camera>, stream: Option<&mut EyeStream>) -> Option<Self> {
        let (true, Some(depth), Some(motion), Some(camera)) = (fresh, depth, motion, camera) else {
            if let Some(stream) = stream {
                stream.primed = false;
            }
            skip_eye("depth, motion vectors or camera missing this frame");
            return None;
        };
        // SAFETY: reads a descriptor.
        let render = unsafe { depth.GetDesc() };
        let render_size = Size2 { width: render.Width as u32, height: render.Height };
        Some(Self { depth: depth.clone(), motion: motion.clone(), camera: camera.for_render(render_size), render_size })
    }
}

/// The eye's stream in `slot` if it `fits` the frames now (their size and format, the render size),
/// else a new one from `make`. The old one goes first, once `recorder`'s lists (which may still
/// use it on the GPU) are done, waiting up to a second. `S` is the stream with whatever the
/// producer keeps beside it. On an error the slot is left empty.
pub fn fitting_stream<'a, S>(slot: &'a mut Option<S>, recorder: &Recorder, fits: impl FnOnce(&S) -> bool, make: impl FnOnce() -> Result<S, String>) -> Result<&'a mut S, String> {
    if !slot.as_ref().is_some_and(fits) {
        if let Some(old) = slot.take() {
            recorder.idle(1000);
            drop(old);
        }
        *slot = Some(make()?);
    }
    Ok(slot.as_mut().expect("made above"))
}

/// Frees a producer's frame-generation state at stop, once the GPU is done with its work (the
/// lists of its `recorder`, waiting up to five seconds); if the GPU is still busy then, the state
/// is leaked (logged) rather than freed under it.
pub fn free_when_idle<S>(state: S, recorder: impl FnOnce(&S) -> &Recorder) {
    if !recorder(&state).idle(5000) {
        log!("frame generation: GPU still busy at stop; its resources are left allocated");
        std::mem::forget(state);
    }
}
