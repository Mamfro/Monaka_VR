//! FSR upscaling for D3D11 games. AMD's upscaler runs on D3D12 only, so this keeps a private D3D12
//! device on the game's adapter and shares textures and one fence with the game's D3D11 device:
//!
//! 1. on the game's context, the frame's colour is copied into a shared texture and its depth and
//!    motion vectors are written into two more ([`monaka_warp::MotionPass`]), then the fence is
//!    signalled;
//! 2. the D3D12 queue waits for that value, upscales into the eye's shared output and signals the
//!    next value;
//! 3. the game's context waits for it before anything reads the output.
//!
//! Neither side waits on the CPU. One upscaling context per eye: alternate-eye rendering gives
//! each eye its own history, two game frames apart.

use monaka_channel::d3d::{self, adapter_luid, find_adapter};
use monaka_channel::d3d12::{Recorder, wait_fence};
use monaka_ffx::{self as ffx, Api, Float2, Resource, Size2, state};
use monaka_ffx::upscale::{self, Upscaler};
use monaka_warp::{EyeView, MotionPass, ObjectMotion};
use std::fmt;
use std::path::Path;
use windows::Win32::Foundation::{CloseHandle, GENERIC_ALL};
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_BIND_UNORDERED_ACCESS, D3D11_RESOURCE_MISC_SHARED, D3D11_RESOURCE_MISC_SHARED_NTHANDLE,
    D3D11_TEXTURE2D_DESC, ID3D11Device, ID3D11Device5, ID3D11DeviceContext, ID3D11DeviceContext4, ID3D11Fence, ID3D11Texture2D,
    ID3D11UnorderedAccessView,
};
use windows::Win32::Graphics::Direct3D12::{
    D3D12_COMMAND_LIST_TYPE_DIRECT, D3D12_COMMAND_QUEUE_DESC, D3D12_FENCE_FLAG_SHARED, D3D12CreateDevice, ID3D12CommandQueue, ID3D12Device, ID3D12Fence,
    ID3D12Resource,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R16G16_FLOAT, DXGI_FORMAT_R32_FLOAT};
use windows::Win32::Graphics::Dxgi::IDXGIResource1;
use windows::core::Interface;

mod framegen;

pub use framegen::{FrameGen11, GenFrame, GenSettings};

/// AMD's loader and the upscaler provider it loads.
pub const UPSCALER_DLL: &str = "amd_fidelityfx_upscaler_dx12.dll";
/// The frame generation provider ([`load_with_framegen`]).
pub const FRAMEGEN_DLL: &str = "amd_fidelityfx_framegeneration_dx12.dll";

#[derive(Debug)]
pub enum Error {
    Os(windows::core::Error),
    Ffx(ffx::Error),
    Warp(monaka_warp::Error),
    /// Missing or unusable: what.
    Unavailable(String),
    /// The previous frames' D3D12 work still holds the command lists (nothing was done).
    Busy,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Os(e) => write!(f, "{e}"),
            Error::Ffx(e) => write!(f, "{e}"),
            Error::Warp(e) => write!(f, "{e}"),
            Error::Unavailable(what) => write!(f, "{what}"),
            Error::Busy => write!(f, "the upscaler's command lists are still in use"),
        }
    }
}

impl From<windows::core::Error> for Error {
    fn from(e: windows::core::Error) -> Self {
        Error::Os(e)
    }
}

impl From<ffx::Error> for Error {
    fn from(e: ffx::Error) -> Self {
        Error::Ffx(e)
    }
}

impl From<monaka_warp::Error> for Error {
    fn from(e: monaka_warp::Error) -> Self {
        Error::Warp(e)
    }
}

