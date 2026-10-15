//! AMD FSR frame generation through the FFX API, from the game's own
//! `amd_fidelityfx_loader_dx12.dll` (frame generation 3.1.5 here). Dispatched manually, without
//! AMD's frame-interpolation swapchain: one context per image stream, given each frame's depth,
//! motion vectors and camera ([`FrameGen::prepare`]), then the frame itself
//! ([`FrameGen::generate`]), which writes the frame halfway between it and the previous one.
//!
//! The structures follow the FidelityFX SDK headers (MIT; ffx_api.h, ffx_api_types.h,
//! ffx_framegeneration.h, dx12/ffx_api_dx12.h); every size and offset below is checked against an
//! MSVC compile of those headers. Frame generation 3.1.5 takes the older prepare structure with
//! the camera-info structure linked to it (the V2 prepare is newer than this DLL).
//!
//! FSR upscaling through the same API is [`upscale`].

pub mod upscale;

use monaka_hook::module::Module;
use std::ffi::c_void;
use std::fmt;
use std::path::{Path, PathBuf};
use windows::Win32::Graphics::Direct3D12::{D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS, ID3D12Device, ID3D12GraphicsCommandList, ID3D12Resource};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;
use windows::Win32::System::LibraryLoader::LoadLibraryW;
use windows::core::{HSTRING, Interface};

pub const LOADER: &str = "amd_fidelityfx_loader_dx12.dll";

const CREATE_FRAMEGENERATION: u64 = 0x20001;
pub(crate) const CREATE_BACKEND_DX12: u64 = 0x2;
const CONFIGURE_FRAMEGENERATION: u64 = 0x20002;
const DISPATCH_FRAMEGENERATION: u64 = 0x20003;
const DISPATCH_PREPARE: u64 = 0x20004;
const DISPATCH_PREPARE_CAMERA_INFO: u64 = 0x2000a;
const QUERY_PROVIDER_VERSION: u64 = 0x6;

/// `FfxApiCreateContextFramegenerationFlags`.
pub mod create {
    pub const DEPTH_INVERTED: u32 = 1 << 3;
    pub const DEPTH_INFINITE: u32 = 1 << 4;
}
/// `FFX_FRAMEGENERATION_FLAG_NO_SWAPCHAIN_CONTEXT_NOTIFY`: interpolate only, no swapchain.
const NO_SWAPCHAIN_CONTEXT_NOTIFY: u32 = 1 << 3;
/// `FfxApiResourceState`.
pub mod state {
    use windows::Win32::Graphics::Direct3D12::*;

    pub const COMMON: u32 = 1 << 0;
    pub const UNORDERED_ACCESS: u32 = 1 << 1;
    pub const COMPUTE_READ: u32 = 1 << 2;
    pub const PIXEL_READ: u32 = 1 << 3;
    /// Readable by pixel and compute shaders (`D3D12_RESOURCE_STATE_*_SHADER_RESOURCE`).
    pub const PIXEL_COMPUTE_READ: u32 = PIXEL_READ | COMPUTE_READ;
    pub const COPY_SRC: u32 = 1 << 4;
    pub const COPY_DEST: u32 = 1 << 5;
    pub const GENERIC_READ: u32 = COPY_SRC | COMPUTE_READ;
    pub const INDIRECT_ARGUMENT: u32 = 1 << 6;
    pub const PRESENT: u32 = 1 << 7;
    pub const RENDER_TARGET: u32 = 1 << 8;

