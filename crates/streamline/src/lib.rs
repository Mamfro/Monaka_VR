//! NVIDIA Streamline as a game's renderer calls it for DLSS: the layouts of what it passes
//! (`slSetTagForFrame`'s tags, `slSetConstants`' camera, viewport handles) and the functions'
//! types, for producers that hook those calls in the renderer's import table (Dying Light 2, The
//! Beast). The hooks themselves, with the game's handlers in them: [`imports`]. DLSS with a
//! history per eye (its own viewport, its own clip-to-previous, its motion vectors rebased):
//! [`per_eye`].
//!
//! The layouts of Streamline 2.9 (Dying Light 2) and 2.8 (The Beast): every structure starts with a 32-byte header (next, GUID, version); a
//! ViewportHandle's id at +32; a Resource's type +32 (a byte), native object +40, state +64; a
//! ResourceTag (64 bytes): resource +32, buffer type +40. Reads go through [`monaka_hook::mem`], so a
//! bad pointer reads as nothing rather than faulting.

pub mod imports;
pub mod per_eye;

use monaka_core::Aligned16;
use monaka_core::depth::multiply;
use monaka_hook::module::Module;
use monaka_hook::{Hooks, Original, mem};
use monaka_producer::log;
use std::ffi::c_void;
use windows::Win32::Graphics::Direct3D12::{D3D12_RESOURCE_STATES, ID3D12GraphicsCommandList, ID3D12Resource};
use windows::core::Interface;

pub const INTERPOSER: &str = "sl.interposer.dll";

pub type SetTagFn = unsafe extern "system" fn(frame: usize, viewport: *const u8, tags: *const u8, count: u32, commands: *mut c_void) -> i32;
pub type SetConstantsFn = unsafe extern "system" fn(constants: *const u8, frame: usize, viewport: *const u8) -> i32;
pub type SetOptionsFn = unsafe extern "system" fn(viewport: *const u8, options: *const u8) -> i32;
pub type EvaluateFn = unsafe extern "system" fn(feature: u32, frame: *const u8, inputs: *const *const u8, count: u32, commands: *mut c_void) -> i32;
pub type GetNativeFn = unsafe extern "system" fn(proxy: *mut c_void, native: *mut *mut c_void) -> i32;
pub type FreeFn = unsafe extern "system" fn(feature: u32, viewport: *const u8) -> i32;
/// `slGetFeatureFunction(Feature, const char*, void*&)`.
pub type GetFeatureFunctionFn = unsafe extern "system" fn(feature: u32, name: *const u8, function: *mut *mut c_void) -> i32;

/// Streamline's interposer, when the game loaded it; otherwise `None`, logged (the renderer gets
/// no Streamline inputs).
pub fn interposer() -> Option<Module> {
    let found = Module::find(INTERPOSER);
    if found.is_none() {
        log!("{INTERPOSER} not loaded; no Streamline inputs");
    }
    found
}

/// Prepares the hooks of `renderer`'s imports of `slSetTagForFrame` and `slSetConstants`, the two
/// calls through which a renderer gives DLSS its inputs each frame: the tagged buffers and the
/// camera. The import table is swapped, no code patched. A hook that cannot be prepared is logged
/// and left out.
///
/// # Safety
/// `renderer` must be the game's renderer DLL, importing both from [`INTERPOSER`].
pub unsafe fn hook_inputs(hooks: &mut Hooks, renderer: &Module, tag: (&Original<SetTagFn>, SetTagFn), constants: (&Original<SetConstantsFn>, SetConstantsFn)) {
    // SAFETY: the imports have these signatures (sl_core_api.h).
    unsafe {
        if let Err(e) = hooks.import(tag.0, "slSetTagForFrame", renderer, INTERPOSER, "slSetTagForFrame", tag.1) {
            log!("Streamline tags not hooked: {e}");
        }
        if let Err(e) = hooks.import(constants.0, "slSetConstants", renderer, INTERPOSER, "slSetConstants", constants.1) {
            log!("Streamline constants not hooked: {e}");
        }
    }
}

/// Eye `eye`'s (1 or 2) own copy of viewport id `id`: the eye in the id's top two bits, so DLSS
/// keeps a feature and a history per eye. Dying Light 2 maps its renderer's DLSS viewport id this
/// way, The Beast the id in Streamline's viewport handle.
pub fn eye_viewport(id: u32, eye: u32) -> u32 {
    (id & 0x3fff_ffff) | (eye << 30)
}

/// [`eye_viewport`] where the id may already be an eye's: `None` for an eye other than 1 or 2, or
/// an id already using the top bits (left as it is).
pub fn own_eye_viewport(id: u32, eye: u32) -> Option<u32> {
    ((1..=2).contains(&eye) && id < 0x4000_0000).then(|| eye_viewport(id, eye))
}

/// `sl::Feature` values.
pub mod feature {
    pub const DLSS: u32 = 0;
    pub const DLSS_G: u32 = 1000;
}

