//! OpenGL games into the D3D11 channels. A GL framebuffer (the default back buffer or any
//! framebuffer object) is copied, flipped to top-down, into a D3D11 texture on our own device
//! through `WGL_NV_DX_interop2`; from there the fence channel, the alternate-eye publisher and the
//! depth warp work as for a Direct3D game.
//!
//! A [`Canvas`] is a framebuffer of our own the game's own draw calls can be run into (a HUD piece
//! cut out of its 2D layer), captured the same way.
//!
//! A GL depth texture goes into a D3D11 `R32_FLOAT` texture through a small shader pass
//! ([`Bridge::capture_depth`], [`depth`]), for a depth warp or an upscaler.
//!
//! Everything here runs on the game's render thread with its GL context current (inside its
//! `SwapBuffers`, see `monaka_hook::Hooks::import`). GL state the copy touches (framebuffer
//! bindings, read buffer, scissor test, sRGB writes) is saved and put back.

mod depth;

pub use depth::texel_rect;
use depth::DepthPass;
use monaka_channel::d3d;
use std::ffi::c_void;
use std::fmt;
use windows::Win32::Graphics::Direct3D11::{D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, ID3D11Device, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_FORMAT_R32_FLOAT, DXGI_FORMAT_R8G8B8A8_UNORM};
use windows::Win32::Graphics::OpenGL::{
    GL_BACK, GL_COLOR_BUFFER_BIT, GL_COLOR_CLEAR_VALUE, GL_COLOR_WRITEMASK, GL_DEPTH_BUFFER_BIT, GL_DEPTH_CLEAR_VALUE, GL_DEPTH_WRITEMASK,
    GL_NEAREST, GL_RGBA, GL_RGBA8, GL_SCISSOR_TEST, GL_STENCIL_BUFFER_BIT, GL_STENCIL_CLEAR_VALUE, GL_STENCIL_WRITEMASK, GL_TEXTURE_2D,
    GL_TEXTURE_BINDING_2D, GL_UNSIGNED_BYTE, GL_VIEWPORT, glBindTexture, glClear, glClearColor, glClearDepth, glClearStencil,
    glColorMask, glDeleteTextures, glDepthMask, glDisable, glEnable, glGenTextures, glGetBooleanv, glGetDoublev, glGetFloatv,
    glGetIntegerv, glIsEnabled, glReadBuffer, glStencilMask, glTexImage2D, glViewport, wglGetCurrentContext, wglGetProcAddress,
};
use windows::core::{Interface, PCSTR};

const GL_READ_FRAMEBUFFER: u32 = 0x8CA8;
const GL_DRAW_FRAMEBUFFER: u32 = 0x8CA9;
const GL_READ_FRAMEBUFFER_BINDING: u32 = 0x8CAA;
const GL_DRAW_FRAMEBUFFER_BINDING: u32 = 0x8CA6;
const GL_READ_BUFFER: u32 = 0x0C02;
const GL_COLOR_ATTACHMENT0: u32 = 0x8CE0;
const GL_FRAMEBUFFER_COMPLETE: u32 = 0x8CD5;
const GL_FRAMEBUFFER_SRGB: u32 = 0x8DB9;
const WGL_ACCESS_WRITE_DISCARD_NV: u32 = 0x0002;
const GL_RENDERBUFFER: u32 = 0x8D41;
const GL_DEPTH24_STENCIL8: u32 = 0x88F0;
const GL_DEPTH_STENCIL_ATTACHMENT: u32 = 0x821A;

