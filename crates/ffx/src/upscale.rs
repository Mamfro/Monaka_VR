//! AMD FSR upscaling through the FFX API (`ffx_upscale.h`, MIT): one context per image stream (one
//! eye), given each frame's colour, depth and motion vectors at render size with the jitter it was
//! rendered with, writing the frame at display size. Every size and offset below is checked against
//! an MSVC compile of the SDK headers.

use crate::{Api, CreateBackendDx12, Error, Float2, Header, OwnedContext, Resource, Size2};
use windows::Win32::Graphics::Direct3D12::{ID3D12Device, ID3D12GraphicsCommandList};
use windows::core::Interface;

const CREATE_UPSCALE: u64 = 0x10000;
const DISPATCH_UPSCALE: u64 = 0x10001;
const QUERY_JITTER_PHASE_COUNT: u64 = 0x10004;
const QUERY_JITTER_OFFSET: u64 = 0x10005;

/// `FfxApiCreateContextUpscaleFlags`.
pub mod create {
    pub const HIGH_DYNAMIC_RANGE: u32 = 1 << 0;
    pub const DEPTH_INVERTED: u32 = 1 << 3;
    pub const DEPTH_INFINITE: u32 = 1 << 4;
    pub const AUTO_EXPOSURE: u32 = 1 << 5;
    pub const DEBUG_CHECKING: u32 = 1 << 7;
    pub const NON_LINEAR_COLORSPACE: u32 = 1 << 8;
}

/// `FfxApiDispatchFsrUpscaleFlags`.
pub mod dispatch {
    pub const DRAW_DEBUG_VIEW: u32 = 1 << 0;
    pub const NON_LINEAR_COLOR_SRGB: u32 = 1 << 1;
}

#[repr(C)]
struct CreateUpscale {
    header: Header,
    flags: u32,
    max_render_size: Size2,
    max_upscale_size: Size2,
    message: *const core::ffi::c_void,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct DispatchUpscale {
    header: Header,
    command_list: *mut core::ffi::c_void,
    color: Resource,
    depth: Resource,
    motion_vectors: Resource,
    exposure: Resource,
    reactive: Resource,
    transparency_and_composition: Resource,
    output: Resource,
    jitter_offset: Float2,
    motion_vector_scale: Float2,
    render_size: Size2,
    upscale_size: Size2,
    enable_sharpening: bool,
    sharpness: f32,
    frame_time_delta: f32,
    pre_exposure: f32,
    reset: bool,
    camera_near: f32,
    camera_far: f32,
    camera_fov_vertical: f32,
    view_space_to_meters: f32,
    flags: u32,
}

#[repr(C)]
struct QueryJitterPhaseCount {
    header: Header,
    render_width: u32,
    display_width: u32,
    out_phase_count: *mut i32,
}

#[repr(C)]
struct QueryJitterOffset {
    header: Header,
    index: i32,
    phase_count: i32,
    out_x: *mut f32,
    out_y: *mut f32,
}

const _: () = {
    use std::mem::{offset_of, size_of};
    assert!(size_of::<CreateUpscale>() == 48 && offset_of!(CreateUpscale, max_render_size) == 20 && offset_of!(CreateUpscale, message) == 40);
    assert!(size_of::<DispatchUpscale>() == 432 && offset_of!(DispatchUpscale, color) == 24 && offset_of!(DispatchUpscale, output) == 312);
    assert!(offset_of!(DispatchUpscale, jitter_offset) == 360 && offset_of!(DispatchUpscale, render_size) == 376);
    assert!(offset_of!(DispatchUpscale, upscale_size) == 384 && offset_of!(DispatchUpscale, enable_sharpening) == 392);
    assert!(offset_of!(DispatchUpscale, sharpness) == 396 && offset_of!(DispatchUpscale, pre_exposure) == 404);
    assert!(offset_of!(DispatchUpscale, reset) == 408 && offset_of!(DispatchUpscale, camera_near) == 412);
    assert!(offset_of!(DispatchUpscale, view_space_to_meters) == 424 && offset_of!(DispatchUpscale, flags) == 428);
    assert!(size_of::<QueryJitterPhaseCount>() == 32 && offset_of!(QueryJitterPhaseCount, out_phase_count) == 24);
    assert!(size_of::<QueryJitterOffset>() == 40 && offset_of!(QueryJitterOffset, out_x) == 24 && offset_of!(QueryJitterOffset, out_y) == 32);
};

/// The kind of a description header (its first field), for a producer reading the game's own calls.
pub const DISPATCH_UPSCALE_KIND: u64 = DISPATCH_UPSCALE;

/// Whether `desc` (the game's argument to `ffxCreateContext`) creates an upscale context.
pub fn creates_upscaler(desc: *const core::ffi::c_void) -> bool {
    monaka_hook::mem::read::<u64>(desc as usize) == Some(CREATE_UPSCALE)
}

/// The flags ([`create`]) of the upscale context `desc` creates (the game's argument to
/// `ffxCreateContext`), if it creates one.
pub fn create_flags(desc: *const core::ffi::c_void) -> Option<u32> {
    creates_upscaler(desc).then(|| monaka_hook::mem::read::<u32>(desc as usize + std::mem::offset_of!(CreateUpscale, flags))).flatten()
}

/// A resource of a game's own upscale dispatch, as the game described it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GameResource {
    /// The `ID3D12Resource` (raw pointer; borrowed from the game).
    pub resource: usize,
    /// FFX surface format ([`crate::surface_format`]'s values).
    pub format: u32,
    pub width: u32,
    pub height: u32,
    /// FFX resource state.
    pub state: u32,
}