impl From<monaka_channel::Error> for Error {
    fn from(e: monaka_channel::Error) -> Self {
        Error::Unavailable(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// A Direct3D failure named by the call that failed.
fn at<T>(what: &str, result: windows::core::Result<T>) -> Result<T> {
    result.map_err(|e| Error::Unavailable(format!("{what}: {e}")))
}

/// AMD's FFX API from `dir` (the loader and the upscaler provider, loaded by full path so the
/// loader finds its provider already loaded instead of searching the game's folders).
pub fn load(dir: &Path) -> Result<Api> {
    load_providers(dir, &[UPSCALER_DLL])
}

/// [`load`] with the frame generation provider too (the upscaler's when `dir` has it), both before
/// the loader, which then finds both in the process: for a run that uses frame generation
/// ([`FrameGen11`]) with or without the upscaler, whichever is made first loading the loader.
pub fn load_with_framegen(dir: &Path) -> Result<Api> {
    let providers: &[&str] = if dir.join(UPSCALER_DLL).exists() { &[UPSCALER_DLL, FRAMEGEN_DLL] } else { &[FRAMEGEN_DLL] };
    load_providers(dir, providers)
}

fn load_providers(dir: &Path, providers: &[&str]) -> Result<Api> {
    Api::load(Some(dir), providers).map_err(|e| match e {
        ffx::LoadError::Os(_, e) => Error::Os(e),
        other => Error::Unavailable(other.to_string()),
    })
}

/// One eye's frame as the game rendered it, before its HUD.
pub struct EyeFrame<'a> {
    /// The finished world image at render size (RGBA8, any of the family's formats).
    pub color: &'a ID3D11Texture2D,
    pub depth: &'a ID3D11Texture2D,
    /// The game's own object motion, if it writes one.
    pub object: Option<ObjectMotion<'a>>,
    /// The camera and the unjittered projection the depth was rendered with.
    pub view: EyeView,
    /// The pixel offset the projection was jittered by (x right, y down), from [`Fsr11::jitter`].
    pub jitter: [f32; 2],
    pub near: f32,
    pub fov_vertical: f32,
    pub frame_time_ms: f32,
    /// Drop the eye's history (a cut).
    pub reset: bool,
}

/// A texture made on the game's D3D11 device and opened on the D3D12 device. (The other way
/// round, D3D11 opens only render-target-only RGBA8, RGB10A2 and RGBA16F textures. Made on
/// D3D11, every format here opens, as render target, unordered access and simultaneous access on
/// D3D12.)
struct Shared {
    d12: ID3D12Resource,
    d11: ID3D11Texture2D,
}

impl Shared {
    fn new(device11: &ID3D11Device, device12: &ID3D12Device, (width, height): (u32, u32), format: DXGI_FORMAT) -> Result<Self> {
        let desc = D3D11_TEXTURE2D_DESC {
            BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_UNORDERED_ACCESS.0) as u32,
            MiscFlags: (D3D11_RESOURCE_MISC_SHARED.0 | D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0) as u32,
            ..d3d::simple_texture_desc(width, height, format)
        };
        let mut d11 = None;
        let mut d12: Option<ID3D12Resource> = None;
        // SAFETY: creates a shared texture on the game's device, shares it and opens the share on
        // ours; the handle is closed once opened (each side holds its own reference).
        unsafe {
            at("shared texture", device11.CreateTexture2D(&desc, None, Some(&mut d11)))?;
            let d11 = d11.ok_or_else(|| Error::Unavailable("shared texture".into()))?;
            let handle = at("texture share", d11.cast::<IDXGIResource1>().and_then(|r| r.CreateSharedHandle(None, GENERIC_ALL.0, None)))?;
            let opened = device12.OpenSharedHandle(handle, &mut d12);
            let _ = CloseHandle(handle);
            at(&format!("opening the shared {width}x{height} texture of format {}", format.0), opened)?;
            Ok(Self { d12: d12.ok_or_else(|| Error::Unavailable("shared texture".into()))?, d11 })
        }
    }

    fn resource(&self, format: DXGI_FORMAT) -> Resource {
        Resource::texture(&self.d12, state::COMMON, Some(format))
    }
}

/// The render-size inputs, shared by both eyes (one frame is in flight at a time).
struct Inputs {
    color: Shared,
    depth: Shared,
    depth_uav: ID3D11UnorderedAccessView,
    motion: Shared,
    motion_uav: ID3D11UnorderedAccessView,
}

struct Eye {
    upscaler: Upscaler,
    output: Shared,
    previous: Option<EyeView>,
    phases: i32,
    index: i32,
}

pub struct Fsr11 {
    device: ID3D11Device,
    motion: MotionPass,
    device12: ID3D12Device,
    queue: ID3D12CommandQueue,
    recorder: Recorder,
    fence12: ID3D12Fence,
    fence11: ID3D11Fence,
    /// The fence's last value: odd once the inputs are written, even once the upscale is done.
    value: u64,
    render: (u32, u32),
    display: (u32, u32),
    sharpness: Option<f32>,
    /// Sign the dispatch's jitter carries relative to the content shift (see [`Fsr11::jitter`]).
    jitter_sign: f32,
    inputs: Inputs,
    eyes: [Eye; 2],
}

// SAFETY: used under its owner's lock on the game's render thread.
unsafe impl Send for Fsr11 {}

fn uav(device: &ID3D11Device, texture: &ID3D11Texture2D) -> Result<ID3D11UnorderedAccessView> {
    let mut view = None;
    // SAFETY: a view of a shared texture made with unordered access.
    unsafe { at("unordered access view", device.CreateUnorderedAccessView(texture, None, Some(&mut view)))? };
    view.ok_or_else(|| Error::Unavailable("unordered access view".into()))
}

/// Settings for [`Fsr11::new`].
#[derive(Clone, Copy, Debug)]
pub struct Settings {
    /// The size the game renders each eye at.
    pub render: (u32, u32),
    /// The size each eye is upscaled to.
    pub display: (u32, u32),
    /// RCAS sharpening 0..1, or none.
    pub sharpness: Option<f32>,
    /// 1 or -1: whether the dispatch is told the content shift or its opposite.
    pub jitter_sign: f32,
    /// [`upscale::create`] flags for the depth convention (inverted, infinite).
    pub depth_flags: u32,
}

/// A D3D12 device and direct queue on the game device's adapter, and one fence both devices
/// share: what the upscaler and frame generation ([`FrameGen11`]) each work over.
struct Beside {
    device12: ID3D12Device,
    queue: ID3D12CommandQueue,
    fence12: ID3D12Fence,
    fence11: ID3D11Fence,
}

impl Beside {
    fn new(device: &ID3D11Device) -> Result<Self> {
        let adapter = find_adapter(adapter_luid(device)?)?;
        let device5: ID3D11Device5 = device.cast()?;
        let mut device12: Option<ID3D12Device> = None;
        // SAFETY: creation calls on the game's adapter, into locals.
        unsafe {
            at("D3D12 device", D3D12CreateDevice(&adapter, D3D_FEATURE_LEVEL_11_0, &mut device12))?;
            let device12 = device12.ok_or_else(|| Error::Unavailable("D3D12 device".into()))?;
            let queue: ID3D12CommandQueue = device12.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC { Type: D3D12_COMMAND_LIST_TYPE_DIRECT, ..Default::default() })?;
            let fence12: ID3D12Fence = at("shared fence", device12.CreateFence(0, D3D12_FENCE_FLAG_SHARED))?;
            let handle = at("fence share", device12.CreateSharedHandle(&fence12, None, GENERIC_ALL.0, None))?;
            let mut fence11: Option<ID3D11Fence> = None;
            let opened = device5.OpenSharedFence(handle, &mut fence11);
            let _ = CloseHandle(handle);
            at("opening the shared fence", opened)?;
            Ok(Self { device12, queue, fence12, fence11: fence11.ok_or_else(|| Error::Unavailable("shared fence".into()))? })
        }
    }
}