/// `sl::BufferType` values the producers use.
pub mod buffer {
    pub const DEPTH: u32 = 0;
    pub const MOTION_VECTORS: u32 = 1;
    pub const HUDLESS_COLOR: u32 = 2;
    pub const SCALING_INPUT_COLOR: u32 = 3;
    pub const SCALING_OUTPUT_COLOR: u32 = 4;
    pub const UI_COLOR_AND_ALPHA: u32 = 23;
}

/// `sl::ResourceType::eTex2d` (a one-byte enum).
pub const RESOURCE_TEX2D: u8 = 0;
pub const TAG_BYTES: usize = 64;
/// More tags than this in one call is not a real frame's.
const MAX_TAGS: u32 = 16;

/// A viewport handle's id.
pub fn viewport_id(viewport: *const u8) -> Option<u32> {
    mem::read::<u32>(viewport as usize + 32)
}

/// One tagged resource.
#[derive(Clone, Copy, Debug)]
pub struct Tag {
    /// Its index in the game's array.
    pub index: usize,
    /// The `sl::BufferType`.
    pub kind: u32,
    /// The `sl::Resource` describing it, and what it describes.
    pub resource: usize,
    pub native: usize,
    pub state: u32,
    pub resource_type: u8,
    /// `native` as the pointer COM borrows from.
    raw: *mut c_void,
}

impl Tag {
    pub fn state(&self) -> D3D12_RESOURCE_STATES {
        D3D12_RESOURCE_STATES(self.state as i32)
    }

    /// The tagged D3D12 texture, borrowed for as long as the game's call runs.
    pub fn texture(&self) -> Option<&ID3D12Resource> {
        if self.resource_type != RESOURCE_TEX2D || self.raw.is_null() {
            return None;
        }
        // SAFETY: the game names a live native resource in its tag, for the duration of its call;
        // a tag is read inside that call and not kept beyond it.
        unsafe { ID3D12Resource::from_raw_borrowed(&self.raw) }
    }

    /// The tagged texture's size (width, height).
    pub fn size(&self) -> Option<(u32, u32)> {
        // SAFETY: reads a descriptor of the tagged texture, live for the game's call.
        self.texture().map(|t| unsafe { t.GetDesc() }).map(|d| (d.Width as u32, d.Height))
    }
}

/// The size of the first 2D texture tagged as buffer type `kind` in `tags`.
pub fn tagged_size(tags: &[Tag], kind: u32) -> Option<(u32, u32)> {
    tags.iter().find(|t| t.kind == kind && t.resource_type == RESOURCE_TEX2D)?.size()
}

/// The tags in the game's array of `count` (resources without a native object left out).
pub fn read_tags(tags: *const u8, count: u32) -> Vec<Tag> {
    if tags.is_null() {
        return Vec::new();
    }
    (0..count.min(MAX_TAGS) as usize)
        .filter_map(|index| {
            let at = tags as usize + index * TAG_BYTES;
            let resource = mem::read::<usize>(at + 32).filter(|&r| r != 0)?;
            let native = mem::read::<usize>(resource + 40).filter(|&n| n != 0)?;
            Some(Tag {
                index,
                kind: mem::read::<u32>(at + 40)?,
                resource,
                native,
                state: mem::read::<u32>(resource + 64).unwrap_or(0),
                resource_type: mem::read::<u8>(resource + 32).unwrap_or(0xff),
                raw: native as *mut c_void,
            })
        })
        .collect()
}

/// `sl::Constants`' offsets: the camera's matrices (4x4, row vectors), jitter, motion-vector scale,
/// position and axes, planes and flags.
pub mod constants {
    pub const CAMERA_VIEW_TO_CLIP: usize = 32;
    pub const CLIP_TO_CAMERA_VIEW: usize = 96;
    pub const CLIP_TO_LENS_CLIP: usize = 160;
    pub const CLIP_TO_PREV_CLIP: usize = 224;
    pub const PREV_CLIP_TO_CLIP: usize = 288;
    pub const JITTER: usize = 352;
    pub const MVEC_SCALE: usize = 360;
    pub const PINHOLE_OFFSET: usize = 368;
    pub const POSITION: usize = 376;
    pub const UP: usize = 388;
    pub const RIGHT: usize = 400;
    pub const FORWARD: usize = 412;
    pub const NEAR: usize = 424;
    pub const FAR: usize = 428;
    pub const FOV: usize = 432;
    pub const ASPECT: usize = 436;
    pub const MOTION_VECTORS_INVALID: usize = 440;
    /// Bytes from here: depthInverted, cameraMotionIncluded, motionVectors3D, reset.
    pub const DEPTH_INVERTED: usize = 444;
    pub const CAMERA_MOTION_INCLUDED: usize = 445;
    pub const MOTION_VECTORS_3D: usize = 446;
    pub const RESET: usize = 447;
    /// `sl::Constants` is 456 bytes in Streamline 2.8 and 2.9; copies are this generous, and
    /// Streamline reads only as far as the structure's own version says.
    pub const BYTES: usize = 1024;
}