impl GameResource {
    fn from(r: &Resource) -> Self {
        Self { resource: r.resource as usize, format: r.description.format, width: r.description.width, height: r.description.height, state: r.state }
    }
}

/// A game's own upscale dispatch (`ffxDispatchDescUpscale`), read where a producer intercepts the
/// game's `ffxDispatch`: what the game renders each frame with, for per-eye upscaling and frame
/// generation of the game's frames.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GameDispatch {
    pub command_list: usize,
    pub color: GameResource,
    pub depth: GameResource,
    pub motion_vectors: GameResource,
    pub reactive: GameResource,
    pub transparency_and_composition: GameResource,
    pub output: GameResource,
    pub jitter: Float2,
    pub motion_vector_scale: Float2,
    pub render_size: Size2,
    pub upscale_size: Size2,
    pub sharpening: Option<f32>,
    pub frame_time_ms: f32,
    pub pre_exposure: f32,
    pub reset: bool,
    pub near: f32,
    pub far: f32,
    pub fov_vertical: f32,
    pub view_space_to_meters: f32,
    pub flags: u32,
}

/// The upscale dispatch at `desc`, if that is one (its header's kind).
///
/// # Safety
/// `desc` must point to a live FFX dispatch description (the game's argument to `ffxDispatch`).
pub unsafe fn read_game_dispatch(desc: *const core::ffi::c_void) -> Option<GameDispatch> {
    if desc.is_null() {
        return None;
    }
    // SAFETY: the caller's guarantee; the header is read first, the rest only for its kind.
    let header = unsafe { &*(desc as *const Header) };
    if header.kind != DISPATCH_UPSCALE {
        return None;
    }
    // SAFETY: as above, a description of this kind has this layout.
    let d = unsafe { &*(desc as *const DispatchUpscale) };
    Some(GameDispatch {
        command_list: d.command_list as usize,
        color: GameResource::from(&d.color),
        depth: GameResource::from(&d.depth),
        motion_vectors: GameResource::from(&d.motion_vectors),
        reactive: GameResource::from(&d.reactive),
        transparency_and_composition: GameResource::from(&d.transparency_and_composition),
        output: GameResource::from(&d.output),
        jitter: d.jitter_offset,
        motion_vector_scale: d.motion_vector_scale,
        render_size: d.render_size,
        upscale_size: d.upscale_size,
        sharpening: d.enable_sharpening.then_some(d.sharpness),
        frame_time_ms: d.frame_time_delta,
        pre_exposure: d.pre_exposure,
        reset: d.reset,
        near: d.camera_near,
        far: d.camera_far,
        fov_vertical: d.camera_fov_vertical,
        view_space_to_meters: d.view_space_to_meters,
        flags: d.flags,
    })
}