impl Fsr11 {
    /// A D3D12 device on the game device's adapter, the shared textures and one upscaling context
    /// per eye.
    pub fn new(device: &ID3D11Device, api: Api, settings: Settings) -> Result<Self> {
        let Beside { device12, queue, fence12, fence11 } = Beside::new(device)?;
        let recorder = Recorder::on(&device12, 3, fence12.clone())?;
        let (render, display) = (settings.render, settings.display);
        // The sRGB back buffer copies into UNORM as stored.
        let color = Shared::new(device, &device12, render, DXGI_FORMAT_R8G8B8A8_UNORM)?;
        let depth = Shared::new(device, &device12, render, DXGI_FORMAT_R32_FLOAT)?;
        let motion = Shared::new(device, &device12, render, DXGI_FORMAT_R16G16_FLOAT)?;
        let inputs = Inputs { depth_uav: uav(device, &depth.d11)?, motion_uav: uav(device, &motion.d11)?, color, depth, motion };
        let size = |(width, height): (u32, u32)| Size2 { width, height };
        let eye = || -> Result<Eye> {
            let mut upscaler = Upscaler::new(api, &device12, size(render), size(display), settings.depth_flags)?;
            let phases = upscaler.jitter_phase_count(render.0)?.max(1);
            let output = Shared::new(device, &device12, display, DXGI_FORMAT_R8G8B8A8_UNORM)?;
            Ok(Eye { upscaler, output, previous: None, phases, index: 0 })
        };
        let eyes = [eye()?, eye()?];
        Ok(Self {
            device: device.clone(),
            motion: MotionPass::new(device)?,
            device12,
            queue,
            recorder,
            fence12,
            fence11,
            value: 0,
            render,
            display,
            sharpness: settings.sharpness,
            jitter_sign: settings.jitter_sign,
            inputs,
            eyes,
        })
    }

