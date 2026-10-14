//! The 2D window: a desktop window that shows a picture and egui over it, used with the mouse.
//! A click never takes focus (`WS_EX_NOACTIVATE`, and `MA_NOACTIVATE` on clicks), so a game that
//! pauses when it loses focus keeps running while the window is clicked; it stays above other
//! windows so a windowed game does not cover it. Typing is the exception: while a text box in it
//! has focus the window takes the keyboard (the game may pause meanwhile), and it gives it back to
//! the window that had it once the box lets go ([`Window::ui`]). Copy and paste go through the
//! clipboard. One per process.

use crate::Result;
use crate::gpu::failed;
use crate::menu::painter::Painter;
use egui::{Pos2, Vec2};
use monaka_core::sync::Locked;
use std::sync::Mutex;
use std::time::Instant;
use windows::Win32::Foundation::{HANDLE, HGLOBAL, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetProcessDpiAwarenessContext};
use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{Interface, PCWSTR, w};

/// What the window's messages said, in order, for the next frame.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Input {
    Moved(f32, f32),
    Button { secondary: bool, pressed: bool, x: f32, y: f32 },
    /// Wheel notches, up positive.
    Wheel(f32),
    Left,
    Resized(u32, u32),
    Dpi(u32),
    Closed,
    /// A character typed.
    Char(char),
    /// A key pressed or let go (virtual key), with Ctrl and Shift as they were.
    Key { key: u16, pressed: bool, repeat: bool, ctrl: bool, shift: bool },
    /// The window lost the keyboard (another window took it).
    Unfocused,
}

/// The first half of a character typed in two (UTF-16 surrogates).
static HIGH_SURROGATE: Mutex<Option<u16>> = Mutex::new(None);
/// The clipboard's Unicode text format.
const CF_UNICODETEXT: u32 = 13;

static INPUT: Mutex<Vec<Input>> = Mutex::new(Vec::new());
/// Whether the window asked to hear when the pointer leaves (once per entry).
static TRACKING: Mutex<bool> = Mutex::new(false);

const CLASS: PCWSTR = w!("MonakaWindow");
/// `WM_MOUSELEAVE` (not in the bindings' message list).
const WM_MOUSELEAVE: u32 = 0x02A3;
/// The window's back buffers' format.
const FORMAT: DXGI_FORMAT = DXGI_FORMAT_B8G8R8A8_UNORM;

pub struct Window {
    hwnd: HWND,
    swapchain: IDXGISwapChain1,
    target: Option<ID3D11RenderTargetView>,
    size: [u32; 2],
    dpi: u32,
    resized: bool,
    closed: bool,
    egui: egui::Context,
    painter: Option<Painter>,
    events: Vec<egui::Event>,
    pointer: Option<Pos2>,
    started: Instant,
    /// The window that had the keyboard before a text box here took it.
    gave_up: Option<HWND>,
}