/// A copy of a game's own upscale dispatch, changed and passed on in its place (per-eye upscaling
/// of the game's frames: this eye's motion vectors, this eye's reset). The rest of the game's
/// description chain is linked as it was.
#[derive(Clone, Copy)]
pub struct GameDispatchCopy(DispatchUpscale);

impl GameDispatchCopy {
    /// A copy of the upscale dispatch at `desc`, if that is one.
    ///
    /// # Safety
    /// As [`read_game_dispatch`].
    pub unsafe fn of(desc: *const core::ffi::c_void) -> Option<Self> {
        // SAFETY: the caller's guarantee; the kind is checked before the rest is read.
        unsafe { read_game_dispatch(desc) }?;
        // SAFETY: as above, a description of this kind has this layout.
        Some(Self(unsafe { *(desc as *const DispatchUpscale) }))
    }

    pub fn set_motion_vectors(&mut self, motion_vectors: Resource) {
        self.0.motion_vectors = motion_vectors;
    }

    pub fn set_reset(&mut self, reset: bool) {
        self.0.reset = reset;
    }

    /// The description to pass to `ffxDispatch` (valid while this copy lives).
    pub fn as_desc(&self) -> *const core::ffi::c_void {
        &self.0 as *const DispatchUpscale as *const core::ffi::c_void
    }

    /// A game's upscale dispatch made here (for a test): on `list`, `frame` the depth and motion
    /// vectors (both in `state`) with the camera and sizes it gives.
    pub fn made(list: &ID3D12GraphicsCommandList, depth: Resource, motion_vectors: Resource, frame: &Frame) -> Self {
        Self(DispatchUpscale {
            header: Header::new(DISPATCH_UPSCALE),
            command_list: list.as_raw(),
            color: Resource::default(),
            depth,
            motion_vectors,
            exposure: Resource::default(),
            reactive: Resource::default(),
            transparency_and_composition: Resource::default(),
            output: Resource::default(),
            jitter_offset: frame.jitter,
            motion_vector_scale: frame.motion_vector_scale,
            render_size: frame.render_size,
            upscale_size: Size2 { width: frame.render_size.width * 3 / 2, height: frame.render_size.height * 3 / 2 },
            enable_sharpening: false,
            sharpness: 0.0,
            frame_time_delta: frame.frame_time_ms,
            pre_exposure: 1.0,
            reset: frame.reset,
            camera_near: frame.near,
            camera_far: frame.far,
            camera_fov_vertical: frame.fov_vertical,
            view_space_to_meters: 1.0,
            flags: frame.flags,
        })
    }
}

/// One frame to upscale: its resources (each read in its given state; the output a UAV texture of
/// the display size) and how it was rendered.
pub struct Frame {
    pub color: Resource,
    pub depth: Resource,
    pub motion_vectors: Resource,
    /// Optional: how much each pixel should ignore its history (R8, render size).
    pub reactive: Option<Resource>,
    pub output: Resource,
    pub render_size: Size2,
    /// Pixel-space jitter the frame was rendered with.
    pub jitter: Float2,
    /// Converts the motion vectors to pixels.
    pub motion_vector_scale: Float2,
    /// `None`: no sharpening; otherwise 0..1.
    pub sharpness: Option<f32>,
    pub frame_time_ms: f32,
    /// Drops the history (a cut, or the first frame).
    pub reset: bool,
    pub near: f32,
    pub far: f32,
    pub fov_vertical: f32,
    /// [`dispatch`] flags.
    pub flags: u32,
}

