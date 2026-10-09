//! OpenGL games into the D3D11 channels. A GL framebuffer (the default back buffer or any
//! framebuffer object) is copied, flipped to top-down, into a D3D11 texture on our own device
//! through `WGL_NV_DX_interop2`; from there the keyed channel, the alternate-eye publisher and the
//! depth warp work as for a Direct3D game.
//!
//! Everything here runs on the game's render thread with its GL context current (inside its
//! `SwapBuffers`, see `monaka_hook::Hooks::import`). GL state the copy touches (framebuffer
//! bindings, read buffer, scissor test, sRGB writes) is saved and put back.

use monaka_channel::d3d;
use std::ffi::c_void;
use std::fmt;
use windows::Win32::Graphics::Direct3D11::{D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, ID3D11Device, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM;
use windows::Win32::Graphics::OpenGL::{
    GL_BACK, GL_COLOR_BUFFER_BIT, GL_NEAREST, GL_SCISSOR_TEST, GL_TEXTURE_2D, glDeleteTextures, glDisable, glEnable, glGenTextures,
    glGetIntegerv, glIsEnabled, glReadBuffer, wglGetCurrentContext, wglGetProcAddress,
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
    open_device: DxOpenDevice,
    close_device: DxCloseDevice,
    register_object: DxRegisterObject,
    unregister_object: DxUnregisterObject,
    lock_objects: DxLockObjects,
    unlock_objects: DxLockObjects,
}

fn load<F: Copy>(name: &'static str) -> Result<F> {
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
        Ok(Self { gl, device: device.clone(), interop, target: None })
    }

    pub fn device(&self) -> &ID3D11Device {
        &self.device
    }

    fn release_target(&mut self) {
        if let Some(target) = self.target.take() {
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
        let mut desc = d3d::simple_texture_desc(width, height, DXGI_FORMAT_R8G8B8A8_UNORM);
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
            self.target = Some(Target { texture, gl_texture, framebuffer, object, width, height });
            if !complete {
                self.release_target();
                return Err(Error::Failed("the shared framebuffer is incomplete"));
            }
        }
        Ok(self.target.as_ref().expect("just made"))
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
        if !monaka_core::protocol::dimensions_ok(width, height) {
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
            if (gl.unlock_objects)(interop, 1, &mut object) == 0 {
                return Err(Error::Failed("wglDXUnlockObjectsNV"));
            }
        }
        Ok(&self.target.as_ref().expect("made above").texture)
    }

    /// Frees the GL objects and closes interop; call with the GL context current. Dropping a
    /// bridge without it leaves them to the context's own teardown.
    pub fn release(mut self) {
        self.release_target();
        // SAFETY: closes the interop handle this bridge opened.
        unsafe { (self.gl.close_device)(self.interop) };
        self.interop = std::ptr::null_mut();
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
}