impl Window {
    /// Opens the window, titled `title`, its inside `size` pixels at 96 dpi (scaled to the monitor's),
    /// drawn by `device`.
    pub fn open(device: &ID3D11Device, title: &str, size: [u32; 2]) -> Result<Self> {
        // SAFETY: plain Win32 calls with valid arguments; the class's procedure lives as long as
        // the process.
        let hwnd = unsafe {
            // Real pixels on a scaled monitor (a second call in the process fails harmlessly).
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            let instance = GetModuleHandleW(None).map_err(failed("module handle"))?;
            let class = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(procedure),
                hInstance: instance.into(),
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                lpszClassName: CLASS,
                ..Default::default()
            };
            RegisterClassExW(&class);
            let style = WS_OVERLAPPEDWINDOW;
            let extended = WS_EX_NOACTIVATE | WS_EX_TOPMOST | WS_EX_APPWINDOW;
            let mut frame = RECT { left: 0, top: 0, right: size[0] as i32, bottom: size[1] as i32 };
            let _ = AdjustWindowRectEx(&mut frame, style, false, extended);
            let title: Vec<u16> = title.encode_utf16().chain([0]).collect();
            let hwnd = CreateWindowExW(extended, CLASS, PCWSTR(title.as_ptr()), style, CW_USEDEFAULT, CW_USEDEFAULT, frame.right - frame.left, frame.bottom - frame.top, None, None, Some(instance.into()), None)
                .map_err(failed("window"))?;
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            hwnd
        };
        // SAFETY: a live window.
        let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
        let size = client_size(hwnd);
        let swapchain = swapchain(device, hwnd, size)?;
        let egui = egui::Context::default();
        egui.all_styles_mut(|style| style.animation_time = 0.0);
        Ok(Self { hwnd, swapchain, target: None, size, dpi, resized: false, closed: false, egui, painter: None, events: Vec::new(), pointer: None, started: Instant::now(), gave_up: None })
    }

    /// Reads the window's messages; false once it has been closed.
    pub fn pump(&mut self) -> bool {
        let mut message = MSG::default();
        // SAFETY: the thread's own message queue, read into a local.
        unsafe {
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        let ppp = self.pixels_per_point();
        let modifiers = egui::Modifiers::default();
        for input in std::mem::take(&mut *INPUT.locked()) {
            let at = |x: f32, y: f32| Pos2::new(x / ppp, y / ppp);
            match input {
                Input::Moved(x, y) => {
                    self.pointer = Some(at(x, y));
                    self.events.push(egui::Event::PointerMoved(at(x, y)));
                }
                Input::Button { secondary, pressed, x, y } => {
                    let button = if secondary { egui::PointerButton::Secondary } else { egui::PointerButton::Primary };
                    self.events.push(egui::Event::PointerButton { pos: at(x, y), button, pressed, modifiers });
                }
                Input::Wheel(notches) => {
                    self.events.push(egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Line, delta: Vec2::new(0.0, notches), phase: egui::TouchPhase::Move, modifiers });
                }
                Input::Left => {
                    self.pointer = None;
                    self.events.push(egui::Event::PointerGone);
                }
                Input::Resized(width, height) => {
                    if [width, height] != self.size && width > 0 && height > 0 {
                        self.size = [width, height];
                        self.resized = true;
                    }
                }
                Input::Dpi(dpi) => self.dpi = dpi.max(96),
                Input::Closed => self.closed = true,
                Input::Char(c) => self.events.push(egui::Event::Text(c.to_string())),
                Input::Key { key, pressed, repeat, ctrl, shift } => {
                    let modifiers = egui::Modifiers { ctrl, shift, command: ctrl, ..Default::default() };
                    // Copy, cut and paste are their own events; the clipboard is ours to read.
                    if pressed && ctrl && key == u16::from(b'C') {
                        self.events.push(egui::Event::Copy);
                    } else if pressed && ctrl && key == u16::from(b'X') {
                        self.events.push(egui::Event::Cut);
                    } else if pressed && ctrl && key == u16::from(b'V') {
                        if let Some(text) = clipboard_text(self.hwnd) {
                            self.events.push(egui::Event::Paste(text));
                        }
                    } else if let Some(key) = egui_key(key) {
                        self.events.push(egui::Event::Key { key, physical_key: None, pressed, repeat, modifiers });
                    }
                }
                Input::Unfocused => {
                    self.gave_up = None;
                    self.events.push(egui::Event::WindowFocused(false));
                }
            }
        }
        !self.closed
    }

    /// The back buffer to draw this frame into, and its size (pixels).
    pub fn target(&mut self, device: &ID3D11Device) -> Result<(ID3D11RenderTargetView, [u32; 2])> {
        if self.resized {
            self.target = None;
            // SAFETY: no view of the buffers is held any more.
            unsafe { self.swapchain.ResizeBuffers(0, self.size[0], self.size[1], DXGI_FORMAT_UNKNOWN, DXGI_SWAP_CHAIN_FLAG(0)) }.map_err(failed("window resize"))?;
            self.resized = false;
        }
        if self.target.is_none() {
            // SAFETY: the swapchain's own back buffer and a view of it in its format.
            let view = unsafe {
                let buffer: ID3D11Texture2D = self.swapchain.GetBuffer(0).map_err(failed("window back buffer"))?;
                let mut view = None;
                device.CreateRenderTargetView(&buffer, None, Some(&mut view)).map_err(failed("window target"))?;
                view
            };
            self.target = Some(view.ok_or("no window target")?);
        }
        let target = self.target.clone().ok_or("no window target")?;
        Ok((target, self.size))
    }

    /// Pixels to egui's point (the monitor's scale).
    pub fn pixels_per_point(&self) -> f32 {
        self.dpi as f32 / 96.0
    }

    /// The window's inside in egui's points.
    pub fn points(&self) -> Vec2 {
        Vec2::new(self.size[0] as f32, self.size[1] as f32) / self.pixels_per_point()
    }

    /// Runs egui with the mouse since the last frame, `build` adding to it, and draws it over
    /// `target` (the back buffer, [`Window::target`]).
    pub fn ui(&mut self, device: &ID3D11Device, context: &ID3D11DeviceContext, target: &ID3D11RenderTargetView, mut build: impl FnMut(&egui::Context)) -> Result<()> {
        if self.painter.is_none() {
            self.painter = Some(Painter::new(device)?);
        }
        let ppp = self.pixels_per_point();
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, self.points())),
            max_texture_side: Some(4096),
            time: Some(self.started.elapsed().as_secs_f64()),
            events: std::mem::take(&mut self.events),
            ..Default::default()
        };
        input.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(ppp);
        let mut output = self.egui.run_ui(input, |ui| build(&ui.ctx().clone()));
        for command in std::mem::take(&mut output.platform_output.commands) {
            if let egui::OutputCommand::CopyText(text) = command {
                to_clipboard(self.hwnd, &text);
            }
        }
        self.keyboard(self.egui.egui_wants_keyboard_input());
        let primitives = self.egui.tessellate(std::mem::take(&mut output.shapes), output.pixels_per_point);
        let Some(painter) = self.painter.as_mut() else { return Ok(()) };
        painter.paint(device, context, target, self.size, output.pixels_per_point, false, &primitives, &mut output.textures_delta)
    }

    /// Takes the keyboard while a text box wants it (`wanted`), from the window that had it, and
    /// gives it back once the box lets go (if the keyboard is still here).
    fn keyboard(&mut self, wanted: bool) {
        // SAFETY: plain focus calls on live windows; a stale one fails harmlessly.
        unsafe {
            let foreground = GetForegroundWindow();
            if wanted && foreground != self.hwnd {
                self.gave_up = Some(foreground);
                let _ = SetForegroundWindow(self.hwnd);
                let _ = SetFocus(Some(self.hwnd));
            } else if !wanted
                && let Some(previous) = self.gave_up.take()
                && foreground == self.hwnd
            {
                let _ = SetForegroundWindow(previous);
            }
        }
    }

    /// Shows the frame drawn, at the monitor's refresh.
    pub fn present(&mut self) -> Result<()> {
        // SAFETY: presents our own swapchain.
        let result = unsafe { self.swapchain.Present(1, DXGI_PRESENT(0)) };
        // An occluded window (minimized: `DXGI_STATUS_OCCLUDED`) is no error.
        if result == windows::core::HRESULT(0x087A_0001) { Ok(()) } else { result.ok().map_err(failed("window present")) }
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        // SAFETY: our own window, destroyed on the thread that made it.
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

/// Fills `target` with `colour` (straight RGBA).
pub fn clear(context: &ID3D11DeviceContext, target: &ID3D11RenderTargetView, colour: [f32; 4]) {
    // SAFETY: clears a live view.
    unsafe { context.ClearRenderTargetView(target, &colour) };
}

fn client_size(hwnd: HWND) -> [u32; 2] {
    let mut rect = RECT::default();
    // SAFETY: a live window and a local rectangle.
    let _ = unsafe { GetClientRect(hwnd, &mut rect) };
    [(rect.right - rect.left).max(1) as u32, (rect.bottom - rect.top).max(1) as u32]
}

/// A flip-model swapchain for `hwnd` on `device`'s adapter.
fn swapchain(device: &ID3D11Device, hwnd: HWND, size: [u32; 2]) -> Result<IDXGISwapChain1> {
    let desc = DXGI_SWAP_CHAIN_DESC1 {
        Width: size[0],
        Height: size[1],
        Format: FORMAT,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
        BufferCount: 2,
        Scaling: DXGI_SCALING_NONE,
        SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
        Stereo: false.into(),
        AlphaMode: DXGI_ALPHA_MODE_IGNORE,
        Flags: 0,
    };
    // SAFETY: the device's own adapter and factory, and a live window.
    unsafe {
        let adapter = device.cast::<IDXGIDevice>().and_then(|d| d.GetAdapter()).map_err(failed("window adapter"))?;
        let factory: IDXGIFactory2 = adapter.GetParent().map_err(failed("window factory"))?;
        let swapchain = factory.CreateSwapChainForHwnd(device, hwnd, &desc, None, None).map_err(failed("window swapchain"))?;
        let _ = factory.MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER);
        Ok(swapchain)
    }
}