/// One upscaling context: one stream of frames (one eye).
pub struct Upscaler {
    context: OwnedContext,
    display: Size2,
}

impl Upscaler {
    /// A context writing `display`-size frames from frames rendered at up to `max_render`; `flags`
    /// from [`create`].
    pub fn new(api: Api, device: &ID3D12Device, max_render: Size2, display: Size2, flags: u32) -> Result<Self, Error> {
        let backend = CreateBackendDx12 { header: Header::new(crate::CREATE_BACKEND_DX12), device: device.as_raw() };
        let desc = CreateUpscale {
            header: Header { kind: CREATE_UPSCALE, next: &backend.header },
            flags,
            max_render_size: max_render,
            max_upscale_size: display,
            message: std::ptr::null(),
        };
        // SAFETY: valid descriptors, live for the call.
        let context = unsafe { OwnedContext::create(api, "ffxCreateContext (upscale)", &desc.header) }?;
        Ok(Self { context, display })
    }

    /// The provider's version name (for the log).
    pub fn version(&self) -> &str {
        self.context.version()
    }

    pub fn display(&self) -> Size2 {
        self.display
    }

    /// The length of the jitter sequence for upscaling `render_width` to the display width.
    pub fn jitter_phase_count(&mut self, render_width: u32) -> Result<i32, Error> {
        let mut count = 0;
        let query = QueryJitterPhaseCount {
            header: Header::new(QUERY_JITTER_PHASE_COUNT),
            render_width,
            display_width: self.display.width,
            out_phase_count: &mut count,
        };
        // SAFETY: a query structure live for the call, writing into a local.
        unsafe { self.context.query("jitter phase count", &query.header as *const Header as *mut Header) }?;
        Ok(count)
    }

    /// The pixel-space jitter of frame `index` of a sequence of `phase_count`.
    pub fn jitter_offset(&mut self, index: i32, phase_count: i32) -> Result<Float2, Error> {
        let mut out = Float2::default();
        let query = QueryJitterOffset { header: Header::new(QUERY_JITTER_OFFSET), index, phase_count, out_x: &mut out.x, out_y: &mut out.y };
        // SAFETY: a query structure live for the call, writing into locals.
        unsafe { self.context.query("jitter offset", &query.header as *const Header as *mut Header) }?;
        Ok(out)
    }

    /// Records the upscaling of `frame` into `list`.
    pub fn dispatch(&mut self, list: &ID3D12GraphicsCommandList, frame: &Frame) -> Result<(), Error> {
        let desc = DispatchUpscale {
            header: Header::new(DISPATCH_UPSCALE),
            command_list: list.as_raw(),
            color: frame.color,
            depth: frame.depth,
            motion_vectors: frame.motion_vectors,
            exposure: Resource::default(),
            reactive: frame.reactive.unwrap_or_default(),
            transparency_and_composition: Resource::default(),
            output: frame.output,
            jitter_offset: frame.jitter,
            motion_vector_scale: frame.motion_vector_scale,
            render_size: frame.render_size,
            upscale_size: self.display,
            enable_sharpening: frame.sharpness.is_some(),
            sharpness: frame.sharpness.unwrap_or(0.0),
            frame_time_delta: frame.frame_time_ms,
            pre_exposure: 1.0,
            reset: frame.reset,
            camera_near: frame.near,
            camera_far: frame.far,
            camera_fov_vertical: frame.fov_vertical,
            view_space_to_meters: 1.0,
            flags: frame.flags,
        };
        // SAFETY: a dispatch structure live for the call, naming a list being recorded.
        unsafe { self.context.dispatch("upscale", &desc.header) }
    }
}

#[cfg(test)]
mod tests;