    /// The provider's version name (for the log).
    pub fn version(&self) -> &str {
        self.eyes[0].upscaler.version()
    }

    pub fn render(&self) -> (u32, u32) {
        self.render
    }

    pub fn display(&self) -> (u32, u32) {
        self.display
    }

    pub fn phases(&self) -> i32 {
        self.eyes[0].phases
    }

    /// The pixel offset (x right, y down) to shift eye `eye`'s next frame by, the next of its
    /// jitter sequence. The projection moves the image content by this much; give the same value
    /// back in that frame's [`EyeFrame::jitter`].
    pub fn jitter(&mut self, eye: usize) -> [f32; 2] {
        let eye = &mut self.eyes[eye];
        eye.index = (eye.index + 1) % eye.phases;
        eye.upscaler.jitter_offset(eye.index, eye.phases).map_or([0.0; 2], |j| [j.x, j.y])
    }

    /// Upscales eye `eye`'s frame on the game's `context`; returns the eye's output (display size,
    /// RGBA8 UNORM), ready for anything later on that context.
    pub fn upscale(&mut self, context: &ID3D11DeviceContext, eye: usize, frame: &EyeFrame<'_>) -> Result<ID3D11Texture2D> {
        let color_desc = d3d::texture_desc(frame.color);
        if (color_desc.Width, color_desc.Height) != self.render {
            return Err(Error::Unavailable(format!("a {}x{} frame for a {}x{} upscaler", color_desc.Width, color_desc.Height, self.render.0, self.render.1)));
        }
        let Some(list) = self.recorder.begin()? else { return Err(Error::Busy) };
        let context4: ID3D11DeviceContext4 = context.cast()?;
        let previous = if frame.reset { None } else { self.eyes[eye].previous };
        // SAFETY: GPU waits and signals on the shared fence, and a copy between same-shaped
        // textures of one family, all on the game's context.
        unsafe {
            context4.Wait(&self.fence11, self.value)?;
            context.CopyResource(&self.inputs.color.d11, frame.color);
        }
        let inputs = &self.inputs;
        self.motion.run(context, frame.depth, &frame.view, previous.as_ref(), frame.object.as_ref().map(|o| ObjectMotion { texture: o.texture, scale: o.scale }), &inputs.motion_uav, &inputs.depth_uav, self.render)?;
        self.value += 1;
        let written = self.value;
        // SAFETY: as above; the flush hands the work to the GPU so the D3D12 queue's wait ends.
        unsafe {
            context4.Signal(&self.fence11, written)?;
            context.Flush();
        }

        let size = |(width, height): (u32, u32)| Size2 { width, height };
        let state = &mut self.eyes[eye];
        let dispatch = upscale::Frame {
            color: inputs.color.resource(DXGI_FORMAT_R8G8B8A8_UNORM),
            depth: inputs.depth.resource(DXGI_FORMAT_R32_FLOAT),
            motion_vectors: inputs.motion.resource(DXGI_FORMAT_R16G16_FLOAT),
            reactive: None,
            output: state.output.resource(DXGI_FORMAT_R8G8B8A8_UNORM),
            render_size: size(self.render),
            jitter: Float2 { x: self.jitter_sign * frame.jitter[0], y: self.jitter_sign * frame.jitter[1] },
            motion_vector_scale: Float2 { x: self.render.0 as f32, y: self.render.1 as f32 },
            sharpness: self.sharpness,
            frame_time_ms: frame.frame_time_ms,
            reset: previous.is_none(),
            near: frame.near,
            far: 100_000.0,
            fov_vertical: frame.fov_vertical,
            flags: 0,
        };
        let recorded = state.upscaler.dispatch(&list, &dispatch);
        // SAFETY: the queue waits for the inputs on the GPU before running the list.
        unsafe { self.queue.Wait(&self.fence12, written)? };
        // The list is submitted even when the dispatch failed, so the fence keeps counting.
        self.value = self.recorder.submit_as(&self.queue, written + 1)?;
        // SAFETY: the game's context waits on the GPU for the upscale before reading its output.
        unsafe { context4.Wait(&self.fence11, self.value)? };
        recorded?;
        state.previous = Some(frame.view);
        Ok(state.output.d11.clone())
    }
}