    /// The D3D12 state a resource a game described to FFX is in (the D3D12 backend's own mapping),
    /// `None` for a value that is not one of FFX's states.
    pub fn d3d12(state: u32) -> Option<D3D12_RESOURCE_STATES> {
        Some(match state {
            COMMON => D3D12_RESOURCE_STATE_COMMON,
            UNORDERED_ACCESS => D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
            COMPUTE_READ => D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE,
            PIXEL_READ => D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
            PIXEL_COMPUTE_READ => D3D12_RESOURCE_STATES(D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE.0 | D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE.0),
            COPY_SRC => D3D12_RESOURCE_STATE_COPY_SOURCE,
            COPY_DEST => D3D12_RESOURCE_STATE_COPY_DEST,
            GENERIC_READ => D3D12_RESOURCE_STATE_GENERIC_READ,
            INDIRECT_ARGUMENT => D3D12_RESOURCE_STATE_INDIRECT_ARGUMENT,
            PRESENT => D3D12_RESOURCE_STATE_PRESENT,
            RENDER_TARGET => D3D12_RESOURCE_STATE_RENDER_TARGET,
            _ => return None,
        })
    }
}
const RESOURCE_TYPE_TEXTURE2D: u32 = 2;
const USAGE_UAV: u32 = 1 << 1;

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct Header {
    pub(crate) kind: u64,
    pub(crate) next: *const Header,
}