/// A copy of a game's `sl::Constants`, to read, or to change and pass on in the game's place
/// (aligned, as the game's own).
#[derive(Clone, Copy)]
pub struct Constants(Aligned16<[u8; constants::BYTES]>);

impl Constants {
    /// A copy of the constants at `constants`.
    pub fn read(constants: *const u8) -> Option<Self> {
        let mut copy = Self(Aligned16([0; constants::BYTES]));
        (!constants.is_null() && mem::read_bytes(constants as usize, &mut copy.0.0)).then_some(copy)
    }

    pub fn as_ptr(&self) -> *const u8 {
        self.0.0.as_ptr()
    }

    pub fn f32(&self, offset: usize) -> f32 {
        f32::from_le_bytes(self.0.0[offset..offset + 4].try_into().expect("4 bytes"))
    }

    pub fn vec2(&self, offset: usize) -> [f32; 2] {
        [self.f32(offset), self.f32(offset + 4)]
    }

    pub fn vec3(&self, offset: usize) -> [f32; 3] {
        [self.f32(offset), self.f32(offset + 4), self.f32(offset + 8)]
    }

    pub fn matrix(&self, offset: usize) -> [f32; 16] {
        std::array::from_fn(|i| self.f32(offset + 4 * i))
    }

    pub fn set_matrix(&mut self, offset: usize, m: &[f32; 16]) {
        for (i, v) in m.iter().enumerate() {
            self.0.0[offset + 4 * i..offset + 4 * i + 4].copy_from_slice(&v.to_le_bytes());
        }
    }

    pub fn byte(&self, offset: usize) -> u8 {
        self.0.0[offset]
    }

    /// The game dropped its history for this frame (a cut).
    pub fn reset(&self) -> bool {
        self.byte(constants::RESET) != 0
    }

    pub fn set_reset(&mut self, reset: bool) {
        self.0.0[constants::RESET] = reset as u8;
    }

    /// The camera's view-projection (row vectors, as Streamline's matrices): world to view from its
    /// position and axes (view space looks down -z), then its view to clip.
    pub fn view_projection(&self) -> Option<[f32; 16]> {
        let (position, up, right, forward) = (self.vec3(constants::POSITION), self.vec3(constants::UP), self.vec3(constants::RIGHT), self.vec3(constants::FORWARD));
        let projection = self.matrix(constants::CAMERA_VIEW_TO_CLIP);
        let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        let view = [
            right[0], up[0], -forward[0], 0.0, //
            right[1], up[1], -forward[1], 0.0, //
            right[2], up[2], -forward[2], 0.0, //
            -dot(position, right), -dot(position, up), dot(position, forward), 1.0,
        ];
        view.iter().chain(&projection).all(|v| v.is_finite()).then(|| multiply(&view, &projection))
    }
}

/// The game's command list behind Streamline's proxy (`slGetNativeInterface`, at `get_native`):
/// work recorded on it is not tracked by Streamline, which restores the game's own state after a
/// feature evaluates. `None` when unavailable (logged once).
///
/// # Safety
/// `get_native` is Streamline's `slGetNativeInterface` (or 0), and `commands` the command list (or
/// null) a Streamline call of the game's was given.
pub unsafe fn native_list(get_native: usize, commands: *mut c_void) -> Option<ID3D12GraphicsCommandList> {
    if get_native == 0 || commands.is_null() {
        return None;
    }
    // SAFETY: Streamline's exported slGetNativeInterface(void*, void**).
    let get: GetNativeFn = unsafe { std::mem::transmute(get_native) };
    let mut native = std::ptr::null_mut();
    // SAFETY: asks Streamline for the interface behind its proxy into a local.
    let result = unsafe { get(commands, &mut native) };
    if result != 0 || native.is_null() {
        monaka_producer::log_first(1, format_args!("slGetNativeInterface returned {result}: no native command list"));
        return None;
    }
    // Borrowed, not owned: whether Streamline added a reference is not documented here, and
    // releasing one it did not add would free the game's list (a reference too many only leaks).
    // SAFETY: a live COM interface for the duration of this call; the cast adds its own reference.
    let unknown = unsafe { windows::core::IUnknown::from_raw_borrowed(&native) }?;
    unknown.cast().ok()
}

/// Logs a renderer's tags once (their buffer types, sizes and states), to see what a game gives.
pub fn log_tags(label: &str, tags: &[Tag]) {
    for tag in tags {
        match tag.texture() {
            // SAFETY: reads a descriptor.
            Some(texture) => {
                let d = unsafe { texture.GetDesc() };
                log!("{label}: tag {} type {} {}x{} format {} state {}", tag.index, tag.kind, d.Width, d.Height, d.Format.0, tag.state);
            }
            None => log!("{label}: tag {} type {} (not a texture)", tag.index, tag.kind),
        }
    }
}

#[cfg(test)]
mod tests;