impl Drop for Fsr11 {
    fn drop(&mut self) {
        // The upscalers and shared textures go once the GPU is done with them.
        if !wait_fence(&self.fence12, self.value, 2000) {
            monaka_producer::log!("FSR: the GPU did not finish within 2 s; releasing anyway");
        }
        let _ = (&self.device, &self.device12, &self.inputs.motion.d12, &self.inputs.depth.d12);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use monaka_channel::d3d::{create_device, create_texture_with, read_pixel};
    use windows::Win32::Graphics::Direct3D11::{D3D11_BIND_SHADER_RESOURCE, D3D11_TEXTURE2D_DESC};
    use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM_SRGB;

    fn texture(device: &ID3D11Device, w: u32, h: u32, format: DXGI_FORMAT, bytes: &[u8]) -> ID3D11Texture2D {
        create_texture_with(device, &D3D11_TEXTURE2D_DESC { BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32, ..d3d::simple_texture_desc(w, h, format) }, bytes).unwrap()
    }

    /// A still sRGB picture at 128x72, a wall 2 m away, both eyes upscaled to 256x144 for a few
    /// alternating frames through the shared textures and fence: the output is the picture.
    #[test]
    fn upscales_both_eyes_of_a_d3d11_device() {
        // A folder with AMD's FidelityFX DLLs (a game installs them: Dying Light 2's are in its
        // `ph\work\bin\x64`).
        let Some(dir) = std::env::var_os("MONAKA_FFX_DIR").map(std::path::PathBuf::from).filter(|d| d.join(ffx::LOADER).exists()) else {
            return eprintln!("MONAKA_FFX_DIR (a folder with AMD's FidelityFX DLLs) is not set; skipped");
        };
        let api = load(&dir).unwrap();
        let (device, context, _) = create_device(None).unwrap();
        let (rw, rh) = (128, 72);
        let settings = Settings { render: (rw, rh), display: (256, 144), sharpness: None, jitter_sign: -1.0, depth_flags: upscale::create::DEPTH_INVERTED | upscale::create::DEPTH_INFINITE };
        let mut fsr = Fsr11::new(&device, api, settings).unwrap();
        eprintln!("provider {} with {} jitter phases", fsr.version(), fsr.phases());
        let near = 0.1f32;
        let mut projection = [0.0f32; 16];
        projection[0] = 1.0;
        projection[5] = 1.0;
        projection[11] = near;
        projection[14] = -1.0;
        let pixels: Vec<u8> = (0..rw * rh).flat_map(|_| [40u8, 120, 200, 255]).collect();
        let color = texture(&device, rw, rh, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB, &pixels);
        let depth: Vec<u8> = (0..rw * rh).flat_map(|_| (near / 2.0).to_le_bytes()).collect();
        let depth = texture(&device, rw, rh, DXGI_FORMAT_R32_FLOAT, &depth);
        let mut outputs = Vec::new();
        for frame in 0..8 {
            let eye = frame % 2;
            let jitter = fsr.jitter(eye);
            let camera = [1.0, 0.0, 0.0, if eye == 0 { -0.032 } else { 0.032 }, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0];
            let input = EyeFrame {
                color: &color,
                depth: &depth,
                object: None,
                view: EyeView { camera, projection },
                jitter,
                near,
                fov_vertical: 1.57,
                frame_time_ms: 11.1,
                reset: false,
            };
            // Faster than any game: wait for a free command list as a frame's time would.
            let output = loop {
                match fsr.upscale(&context, eye, &input) {
                    Err(Error::Busy) => std::thread::sleep(std::time::Duration::from_millis(1)),
                    other => break other.unwrap(),
                }
            };
            outputs.push(output);
        }
        for output in &outputs[6..] {
            let pixel = read_pixel(&device, &context, output, 200, 100).unwrap();
            eprintln!("upscaled pixel {pixel:?}");
            for (got, want) in pixel.iter().zip([40u8, 120, 200]) {
                assert!(got.abs_diff(want) <= 3, "upscaled {pixel:?}, expected the still picture");
            }
        }
    }
}