impl Header {
    pub(crate) const fn new(kind: u64) -> Self {
        Self { kind, next: std::ptr::null() }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Size2 {
    pub width: u32,
    pub height: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Float2 {
    pub x: f32,
    pub y: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Rect {
    left: i32,
    top: i32,
    width: i32,
    height: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct ResourceDescription {
    kind: u32,
    pub(crate) format: u32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    depth: u32,
    mip_count: u32,
    flags: u32,
    usage: u32,
}

/// `FfxApiResource`: a D3D12 resource, its shape and its current state (an [`state`] value).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Resource {
    pub(crate) resource: *mut c_void,
    pub(crate) description: ResourceDescription,
    pub(crate) state: u32,
}

impl Default for Resource {
    fn default() -> Self {
        Self { resource: std::ptr::null_mut(), description: ResourceDescription::default(), state: 0 }
    }
}

/// `ffxApiGetSurfaceFormatDX12` for the formats this producer passes; 0 (unknown) otherwise.
pub fn surface_format(format: DXGI_FORMAT) -> u32 {
    match format.0 {
        10 => 4,       // R16G16B16A16_FLOAT
        16 => 6,       // R32G32_FLOAT
        24 => 17,      // R10G10B10A2_UNORM
        26 => 16,      // R11G11B10_FLOAT
        28 => 10,      // R8G8B8A8_UNORM
        29 => 12,      // R8G8B8A8_UNORM_SRGB
        33 => 33,      // R16G16_TYPELESS
        34 => 18,      // R16G16_FLOAT
        39 => 37,      // R32_TYPELESS
        41 => 28,      // R32_FLOAT
        42 => 8,       // R32_UINT
        87 => 14,      // B8G8R8A8_UNORM
        91 => 15,      // B8G8R8A8_UNORM_SRGB
        _ => 0,
    }
}

impl Resource {
    /// `ffxApiGetResourceDX12` for a 2D texture, read with `format` (0: its own) in `state`. The
    /// resource is borrowed: the caller keeps it alive until the GPU has used it.
    pub fn texture(resource: &ID3D12Resource, state: u32, format: Option<DXGI_FORMAT>) -> Self {
        // SAFETY: reads a resource descriptor.
        let desc = unsafe { resource.GetDesc() };
        let usage = if desc.Flags.contains(D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS) { USAGE_UAV } else { 0 };
        Self {
            resource: resource.as_raw(),
            description: ResourceDescription {
                kind: RESOURCE_TYPE_TEXTURE2D,
                format: surface_format(format.unwrap_or(desc.Format)),
                width: desc.Width as u32,
                height: desc.Height,
                depth: desc.DepthOrArraySize as u32,
                mip_count: desc.MipLevels as u32,
                flags: 0,
                usage,
            },
            state,
        }
    }
}

#[repr(C)]
pub(crate) struct CreateBackendDx12 {
    pub(crate) header: Header,
    pub(crate) device: *mut c_void,
}

#[repr(C)]
struct CreateFrameGeneration {
    header: Header,
    flags: u32,
    display_size: Size2,
    max_render_size: Size2,
    back_buffer_format: u32,
}

#[repr(C)]
struct ConfigureFrameGeneration {
    header: Header,
    swap_chain: *mut c_void,
    present_callback: *const c_void,
    present_callback_context: *mut c_void,
    frame_generation_callback: *const c_void,
    frame_generation_callback_context: *mut c_void,
    frame_generation_enabled: bool,
    allow_async_workloads: bool,
    hudless_color: Resource,
    flags: u32,
    only_present_generated: bool,
    generation_rect: Rect,
    frame_id: u64,
}

#[repr(C)]
struct DispatchPrepare {
    header: Header,
    frame_id: u64,
    flags: u32,
    command_list: *mut c_void,
    render_size: Size2,
    jitter_offset: Float2,
    motion_vector_scale: Float2,
    frame_time_delta: f32,
    unused_reset: bool,
    camera_near: f32,
    camera_far: f32,
    camera_fov_vertical: f32,
    view_space_to_meters: f32,
    depth: Resource,
    motion_vectors: Resource,
}

#[repr(C)]
struct PrepareCameraInfo {
    header: Header,
    position: [f32; 3],
    up: [f32; 3],
    right: [f32; 3],
    forward: [f32; 3],
}

#[repr(C)]
struct DispatchFrameGeneration {
    header: Header,
    command_list: *mut c_void,
    present_color: Resource,
    outputs: [Resource; 4],
    generated_frames: u32,
    reset: bool,
    transfer_function: u32,
    min_max_luminance: [f32; 2],
    generation_rect: Rect,
    frame_id: u64,
}

#[repr(C)]
struct QueryProviderVersion {
    header: Header,
    version_id: u64,
    version_name: *const u8,
}

const _: () = {
    use std::mem::{offset_of, size_of};
    assert!(size_of::<Header>() == 16 && size_of::<Resource>() == 48 && offset_of!(Resource, state) == 40);
    assert!(size_of::<CreateBackendDx12>() == 24);
    assert!(size_of::<CreateFrameGeneration>() == 40 && offset_of!(CreateFrameGeneration, back_buffer_format) == 36);
    assert!(size_of::<ConfigureFrameGeneration>() == 144);
    assert!(offset_of!(ConfigureFrameGeneration, frame_generation_enabled) == 56 && offset_of!(ConfigureFrameGeneration, hudless_color) == 64);
    assert!(offset_of!(ConfigureFrameGeneration, flags) == 112 && offset_of!(ConfigureFrameGeneration, generation_rect) == 120);
    assert!(offset_of!(ConfigureFrameGeneration, frame_id) == 136);
    assert!(size_of::<DispatchPrepare>() == 184 && offset_of!(DispatchPrepare, command_list) == 32 && offset_of!(DispatchPrepare, frame_time_delta) == 64);
    assert!(offset_of!(DispatchPrepare, camera_near) == 72 && offset_of!(DispatchPrepare, view_space_to_meters) == 84);
    assert!(offset_of!(DispatchPrepare, depth) == 88 && offset_of!(DispatchPrepare, motion_vectors) == 136);
    assert!(size_of::<PrepareCameraInfo>() == 64);
    assert!(size_of::<DispatchFrameGeneration>() == 312 && offset_of!(DispatchFrameGeneration, outputs) == 72);
    assert!(offset_of!(DispatchFrameGeneration, generated_frames) == 264 && offset_of!(DispatchFrameGeneration, transfer_function) == 272);
    assert!(offset_of!(DispatchFrameGeneration, generation_rect) == 284 && offset_of!(DispatchFrameGeneration, frame_id) == 304);
    assert!(size_of::<QueryProviderVersion>() == 32);
};

type Context = *mut c_void;
type CreateFn = unsafe extern "C" fn(context: *mut Context, desc: *const Header, allocator: *const c_void) -> u32;
type DestroyFn = unsafe extern "C" fn(context: *mut Context, allocator: *const c_void) -> u32;
type HeaderFn = unsafe extern "C" fn(context: *mut Context, desc: *const Header) -> u32;

/// The loader's entry points.
#[derive(Clone, Copy)]
pub struct Api {
    create: CreateFn,
    destroy: DestroyFn,
    configure: HeaderFn,
    query: HeaderFn,
    dispatch: HeaderFn,
}

/// Why [`Api::load`] found no API.
#[derive(Debug)]
pub enum LoadError {
    /// A DLL to load by full path is not there.
    Missing(PathBuf),
    /// Windows refused to load a DLL (its name or path, and why).
    Os(String, windows::core::Error),
    /// The loader is in, without the FFX entry points.
    Exports,
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Missing(path) => write!(f, "{} not found", path.display()),
            LoadError::Os(name, e) => write!(f, "{name}: {e}"),
            LoadError::Exports => write!(f, "{LOADER} exports"),
        }
    }
}

impl Api {
    /// AMD's loader and its API, loading what is not in the process yet: first the `preload`
    /// DLLs, then the loader. From `dir` they are loaded by full path, which keeps the loader from
    /// searching the game's folders for a provider preloaded so; without one, by name through the
    /// DLL search path (in a game, its executable's folder, where the game ships them).
    pub fn load(dir: Option<&Path>, preload: &[&str]) -> Result<Self, LoadError> {
        for &name in preload.iter().chain([&LOADER]) {
            if Module::find(name).is_some() {
                continue;
            }
            let (target, shown) = match dir {
                Some(dir) => {
                    let path = dir.join(name);
                    if !path.exists() {
                        return Err(LoadError::Missing(path));
                    }
                    (HSTRING::from(path.as_os_str()), path.display().to_string())
                }
                None => (HSTRING::from(name), name.to_owned()),
            };
            // SAFETY: loads AMD's signed DLL, which runs only its own initialisation.
            unsafe { LoadLibraryW(&target) }.map_err(|e| LoadError::Os(shown, e))?;
        }
        Module::find(LOADER).and_then(|m| Self::from_module(&m)).ok_or(LoadError::Exports)
    }

    /// From the loader already in the process (the game loads it for its upscaler).
    pub fn from_module(module: &Module) -> Option<Self> {
        let entry = |name: &str| module.export(name);
        let (create, destroy, configure, query, dispatch) =
            (entry("ffxCreateContext")?, entry("ffxDestroyContext")?, entry("ffxConfigure")?, entry("ffxQuery")?, entry("ffxDispatch")?);
        // SAFETY: the loader's exports have these signatures (ffx_api.h).
        unsafe {
            Some(Self {
                create: std::mem::transmute::<usize, CreateFn>(create),
                destroy: std::mem::transmute::<usize, DestroyFn>(destroy),
                configure: std::mem::transmute::<usize, HeaderFn>(configure),
                query: std::mem::transmute::<usize, HeaderFn>(query),
                dispatch: std::mem::transmute::<usize, HeaderFn>(dispatch),
            })
        }
    }
}

#[derive(Debug)]
pub struct Error(pub &'static str, pub u32);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let meaning = match self.1 {
            1 => "error",
            2 => "unknown structure type",
            3 => "runtime error",
            4 => "no provider",
            5 => "out of memory",
            6 => "invalid parameter",
            7 => "structure type too new for the provider",
            _ => "unknown code",
        };
        write!(f, "{} failed: {} ({meaning})", self.0, self.1)
    }
}