/// The egui key of virtual key `key`, for the keys text boxes use.
fn egui_key(key: u16) -> Option<egui::Key> {
    use egui::Key;
    Some(match VIRTUAL_KEY(key) {
        VK_RETURN => Key::Enter,
        VK_BACK => Key::Backspace,
        VK_DELETE => Key::Delete,
        VK_ESCAPE => Key::Escape,
        VK_TAB => Key::Tab,
        VK_LEFT => Key::ArrowLeft,
        VK_RIGHT => Key::ArrowRight,
        VK_UP => Key::ArrowUp,
        VK_DOWN => Key::ArrowDown,
        VK_HOME => Key::Home,
        VK_END => Key::End,
        VK_PRIOR => Key::PageUp,
        VK_NEXT => Key::PageDown,
        _ if (u16::from(b'A')..=u16::from(b'Z')).contains(&key) => Key::from_name(&char::from(key as u8).to_string())?,
        _ => return None,
    })
}

/// The clipboard's text.
fn clipboard_text(hwnd: HWND) -> Option<String> {
    // SAFETY: the clipboard opened, its text read through a lock, unlocked and closed.
    unsafe {
        OpenClipboard(Some(hwnd)).ok()?;
        let text = GetClipboardData(CF_UNICODETEXT).ok().and_then(|data| {
            let global = HGLOBAL(data.0);
            let start = GlobalLock(global) as *const u16;
            if start.is_null() {
                return None;
            }
            let mut length = 0;
            while *start.add(length) != 0 && length < 1 << 20 {
                length += 1;
            }
            let text = String::from_utf16_lossy(std::slice::from_raw_parts(start, length));
            let _ = GlobalUnlock(global);
            Some(text)
        });
        let _ = CloseClipboard();
        text
    }
}