#[derive(Debug)]
pub enum Error {
    /// No GL context is current on this thread.
    NoContext,
    /// The driver lacks a function (named); `WGL_NV_DX_interop2` needs an NVIDIA or AMD driver.
    Missing(&'static str),
    /// A GL or interop call failed (named).
    Failed(&'static str),
    D3d(monaka_channel::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NoContext => f.write_str("no OpenGL context is current"),
            Error::Missing(name) => write!(f, "the driver does not provide {name}"),
            Error::Failed(what) => write!(f, "{what} failed"),
            Error::D3d(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

type GenFramebuffers = unsafe extern "system" fn(i32, *mut u32);
type DeleteFramebuffers = unsafe extern "system" fn(i32, *const u32);
type BindFramebuffer = unsafe extern "system" fn(u32, u32);
type FramebufferTexture2D = unsafe extern "system" fn(u32, u32, u32, u32, i32);
type CheckFramebufferStatus = unsafe extern "system" fn(u32) -> u32;
type BlitFramebuffer = unsafe extern "system" fn(i32, i32, i32, i32, i32, i32, i32, i32, u32, u32);
type GenRenderbuffers = unsafe extern "system" fn(i32, *mut u32);
type DeleteRenderbuffers = unsafe extern "system" fn(i32, *const u32);
type BindRenderbuffer = unsafe extern "system" fn(u32, u32);
type RenderbufferStorage = unsafe extern "system" fn(u32, u32, i32, i32);
type FramebufferRenderbuffer = unsafe extern "system" fn(u32, u32, u32, u32);
type DxOpenDevice = unsafe extern "system" fn(*mut c_void) -> *mut c_void;
type DxCloseDevice = unsafe extern "system" fn(*mut c_void) -> i32;
type DxRegisterObject = unsafe extern "system" fn(*mut c_void, *mut c_void, u32, u32, u32) -> *mut c_void;
type DxUnregisterObject = unsafe extern "system" fn(*mut c_void, *mut c_void) -> i32;
type DxLockObjects = unsafe extern "system" fn(*mut c_void, i32, *mut *mut c_void) -> i32;

/// GL entry points beyond OpenGL 1.1, loaded from the current context's driver.
#[derive(Clone, Copy)]
pub struct Functions {
    gen_framebuffers: GenFramebuffers,
    delete_framebuffers: DeleteFramebuffers,
    bind_framebuffer: BindFramebuffer,
    framebuffer_texture_2d: FramebufferTexture2D,
    check_framebuffer_status: CheckFramebufferStatus,
    blit_framebuffer: BlitFramebuffer,
    gen_renderbuffers: GenRenderbuffers,
    delete_renderbuffers: DeleteRenderbuffers,
    bind_renderbuffer: BindRenderbuffer,
    renderbuffer_storage: RenderbufferStorage,
    framebuffer_renderbuffer: FramebufferRenderbuffer,
    open_device: DxOpenDevice,
    close_device: DxCloseDevice,
    register_object: DxRegisterObject,
    unregister_object: DxUnregisterObject,
    lock_objects: DxLockObjects,
    unlock_objects: DxLockObjects,
}

pub(crate) fn load<F: Copy>(name: &'static str) -> Result<F> {
    let mut text = name.as_bytes().to_vec();
    text.push(0);
    // SAFETY: asks the current context's driver for a function by NUL-terminated name.
    let function = unsafe { wglGetProcAddress(PCSTR(text.as_ptr())) }.ok_or(Error::Missing(name))?;
    // Some drivers return small sentinel values instead of null for missing functions.
    if (function as usize) < 4 {
        return Err(Error::Missing(name));
    }
    assert_eq!(size_of::<F>(), size_of::<usize>());
    // SAFETY: the driver's function with the signature `F` documents for it.
    Ok(unsafe { std::mem::transmute_copy(&function) })
}

impl Functions {
    /// Loads the functions; a GL context must be current.
    pub fn load() -> Result<Self> {
        // SAFETY: queries this thread's current context.
        if unsafe { wglGetCurrentContext() }.is_invalid() {
            return Err(Error::NoContext);
        }
        Ok(Self {
            gen_framebuffers: load("glGenFramebuffers")?,
            delete_framebuffers: load("glDeleteFramebuffers")?,
            bind_framebuffer: load("glBindFramebuffer")?,
            framebuffer_texture_2d: load("glFramebufferTexture2D")?,
            check_framebuffer_status: load("glCheckFramebufferStatus")?,
            blit_framebuffer: load("glBlitFramebuffer")?,
            gen_renderbuffers: load("glGenRenderbuffers")?,
            delete_renderbuffers: load("glDeleteRenderbuffers")?,
            bind_renderbuffer: load("glBindRenderbuffer")?,
            renderbuffer_storage: load("glRenderbufferStorage")?,
            framebuffer_renderbuffer: load("glFramebufferRenderbuffer")?,
            open_device: load("wglDXOpenDeviceNV")?,
            close_device: load("wglDXCloseDeviceNV")?,
            register_object: load("wglDXRegisterObjectNV")?,
            unregister_object: load("wglDXUnregisterObjectNV")?,
            lock_objects: load("wglDXLockObjectsNV")?,
            unlock_objects: load("wglDXUnlockObjectsNV")?,
        })
    }
}

/// One shared texture: a D3D11 texture and the GL texture and framebuffer that alias it.
struct Target {
    texture: ID3D11Texture2D,
    gl_texture: u32,
    framebuffer: u32,
    object: *mut c_void,
    width: u32,
    height: u32,
}

/// GL state the copy changes, restored afterwards.
struct SavedState {
    read: i32,
    draw: i32,
    read_buffer: i32,
    scissor: bool,
    srgb: bool,
}

pub struct Bridge {
    gl: Functions,
    device: ID3D11Device,
    interop: *mut c_void,
    target: Option<Target>,
    /// The depth copy's target (`R32_FLOAT`) and pass, made at the first depth copy.
    depth_target: Option<Target>,
    depth_pass: Option<DepthPass>,
    /// [`Bridge::capture_eye`]'s slots, made as they are first used.
    eye_slots: Vec<EyeSlot>,
}

/// One slot of [`Bridge::capture_eye`]: an eye's colour target and the depth target beside it.
#[derive(Default)]
struct EyeSlot {
    colour: Option<Target>,
    depth: Option<Target>,
}

/// The depth [`Bridge::capture_eye`] copies with the eye: GL depth texture `texture` (`size` its
/// width and height; `multisampled`: its sample 0 is read) over the part `region` covers (x, y from
/// the top-left, width, height: fractions).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DepthSource {
    pub texture: u32,
    pub size: (u32, u32),
    pub multisampled: bool,
    pub region: [f32; 4],
}

impl Bridge {
    /// Opens interop between the current GL context and `device` (which must be on the same GPU).
    pub fn new(device: &ID3D11Device) -> Result<Self> {
        let gl = Functions::load()?;
        // SAFETY: hands the driver our live D3D11 device.
        let interop = unsafe { (gl.open_device)(device.as_raw()) };
        if interop.is_null() {
            return Err(Error::Failed("wglDXOpenDeviceNV (is the D3D11 device on the GL context's GPU?)"));
        }
        Ok(Self { gl, device: device.clone(), interop, target: None, depth_target: None, depth_pass: None, eye_slots: Vec::new() })
    }

    pub fn device(&self) -> &ID3D11Device {
        &self.device
    }

    fn release_target(&mut self) {
        let target = self.target.take();
        self.free(target);
    }

    fn free(&self, target: Option<Target>) {
        if let Some(target) = target {
            // SAFETY: deletes the GL objects and the registration this bridge created, with the
            // context current.
            unsafe {
                (self.gl.unregister_object)(self.interop, target.object);
                (self.gl.delete_framebuffers)(1, &target.framebuffer);
                glDeleteTextures(1, &target.gl_texture);
            }
        }
    }

    fn target(&mut self, width: u32, height: u32) -> Result<&Target> {
        if self.target.as_ref().is_some_and(|t| t.width == width && t.height == height) {
            return Ok(self.target.as_ref().expect("checked"));
        }
        self.release_target();
        self.target = Some(self.make_target(width, height, DXGI_FORMAT_R8G8B8A8_UNORM)?);
        Ok(self.target.as_ref().expect("just made"))
    }

    fn depth_target(&mut self, width: u32, height: u32) -> Result<&Target> {
        if self.depth_target.as_ref().is_some_and(|t| t.width == width && t.height == height) {
            return Ok(self.depth_target.as_ref().expect("checked"));
        }
        let old = self.depth_target.take();
        self.free(old);
        self.depth_target = Some(self.make_target(width, height, DXGI_FORMAT_R32_FLOAT)?);
        Ok(self.depth_target.as_ref().expect("just made"))
    }

    /// A D3D11 texture of `format` shared as a GL texture, with a framebuffer drawing into it.
    fn make_target(&self, width: u32, height: u32, format: DXGI_FORMAT) -> Result<Target> {
        let mut desc = d3d::simple_texture_desc(width, height, format);
        desc.BindFlags = (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32;
        let texture = d3d::create_texture(&self.device, &desc).map_err(Error::D3d)?;
        let gl = self.gl;
        // SAFETY: creates GL objects with the context current and registers our D3D11 texture
        // as the GL texture's storage; the framebuffer is completed while the object is locked.
        unsafe {
            let mut gl_texture = 0u32;
            glGenTextures(1, &mut gl_texture);
            let mut object = (gl.register_object)(self.interop, texture.as_raw(), gl_texture, GL_TEXTURE_2D, WGL_ACCESS_WRITE_DISCARD_NV);
            if object.is_null() {
                glDeleteTextures(1, &gl_texture);
                return Err(Error::Failed("wglDXRegisterObjectNV"));
            }
            let mut framebuffer = 0u32;
            (gl.gen_framebuffers)(1, &mut framebuffer);
            let mut previous = 0i32;
            glGetIntegerv(GL_DRAW_FRAMEBUFFER_BINDING, &mut previous);
            (gl.lock_objects)(self.interop, 1, &mut object);
            (gl.bind_framebuffer)(GL_DRAW_FRAMEBUFFER, framebuffer);
            (gl.framebuffer_texture_2d)(GL_DRAW_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_2D, gl_texture, 0);
            let complete = (gl.check_framebuffer_status)(GL_DRAW_FRAMEBUFFER) == GL_FRAMEBUFFER_COMPLETE;
            (gl.bind_framebuffer)(GL_DRAW_FRAMEBUFFER, previous as u32);
            (gl.unlock_objects)(self.interop, 1, &mut object);
            let target = Target { texture, gl_texture, framebuffer, object, width, height };
            if !complete {
                self.free(Some(target));
                return Err(Error::Failed("the shared framebuffer is incomplete"));
            }
            Ok(target)
        }
    }

    /// `target` when it is `width` x `height`, else freed and made again at that size in `format`.
    fn sized(&self, target: Option<Target>, width: u32, height: u32, format: DXGI_FORMAT) -> Result<Target> {
        match target {
            Some(target) if target.width == width && target.height == height => Ok(target),
            other => {
                self.free(other);
                self.make_target(width, height, format)
            }
        }
    }

    fn save() -> SavedState {
        let mut state = SavedState { read: 0, draw: 0, read_buffer: 0, scissor: false, srgb: false };
        // SAFETY: GL state queries into locals, context current.
        unsafe {
            glGetIntegerv(GL_READ_FRAMEBUFFER_BINDING, &mut state.read);
            glGetIntegerv(GL_DRAW_FRAMEBUFFER_BINDING, &mut state.draw);
            glGetIntegerv(GL_READ_BUFFER, &mut state.read_buffer);
            state.scissor = glIsEnabled(GL_SCISSOR_TEST) != 0;
            state.srgb = glIsEnabled(GL_FRAMEBUFFER_SRGB) != 0;
        }
        state
    }

    /// Copies the `width` x `height` colour of GL framebuffer `source` (0: the default back buffer)
    /// into a D3D11 texture, top row first. The texture is reused until the size changes.
    pub fn capture(&mut self, source: u32, width: u32, height: u32) -> Result<&ID3D11Texture2D> {
        if !monaka_contract::protocol::dimensions_ok(width, height) {
            return Err(Error::Failed("capture size"));
        }
        let gl = self.gl;
        let interop = self.interop;
        let saved = Self::save();
        let target = self.target(width, height)?;
        let mut object = target.object;
        let framebuffer = target.framebuffer;
        let (w, h) = (width as i32, height as i32);
        // SAFETY: GL calls with the context current, on objects this bridge owns, bracketed by the
        // interop lock; the state changed is restored before returning.
        unsafe {
            if (gl.lock_objects)(interop, 1, &mut object) == 0 {
                return Err(Error::Failed("wglDXLockObjectsNV"));
            }
            (gl.bind_framebuffer)(GL_READ_FRAMEBUFFER, source);
            if source == 0 {
                glReadBuffer(GL_BACK);
            }
            (gl.bind_framebuffer)(GL_DRAW_FRAMEBUFFER, framebuffer);
            glDisable(GL_SCISSOR_TEST);
            glDisable(GL_FRAMEBUFFER_SRGB);
            // GL rows run bottom-up, D3D's top-down: the destination is flipped.
            (gl.blit_framebuffer)(0, 0, w, h, 0, h, w, 0, GL_COLOR_BUFFER_BIT, GL_NEAREST);
            Self::restore(gl, &saved, source);
            if (gl.unlock_objects)(interop, 1, &mut object) == 0 {
                return Err(Error::Failed("wglDXUnlockObjectsNV"));
            }
        }
        Ok(&self.target.as_ref().expect("made above").texture)
    }

    /// Puts back the state [`Bridge::save`] read, after a copy from framebuffer `source`.
    ///
    /// # Safety
    /// The GL context is current.
    unsafe fn restore(gl: Functions, saved: &SavedState, source: u32) {
        // SAFETY: binds and enables of the state read before, context current (the caller's).
        unsafe {
            (gl.bind_framebuffer)(GL_READ_FRAMEBUFFER, saved.read as u32);
            (gl.bind_framebuffer)(GL_DRAW_FRAMEBUFFER, saved.draw as u32);
            if source == 0 || saved.read == 0 {
                glReadBuffer(saved.read_buffer as u32);
            }
            if saved.scissor {
                glEnable(GL_SCISSOR_TEST);
            }
            if saved.srgb {
                glEnable(GL_FRAMEBUFFER_SRGB);
            }
        }
    }

    /// Copies the eye at `rect` (pixels: left, top from the top-left, width, height) of GL
    /// framebuffer `source` (0: the default back buffer; `frame` its width and height), and with
    /// `depth` the depth under it, into slot `slot`'s D3D11 textures at the eye's size (`RGBA8`,
    /// `R32_FLOAT`), top row first, under one interop lock. A slot's textures are reused until the
    /// size changes, so a slot goes back to this copy only once D3D11 is done reading it.
    pub fn capture_eye(&mut self, slot: usize, source: u32, frame: (u32, u32), rect: [u32; 4], depth: Option<DepthSource>) -> Result<(ID3D11Texture2D, Option<ID3D11Texture2D>)> {
        let [left, top, width, height] = rect;
        let inside = left.checked_add(width).is_some_and(|right| right <= frame.0) && top.checked_add(height).is_some_and(|bottom| bottom <= frame.1);
        if !monaka_contract::protocol::dimensions_ok(width, height) || !inside {
            return Err(Error::Failed("eye capture rect"));
        }
        if depth.is_some_and(|d| !monaka_contract::protocol::dimensions_ok(d.size.0, d.size.1)) {
            return Err(Error::Failed("depth capture size"));
        }
        if depth.is_some() && self.depth_pass.is_none() {
            self.depth_pass = Some(DepthPass::new()?);
        }
        if self.eye_slots.len() <= slot {
            self.eye_slots.resize_with(slot + 1, EyeSlot::default);
        }
        let colour = self.eye_slots[slot].colour.take();
        self.eye_slots[slot].colour = Some(self.sized(colour, width, height, DXGI_FORMAT_R8G8B8A8_UNORM)?);
        if depth.is_some() {
            let old = self.eye_slots[slot].depth.take();
            self.eye_slots[slot].depth = Some(self.sized(old, width, height, DXGI_FORMAT_R32_FLOAT)?);
        }
        let gl = self.gl;
        let interop = self.interop;
        let made = &self.eye_slots[slot];
        let colour = made.colour.as_ref().expect("made above");
        let depth_target = made.depth.as_ref().filter(|_| depth.is_some());
        // The colour, and the depth when there is one (the count says how many).
        let mut objects: [*mut c_void; 2] = [colour.object, depth_target.map_or(std::ptr::null_mut(), |t| t.object)];
        let count = 1 + depth_target.is_some() as i32;
        let saved = Self::save();
        // SAFETY: the shared textures are locked around the GL copies into them, context current;
        // the state changed is restored before the unlock.
        unsafe {
            if (gl.lock_objects)(interop, count, objects.as_mut_ptr()) == 0 {
                return Err(Error::Failed("wglDXLockObjectsNV"));
            }
            (gl.bind_framebuffer)(GL_READ_FRAMEBUFFER, source);
            if source == 0 {
                glReadBuffer(GL_BACK);
            }
            (gl.bind_framebuffer)(GL_DRAW_FRAMEBUFFER, colour.framebuffer);
            glDisable(GL_SCISSOR_TEST);
            glDisable(GL_FRAMEBUFFER_SRGB);
            // GL rows run bottom-up, D3D's top-down: the source counts from the bottom, the
            // destination is flipped.
            let (x, y) = (left as i32, (frame.1 - top - height) as i32);
            let (w, h) = (width as i32, height as i32);
            (gl.blit_framebuffer)(x, y, x + w, y + h, 0, h, w, 0, GL_COLOR_BUFFER_BIT, GL_NEAREST);
            Self::restore(gl, &saved, source);
        }
        let drawn = match (depth, depth_target) {
            (Some(d), Some(target)) => {
                let pass = self.depth_pass.as_mut().expect("made above");
                pass.draw(d.texture, d.multisampled, texel_rect(d.size.0, d.size.1, d.region), target.framebuffer, width, height, gl.bind_framebuffer)
            }
            _ => Ok(()),
        };
        // SAFETY: unlocks the objects locked above.
        if unsafe { (gl.unlock_objects)(interop, count, objects.as_mut_ptr()) } == 0 {
            return Err(Error::Failed("wglDXUnlockObjectsNV"));
        }
        drawn?;
        let made = &self.eye_slots[slot];
        Ok((made.colour.as_ref().expect("made above").texture.clone(), made.depth.as_ref().filter(|_| depth.is_some()).map(|t| t.texture.clone())))
    }

    /// Copies the depth of GL texture `source` (`size` its width and height; `multisampled`: its
    /// sample 0), the part `region` covers (x, y from the top-left, width, height: fractions), into
    /// a `width` x `height` D3D11 `R32_FLOAT` texture, top row first: the window depth (0..1) as the
    /// game wrote it. The texture is reused until the size changes.
    pub fn capture_depth(&mut self, source: u32, size: (u32, u32), multisampled: bool, region: [f32; 4], width: u32, height: u32) -> Result<&ID3D11Texture2D> {
        if !monaka_contract::protocol::dimensions_ok(width, height) || !monaka_contract::protocol::dimensions_ok(size.0, size.1) {
            return Err(Error::Failed("depth capture size"));
        }
        if self.depth_pass.is_none() {
            self.depth_pass = Some(DepthPass::new()?);
        }
        let gl = self.gl;
        let interop = self.interop;
        let target = self.depth_target(width, height)?;
        let (mut object, framebuffer) = (target.object, target.framebuffer);
        let rect = texel_rect(size.0, size.1, region);
        let pass = self.depth_pass.as_mut().expect("made above");
        // SAFETY: the shared texture is locked around the GL pass that draws into it.
        if unsafe { (gl.lock_objects)(interop, 1, &mut object) } == 0 {
            return Err(Error::Failed("wglDXLockObjectsNV"));
        }
        let drawn = pass.draw(source, multisampled, rect, framebuffer, width, height, gl.bind_framebuffer);
        // SAFETY: unlocks the object locked above.
        if unsafe { (gl.unlock_objects)(interop, 1, &mut object) } == 0 {
            return Err(Error::Failed("wglDXUnlockObjectsNV"));
        }
        drawn?;
        Ok(&self.depth_target.as_ref().expect("made above").texture)
    }

    /// Frees the GL objects and closes interop; call with the GL context current. Dropping a
    /// bridge without it leaves them to the context's own teardown.
    pub fn release(mut self) {
        self.release_target();
        let depth = self.depth_target.take();
        self.free(depth);
        for slot in std::mem::take(&mut self.eye_slots) {
            self.free(slot.colour);
            self.free(slot.depth);
        }
        if let Some(pass) = self.depth_pass.take() {
            pass.release();
        }
        // SAFETY: closes the interop handle this bridge opened.
        unsafe { (self.gl.close_device)(self.interop) };
        self.interop = std::ptr::null_mut();
    }
}

/// A framebuffer of our own: a colour texture (RGBA8) with a depth-stencil buffer, so the game's
/// own draws (masks included) can be run into it ([`Canvas::draw`]) and it can be captured like any
/// framebuffer ([`Bridge::capture`] of [`Canvas::framebuffer`]).
pub struct Canvas {
    gl: Functions,
    texture: u32,
    depth_stencil: u32,
    framebuffer: u32,
    width: u32,
    height: u32,
}

/// GL state a canvas changes around the game's draws, put back after.
struct Bound {
    draw: i32,
    viewport: [i32; 4],
    scissor: bool,
}

impl Canvas {
    /// A canvas of `width` x `height`; a GL context must be current.
    pub fn new(width: u32, height: u32) -> Result<Self> {
        if !monaka_contract::protocol::dimensions_ok(width, height) {
            return Err(Error::Failed("canvas size"));
        }
        let gl = Functions::load()?;
        // SAFETY: creates GL objects with the context current; the texture and draw framebuffer
        // bindings are put back.
        unsafe {
            let mut previous = 0i32;
            glGetIntegerv(GL_DRAW_FRAMEBUFFER_BINDING, &mut previous);
            let mut texture = 0u32;
            glGenTextures(1, &mut texture);
            let mut bound_texture = 0i32;
            glGetIntegerv(GL_TEXTURE_BINDING_2D, &mut bound_texture);
            glBindTexture(GL_TEXTURE_2D, texture);
            glTexImage2D(GL_TEXTURE_2D, 0, GL_RGBA8 as i32, width as i32, height as i32, 0, GL_RGBA, GL_UNSIGNED_BYTE, std::ptr::null());
            glBindTexture(GL_TEXTURE_2D, bound_texture as u32);
            let mut depth_stencil = 0u32;
            (gl.gen_renderbuffers)(1, &mut depth_stencil);
            (gl.bind_renderbuffer)(GL_RENDERBUFFER, depth_stencil);
            (gl.renderbuffer_storage)(GL_RENDERBUFFER, GL_DEPTH24_STENCIL8, width as i32, height as i32);
            let mut framebuffer = 0u32;
            (gl.gen_framebuffers)(1, &mut framebuffer);
            (gl.bind_framebuffer)(GL_DRAW_FRAMEBUFFER, framebuffer);
            (gl.framebuffer_texture_2d)(GL_DRAW_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_2D, texture, 0);
            (gl.framebuffer_renderbuffer)(GL_DRAW_FRAMEBUFFER, GL_DEPTH_STENCIL_ATTACHMENT, GL_RENDERBUFFER, depth_stencil);
            let complete = (gl.check_framebuffer_status)(GL_DRAW_FRAMEBUFFER) == GL_FRAMEBUFFER_COMPLETE;
            (gl.bind_framebuffer)(GL_DRAW_FRAMEBUFFER, previous as u32);
            let canvas = Self { gl, texture, depth_stencil, framebuffer, width, height };
            if !complete {
                canvas.release();
                return Err(Error::Failed("the canvas framebuffer is incomplete"));
            }
            Ok(canvas)
        }
    }

    pub fn framebuffer(&self) -> u32 {
        self.framebuffer
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn bind(&self, viewport: [i32; 4]) -> Bound {
        let mut bound = Bound { draw: 0, viewport: [0; 4], scissor: false };
        // SAFETY: state queries into locals and a bind of our own framebuffer, context current.
        unsafe {
            glGetIntegerv(GL_DRAW_FRAMEBUFFER_BINDING, &mut bound.draw);
            glGetIntegerv(GL_VIEWPORT, bound.viewport.as_mut_ptr());
            bound.scissor = glIsEnabled(GL_SCISSOR_TEST) != 0;
            (self.gl.bind_framebuffer)(GL_DRAW_FRAMEBUFFER, self.framebuffer);
            glViewport(viewport[0], viewport[1], viewport[2], viewport[3]);
            glDisable(GL_SCISSOR_TEST);
        }
        bound
    }

    fn unbind(&self, bound: Bound) {
        // SAFETY: puts back what `bind` read, context current.
        unsafe {
            (self.gl.bind_framebuffer)(GL_DRAW_FRAMEBUFFER, bound.draw as u32);
            glViewport(bound.viewport[0], bound.viewport[1], bound.viewport[2], bound.viewport[3]);
            if bound.scissor {
                glEnable(GL_SCISSOR_TEST);
            }
        }
    }

    /// Clears the canvas to `rgba`, its depth to 1 and its stencil to 0, whatever write masks the
    /// game has set; every state touched is put back.
    pub fn clear(&self, rgba: [f32; 4]) {
        let bound = self.bind([0, 0, self.width as i32, self.height as i32]);
        // SAFETY: state queries into locals, a clear of our own framebuffer, the state put back.
        unsafe {
            let mut colour_mask = [0u8; 4];
            let mut depth_mask = 0u8;
            let mut stencil_mask = 0i32;
            let mut colour = [0.0f32; 4];
            let mut depth = 0.0f64;
            let mut stencil = 0i32;
            glGetBooleanv(GL_COLOR_WRITEMASK, colour_mask.as_mut_ptr());
            glGetBooleanv(GL_DEPTH_WRITEMASK, &mut depth_mask);
            glGetIntegerv(GL_STENCIL_WRITEMASK, &mut stencil_mask);
            glGetFloatv(GL_COLOR_CLEAR_VALUE, colour.as_mut_ptr());
            glGetDoublev(GL_DEPTH_CLEAR_VALUE, &mut depth);
            glGetIntegerv(GL_STENCIL_CLEAR_VALUE, &mut stencil);
            glColorMask(1, 1, 1, 1);
            glDepthMask(1);
            glStencilMask(u32::MAX);
            glClearColor(rgba[0], rgba[1], rgba[2], rgba[3]);
            glClearDepth(1.0);
            glClearStencil(0);
            glClear(GL_COLOR_BUFFER_BIT | GL_DEPTH_BUFFER_BIT | GL_STENCIL_BUFFER_BIT);
            glColorMask(colour_mask[0], colour_mask[1], colour_mask[2], colour_mask[3]);
            glDepthMask(depth_mask);
            glStencilMask(stencil_mask as u32);
            glClearColor(colour[0], colour[1], colour[2], colour[3]);
            glClearDepth(depth);
            glClearStencil(stencil);
        }
        self.unbind(bound);
    }

    /// Runs `draw` (the game's own draw calls) into the canvas with `viewport` (x, y, width,
    /// height, from the bottom; it may reach past the canvas, so a part of what the game draws
    /// fills it) and no scissor; the game's framebuffer, viewport and scissor are put back after.
    pub fn draw(&self, viewport: [i32; 4], draw: impl FnOnce()) {
        let bound = self.bind(viewport);
        draw();
        self.unbind(bound);
    }

    /// Frees the GL objects; call with the GL context current.
    pub fn release(self) {
        // SAFETY: deletes the objects this canvas created, context current.
        unsafe {
            (self.gl.delete_framebuffers)(1, &self.framebuffer);
            (self.gl.delete_renderbuffers)(1, &self.depth_stencil);
            glDeleteTextures(1, &self.texture);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use monaka_channel::d3d::{create_device, read_pixel};
    use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::Graphics::Gdi::GetDC;
    use windows::Win32::Graphics::OpenGL::*;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DefWindowProcW, RegisterClassW, WNDCLASSW, WS_OVERLAPPEDWINDOW};
    use windows::core::w;

    unsafe extern "system" fn window_proc(window: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        unsafe { DefWindowProcW(window, message, w, l) }
    }

    /// A hidden window with a current legacy GL context (the driver's newest compatible GL).
    fn gl_context() {
        unsafe {
            let instance = HINSTANCE(GetModuleHandleW(None).unwrap().0);
            let class = WNDCLASSW { lpfnWndProc: Some(window_proc), hInstance: instance, lpszClassName: w!("monaka_gl-test"), ..Default::default() };
            RegisterClassW(&class);
            let window =
                CreateWindowExW(Default::default(), w!("monaka_gl-test"), w!("monaka_gl-test"), WS_OVERLAPPEDWINDOW, 0, 0, 64, 64, None, None, Some(instance), None)
                    .unwrap();
            let dc = GetDC(Some(window));
            let format = PIXELFORMATDESCRIPTOR {
                nSize: size_of::<PIXELFORMATDESCRIPTOR>() as u16,
                nVersion: 1,
                dwFlags: PFD_DRAW_TO_WINDOW | PFD_SUPPORT_OPENGL | PFD_DOUBLEBUFFER,
                iPixelType: PFD_TYPE_RGBA,
                cColorBits: 32,
                cDepthBits: 24,
                iLayerType: PFD_MAIN_PLANE.0 as u8,
                ..Default::default()
            };
            let index = ChoosePixelFormat(dc, &format);
            SetPixelFormat(dc, index, &format).unwrap();
            let context = wglCreateContext(dc).unwrap();
            wglMakeCurrent(dc, context).unwrap();
        }
    }

    #[test]
    fn a_gl_framebuffer_arrives_in_d3d11_top_down() {
        gl_context();
        let gl = Functions::load().unwrap();
        let (w, h) = (64u32, 32u32);
        // The game's stand-in: a framebuffer whose top half is red and bottom half blue.
        let source = unsafe {
            let mut texture = 0u32;
            glGenTextures(1, &mut texture);
            glBindTexture(GL_TEXTURE_2D, texture);
            glTexImage2D(GL_TEXTURE_2D, 0, GL_RGBA8 as i32, w as i32, h as i32, 0, GL_RGBA, GL_UNSIGNED_BYTE, std::ptr::null());
            let mut framebuffer = 0u32;
            (gl.gen_framebuffers)(1, &mut framebuffer);
            (gl.bind_framebuffer)(GL_DRAW_FRAMEBUFFER, framebuffer);
            (gl.framebuffer_texture_2d)(GL_DRAW_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_2D, texture, 0);
            glEnable(GL_SCISSOR_TEST);
            glScissor(0, 0, w as i32, h as i32 / 2);
            glClearColor(0.0, 0.0, 1.0, 1.0);
            glClear(GL_COLOR_BUFFER_BIT);
            glScissor(0, h as i32 / 2, w as i32, h as i32 / 2);
            glClearColor(1.0, 0.0, 0.0, 1.0);
            glClear(GL_COLOR_BUFFER_BIT);
            framebuffer
        };
        let (device, context, _) = create_device(None).unwrap();
        let mut bridge = Bridge::new(&device).unwrap();
        let texture = bridge.capture(source, w, h).unwrap().clone();
        unsafe { glFinish() };
        assert_eq!(read_pixel(&device, &context, &texture, 5, 2).unwrap(), [255, 0, 0, 255], "top row");
        assert_eq!(read_pixel(&device, &context, &texture, 5, h - 3).unwrap(), [0, 0, 255, 255], "bottom row");
        // The game's state is put back.
        unsafe {
            let mut draw = 0i32;
            glGetIntegerv(GL_DRAW_FRAMEBUFFER_BINDING, &mut draw);
            assert_eq!(draw as u32, source);
            assert_ne!(glIsEnabled(GL_SCISSOR_TEST), 0);
        }
        bridge.release();
    }

    #[test]
    fn an_eye_arrives_from_its_part_of_the_frame_top_down() {
        gl_context();
        let gl = Functions::load().unwrap();
        let (w, h) = (64u32, 32u32);
        // Quarters, as seen: red top-left, green top-right, blue bottom-left, white bottom-right.
        let source = unsafe {
            let mut texture = 0u32;
            glGenTextures(1, &mut texture);
            glBindTexture(GL_TEXTURE_2D, texture);
            glTexImage2D(GL_TEXTURE_2D, 0, GL_RGBA8 as i32, w as i32, h as i32, 0, GL_RGBA, GL_UNSIGNED_BYTE, std::ptr::null());
            let mut framebuffer = 0u32;
            (gl.gen_framebuffers)(1, &mut framebuffer);
            (gl.bind_framebuffer)(GL_DRAW_FRAMEBUFFER, framebuffer);
            (gl.framebuffer_texture_2d)(GL_DRAW_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_2D, texture, 0);
            glEnable(GL_SCISSOR_TEST);
            let (hw, hh) = (w as i32 / 2, h as i32 / 2);
            for (x, y, rgb) in [(0, hh, [1.0, 0.0, 0.0]), (hw, hh, [0.0, 1.0, 0.0]), (0, 0, [0.0, 0.0, 1.0]), (hw, 0, [1.0, 1.0, 1.0])] {
                glScissor(x, y, hw, hh);
                glClearColor(rgb[0], rgb[1], rgb[2], 1.0);
                glClear(GL_COLOR_BUFFER_BIT);
            }
            framebuffer
        };
        let (device, context, _) = create_device(None).unwrap();
        let mut bridge = Bridge::new(&device).unwrap();
        // The middle of the frame: its top-left corner in the red quarter, bottom-right in white.
        let (eye, depth) = bridge.capture_eye(1, source, (w, h), [16, 8, 32, 16], None).unwrap();
        let (other, _) = bridge.capture_eye(0, source, (w, h), [16, 8, 32, 16], None).unwrap();
        unsafe { glFinish() };
        assert!(depth.is_none());
        assert_ne!(eye.as_raw(), other.as_raw(), "each slot its own texture");
        assert_eq!(d3d::texture_desc(&eye).Width, 32);
        assert_eq!(read_pixel(&device, &context, &eye, 2, 2).unwrap(), [255, 0, 0, 255], "top-left");
        assert_eq!(read_pixel(&device, &context, &eye, 29, 2).unwrap(), [0, 255, 0, 255], "top-right");
        assert_eq!(read_pixel(&device, &context, &eye, 2, 13).unwrap(), [0, 0, 255, 255], "bottom-left");
        assert_eq!(read_pixel(&device, &context, &eye, 29, 13).unwrap(), [255, 255, 255, 255], "bottom-right");
        unsafe {
            let mut draw = 0i32;
            glGetIntegerv(GL_DRAW_FRAMEBUFFER_BINDING, &mut draw);
            assert_eq!(draw as u32, source);
            assert_ne!(glIsEnabled(GL_SCISSOR_TEST), 0);
        }
        assert!(bridge.capture_eye(0, source, (w, h), [40, 8, 32, 16], None).is_err(), "past the frame's edge");
        bridge.release();
    }

    #[test]
    fn a_depth_texture_arrives_in_d3d11_top_down_as_floats() {
        const GL_DEPTH_COMPONENT32F: i32 = 0x8CAC;
        const GL_DEPTH_COMPONENT: u32 = 0x1902;
        gl_context();
        let (w, h) = (16u32, 8u32);
        // The game's depth: the left half's top rows 0.25, its bottom rows 0.5, the right half 0.75
        // (GL rows from the bottom).
        let values: Vec<f32> = (0..h).flat_map(|row| (0..w).map(move |x| if x >= w / 2 { 0.75 } else if row >= h / 2 { 0.25 } else { 0.5 })).collect();
        let source = unsafe {
            let mut texture = 0u32;
            glGenTextures(1, &mut texture);
            glBindTexture(GL_TEXTURE_2D, texture);
            glTexImage2D(GL_TEXTURE_2D, 0, GL_DEPTH_COMPONENT32F, w as i32, h as i32, 0, GL_DEPTH_COMPONENT, GL_FLOAT, values.as_ptr().cast());
            glBindTexture(GL_TEXTURE_2D, 0);
            glViewport(1, 2, 3, 4);
            glEnable(GL_DEPTH_TEST);
            texture
        };
        let (device, context, _) = create_device(None).unwrap();
        let mut bridge = Bridge::new(&device).unwrap();
        // The left half, at its own size.
        let texture = bridge.capture_depth(source, (w, h), false, [0.0, 0.0, 0.5, 1.0], w / 2, h).unwrap().clone();
        unsafe { glFinish() };
        let (width, height, texels) = monaka_channel::d3d::read_image(&device, &context, &texture).unwrap();
        assert_eq!((width, height), (w / 2, h));
        let at = |x: u32, y: u32| f32::from_le_bytes(texels[((y * width + x) * 4) as usize..][..4].try_into().unwrap());
        assert_eq!(at(1, 0), 0.25, "top row");
        assert_eq!(at(1, h - 1), 0.5, "bottom row");
        assert_eq!(at(width - 1, 0), 0.25, "the right half left out");
        // The game's state is put back.
        unsafe {
            let mut viewport = [0i32; 4];
            glGetIntegerv(GL_VIEWPORT, viewport.as_mut_ptr());
            assert_eq!(viewport, [1, 2, 3, 4]);
            assert_ne!(glIsEnabled(GL_DEPTH_TEST), 0);
        }
        bridge.release();
    }

    #[test]
    fn the_texel_rect_counts_from_the_bottom() {
        assert_eq!(texel_rect(100, 50, [0.0, 0.0, 0.5, 1.0]), [0, 0, 50, 50]);
        // The top fifth: from the bottom, rows 40 to 49.
        assert_eq!(texel_rect(100, 50, [0.5, 0.0, 0.5, 0.2]), [50, 40, 50, 10]);
    }

    #[test]
    fn a_canvas_takes_part_of_a_draw_and_puts_the_state_back() {
        gl_context();
        let (device, context, _) = create_device(None).unwrap();
        let canvas = Canvas::new(32, 32).unwrap();
        unsafe {
            glViewport(1, 2, 3, 4);
            glEnable(GL_SCISSOR_TEST);
        }
        canvas.clear([0.0, 0.0, 1.0, 1.0]);
        // A full-viewport red triangle, the viewport twice the canvas and starting left of it:
        // the canvas shows the viewport's middle, all red.
        canvas.draw([-16, -16, 64, 64], || unsafe {
            glBegin(GL_TRIANGLES);
            glColor4f(1.0, 0.0, 0.0, 1.0);
            glVertex2f(-1.0, -1.0);
            glVertex2f(3.0, -1.0);
            glVertex2f(-1.0, 3.0);
            glEnd();
        });
        unsafe {
            let mut viewport = [0i32; 4];
            glGetIntegerv(GL_VIEWPORT, viewport.as_mut_ptr());
            assert_eq!(viewport, [1, 2, 3, 4]);
            assert_ne!(glIsEnabled(GL_SCISSOR_TEST), 0);
            glDisable(GL_SCISSOR_TEST);
        }
        let mut bridge = Bridge::new(&device).unwrap();
        let texture = bridge.capture(canvas.framebuffer(), 32, 32).unwrap().clone();
        unsafe { glFinish() };
        assert_eq!(read_pixel(&device, &context, &texture, 16, 16).unwrap(), [255, 0, 0, 255]);
        bridge.release();
        canvas.release();
    }
}