fn check(what: &'static str, code: u32) -> Result<(), Error> {
    if code == 0 { Ok(()) } else { Err(Error(what, code)) }
}

/// What frame generation needs to know about one rendered frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Camera {
    pub render_size: Size2,
    /// Pixel-space jitter.
    pub jitter: Float2,
    /// Converts the motion vectors to pixels.
    pub motion_vector_scale: Float2,
    pub near: f32,
    pub far: f32,
    pub fov_vertical: f32,
    pub position: [f32; 3],
    pub up: [f32; 3],
    pub right: [f32; 3],
    pub forward: [f32; 3],
}

impl Camera {
    /// This camera for a frame rendered at `size`: Streamline's motion-vector scale normalises the
    /// vectors to the screen while frame generation wants pixels, and a far plane that is not
    /// beyond the near one (an infinite projection) gets a finite one.
    pub fn for_render(mut self, size: Size2) -> Self {
        self.render_size = size;
        self.motion_vector_scale.x *= size.width as f32;
        self.motion_vector_scale.y *= size.height as f32;
        if self.far <= self.near || !self.far.is_finite() {
            self.far = 100_000.0;
        }
        self
    }
}

/// An FFX context of ours, with the version of the provider behind it; destroyed on drop (its
/// owner waits for the GPU first). What [`FrameGen`] and [`upscale::Upscaler`] are built on.
pub(crate) struct OwnedContext {
    api: Api,
    context: Context,
    version: String,
}