/// `text` onto the clipboard.
fn to_clipboard(hwnd: HWND, text: &str) {
    let wide: Vec<u16> = text.encode_utf16().chain([0]).collect();
    // SAFETY: the clipboard opened and emptied; a moveable block of our own filled and handed to
    // it (the clipboard owns it from then on); closed.
    unsafe {
        if OpenClipboard(Some(hwnd)).is_err() {
            return;
        }
        let _ = EmptyClipboard();
        if let Ok(global) = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2) {
            let start = GlobalLock(global) as *mut u16;
            if !start.is_null() {
                std::ptr::copy_nonoverlapping(wide.as_ptr(), start, wide.len());
                let _ = GlobalUnlock(global);
                let _ = SetClipboardData(CF_UNICODETEXT, Some(HANDLE(global.0)));
            }
        }
        let _ = CloseClipboard();
    }
}

/// The low and high 16 bits of a message parameter, signed (client coordinates may be negative
/// while the mouse is captured).
fn words(value: isize) -> (f32, f32) {
    (f32::from((value & 0xffff) as u16 as i16), f32::from(((value >> 16) & 0xffff) as u16 as i16))
}

unsafe extern "system" fn procedure(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let (x, y) = words(lparam.0);
    let input = match message {
        // A click never takes focus from the game.
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        WM_MOUSEMOVE => {
            let mut tracking = TRACKING.locked();
            if !*tracking {
                let mut track = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                // SAFETY: a live window and a local request.
                *tracking = unsafe { TrackMouseEvent(&mut track) }.is_ok();
            }
            Some(Input::Moved(x, y))
        }
        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONUP => {
            let pressed = matches!(message, WM_LBUTTONDOWN | WM_RBUTTONDOWN);
            // A drag that leaves the window still ends in it.
            // SAFETY: our own window, on its thread.
            unsafe {
                if pressed {
                    SetCapture(hwnd);
                } else {
                    let _ = ReleaseCapture();
                }
            }
            Some(Input::Button { secondary: matches!(message, WM_RBUTTONDOWN | WM_RBUTTONUP), pressed, x, y })
        }
        WM_MOUSEWHEEL => Some(Input::Wheel(f32::from(((wparam.0 >> 16) & 0xffff) as u16 as i16) / 120.0)),
        WM_MOUSELEAVE => {
            *TRACKING.locked() = false;
            Some(Input::Left)
        }
        WM_CHAR => {
            let unit = wparam.0 as u16;
            let mut high = HIGH_SURROGATE.locked();
            let typed = match unit {
                0xD800..=0xDBFF => {
                    *high = Some(unit);
                    None
                }
                0xDC00..=0xDFFF => high.take().and_then(|h| char::decode_utf16([h, unit]).next()?.ok()),
                _ => char::from_u32(u32::from(unit)),
            };
            // Control characters (Enter, Backspace, Ctrl+letters) come as keys instead.
            typed.filter(|c| !c.is_control()).map(Input::Char)
        }
        WM_KEYDOWN | WM_KEYUP | WM_SYSKEYDOWN | WM_SYSKEYUP => {
            let pressed = matches!(message, WM_KEYDOWN | WM_SYSKEYDOWN);
            // SAFETY: the keyboard's state as of this message, on our thread.
            let held = |key: VIRTUAL_KEY| unsafe { GetKeyState(i32::from(key.0)) } < 0;
            Some(Input::Key { key: wparam.0 as u16, pressed, repeat: pressed && lparam.0 & (1 << 30) != 0, ctrl: held(VK_CONTROL), shift: held(VK_SHIFT) })
        }
        WM_KILLFOCUS => Some(Input::Unfocused),
        WM_SIZE => Some(Input::Resized((lparam.0 & 0xffff) as u32, ((lparam.0 >> 16) & 0xffff) as u32)),
        WM_DPICHANGED => {
            // The window takes the size Windows suggests for the new monitor.
            // SAFETY: Windows passes the suggested rectangle with this message.
            unsafe {
                let suggested = &*(lparam.0 as *const RECT);
                let _ = SetWindowPos(hwnd, None, suggested.left, suggested.top, suggested.right - suggested.left, suggested.bottom - suggested.top, SWP_NOZORDER | SWP_NOACTIVATE);
            }
            Some(Input::Dpi(((wparam.0 >> 16) & 0xffff) as u32))
        }
        // Closing is the owner's to do: it stops its run, then drops the window.
        WM_CLOSE => {
            INPUT.locked().push(Input::Closed);
            return LRESULT(0);
        }
        _ => None,
    };
    if let Some(input) = input {
        INPUT.locked().push(input);
    }
    // SAFETY: the message as Windows passed it.
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

#[cfg(test)]
mod tests;