// SAFETY: the context is used from one thread at a time (its owner's lock).
unsafe impl Send for OwnedContext {}

impl OwnedContext {
    /// Creates a context from `desc`, the head of a descriptor chain (the effect's create
    /// structure linked to the D3D12 backend's), and asks for its provider's version. `what` names
    /// the call in an error.
    ///
    /// # Safety
    /// `desc` must head a valid chain of FFX create structures, live for the call.
    pub(crate) unsafe fn create(api: Api, what: &'static str, desc: *const Header) -> Result<Self, Error> {
        let mut context: Context = std::ptr::null_mut();
        // SAFETY: the caller's descriptors; the loader writes the new context.
        check(what, unsafe { (api.create)(&mut context, desc, std::ptr::null()) })?;
        let mut owned = Self { api, context, version: String::new() };
        let mut query = QueryProviderVersion { header: Header::new(QUERY_PROVIDER_VERSION), version_id: 0, version_name: std::ptr::null() };
        // SAFETY: a query on our live context, written into a local; the name is a static string
        // of the provider.
        if unsafe { (api.query)(&mut owned.context, (&raw mut query).cast::<Header>()) } == 0 && !query.version_name.is_null() {
            // SAFETY: NUL-terminated.
            owned.version = unsafe { std::ffi::CStr::from_ptr(query.version_name as *const std::ffi::c_char) }.to_string_lossy().into_owned();
        }
        Ok(owned)
    }

    pub(crate) fn version(&self) -> &str {
        &self.version
    }

    /// `ffxConfigure` with `desc`.
    ///
    /// # Safety
    /// `desc` must head a valid chain of FFX configure structures for this context, live for the
    /// call.
    pub(crate) unsafe fn configure(&mut self, what: &'static str, desc: *const Header) -> Result<(), Error> {
        // SAFETY: the caller's descriptors, on our live context.
        check(what, unsafe { (self.api.configure)(&mut self.context, desc) })
    }

    /// `ffxQuery` with `desc`, whose outputs it writes.
    ///
    /// # Safety
    /// `desc` must head a valid chain of FFX query structures for this context, live for the call,
    /// with its output pointers writable.
    pub(crate) unsafe fn query(&mut self, what: &'static str, desc: *mut Header) -> Result<(), Error> {
        // SAFETY: the caller's descriptors, on our live context.
        check(what, unsafe { (self.api.query)(&mut self.context, desc) })
    }

    /// `ffxDispatch` with `desc`, recording into the command list it names.
    ///
    /// # Safety
    /// `desc` must head a valid chain of FFX dispatch structures for this context, live for the
    /// call, naming a command list being recorded.
    pub(crate) unsafe fn dispatch(&mut self, what: &'static str, desc: *const Header) -> Result<(), Error> {
        // SAFETY: the caller's descriptors, on our live context.
        check(what, unsafe { (self.api.dispatch)(&mut self.context, desc) })
    }
}

impl Drop for OwnedContext {
    fn drop(&mut self) {
        // SAFETY: destroys our own context (the owner waited for the GPU first).
        unsafe { (self.api.destroy)(&mut self.context, std::ptr::null()) };
    }
}

/// One frame-generation context: one stream of frames (one eye).
pub struct FrameGen {
    context: OwnedContext,
    display: Size2,
}

impl FrameGen {
    /// A context for frames of `display` size and `format`, with depth and motion vectors up to
    /// `max_render`; `flags` from [`create`].
    pub fn new(api: Api, device: &ID3D12Device, display: Size2, max_render: Size2, format: DXGI_FORMAT, flags: u32) -> Result<Self, Error> {
        let backend = CreateBackendDx12 { header: Header::new(CREATE_BACKEND_DX12), device: device.as_raw() };
        let desc = CreateFrameGeneration {
            header: Header { kind: CREATE_FRAMEGENERATION, next: &backend.header },
            flags,
            display_size: display,
            max_render_size: max_render,
            back_buffer_format: surface_format(format),
        };
        // SAFETY: valid descriptors, live for the call.
        let context = unsafe { OwnedContext::create(api, "ffxCreateContext", &desc.header) }?;
        let mut generator = Self { context, display };
        generator.configure(0, None)?;
        Ok(generator)
    }

    /// Sets up the next frame: its id and, if the game has one, the frame without its HUD (the
    /// back buffer's size and format), so the HUD is kept out of the interpolation and laid over
    /// the generated frame from the real one.
    pub fn configure(&mut self, frame_id: u64, hudless: Option<Resource>) -> Result<(), Error> {
        let display = self.display;
        let configure = ConfigureFrameGeneration {
            header: Header::new(CONFIGURE_FRAMEGENERATION),
            swap_chain: std::ptr::null_mut(),
            present_callback: std::ptr::null(),
            present_callback_context: std::ptr::null_mut(),
            frame_generation_callback: std::ptr::null(),
            frame_generation_callback_context: std::ptr::null_mut(),
            frame_generation_enabled: true,
            allow_async_workloads: false,
            hudless_color: hudless.unwrap_or_default(),
            flags: NO_SWAPCHAIN_CONTEXT_NOTIFY,
            only_present_generated: false,
            generation_rect: Rect { left: 0, top: 0, width: display.width as i32, height: display.height as i32 },
            frame_id,
        };
        // SAFETY: a configure structure of this effect, live for the call.
        unsafe { self.context.configure("ffxConfigure", &configure.header) }
    }

    /// The provider's version name (for the log).
    pub fn version(&self) -> &str {
        self.context.version()
    }

    /// Records the frame's depth and motion vectors (each read in the given state) into `list`.
    pub fn prepare(&mut self, list: &ID3D12GraphicsCommandList, frame_id: u64, camera: &Camera, depth: Resource, motion_vectors: Resource, frame_time_ms: f32) -> Result<(), Error> {
        let info = PrepareCameraInfo {
            header: Header::new(DISPATCH_PREPARE_CAMERA_INFO),
            position: camera.position,
            up: camera.up,
            right: camera.right,
            forward: camera.forward,
        };
        let desc = DispatchPrepare {
            header: Header { kind: DISPATCH_PREPARE, next: &info.header },
            frame_id,
            flags: 0,
            command_list: list.as_raw(),
            render_size: camera.render_size,
            jitter_offset: camera.jitter,
            motion_vector_scale: camera.motion_vector_scale,
            frame_time_delta: frame_time_ms,
            unused_reset: false,
            camera_near: camera.near,
            camera_far: camera.far,
            camera_fov_vertical: camera.fov_vertical,
            view_space_to_meters: 1.0,
            depth,
            motion_vectors,
        };
        // SAFETY: a prepare structure linked to its camera info, live for the call, naming a list
        // being recorded.
        unsafe { self.context.dispatch("prepare", &desc.header) }
    }

    /// Records the frame halfway between the previous `present` and this one into `output` (a UAV
    /// texture of the display size and format). `reset` drops the history.
    pub fn generate(&mut self, list: &ID3D12GraphicsCommandList, frame_id: u64, present: Resource, output: Resource, reset: bool) -> Result<(), Error> {
        let mut outputs = [Resource::default(); 4];
        outputs[0] = output;
        let (width, height) = (present.description.width as i32, present.description.height as i32);
        let desc = DispatchFrameGeneration {
            header: Header::new(DISPATCH_FRAMEGENERATION),
            command_list: list.as_raw(),
            present_color: present,
            outputs,
            generated_frames: 1,
            reset,
            transfer_function: 0,
            min_max_luminance: [0.0, 1.0],
            generation_rect: Rect { left: 0, top: 0, width, height },
            frame_id,
        };
        // SAFETY: a dispatch structure live for the call, naming a list being recorded.
        unsafe { self.context.dispatch("frame generation", &desc.header) }
    }
}

#[cfg(test)]
pub(crate) mod tests;
