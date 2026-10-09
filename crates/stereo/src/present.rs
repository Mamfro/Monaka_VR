//! DXGI's `Present` for a game whose swapchain the producer cannot reach: the method is found on a
//! throwaway swapchain made on a hidden window, and hooked in its code (every swapchain of the
//! process goes through it, so the detour must pick the game's own), or in the swapchain class's
//! vtable, which the game's swapchain shares, when that code does not decode.

use monaka_hook::{Hooks, Original, slot};
use monaka_producer::{Rejection, log};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{D3D11_CREATE_DEVICE_FLAG, D3D11_SDK_VERSION, D3D11CreateDeviceAndSwapChain, ID3D11Device};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_MODE_DESC, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{DXGI_SWAP_CHAIN_DESC, DXGI_SWAP_EFFECT_FLIP_DISCARD, DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGISwapChain};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, UnregisterClassW, WINDOW_EX_STYLE, WNDCLASSW, WS_OVERLAPPED,
};
use windows::core::{HRESULT, Interface, w};

/// `IDXGISwapChain::Present(this, sync_interval, flags)`.
pub type PresentFn = unsafe extern "system" fn(*mut core::ffi::c_void, u32, u32) -> HRESULT;

unsafe extern "system" fn probe_window_proc(window: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    // SAFETY: default handling for our own probe window.
    unsafe { DefWindowProcW(window, message, w, l) }
}

/// Prepares a hook of DXGI's `Present` with `detour`, pointing `original` at the real one.
///
/// # Safety
/// Called at start, before the hooks are enabled. `detour` must forward to `original`.
pub unsafe fn hook_dxgi_present(hooks: &mut Hooks, original: &Original<PresentFn>, detour: PresentFn) -> Result<(), Rejection> {
    // SAFETY: window and device creation for a probe that is destroyed before returning; the hook
    // targets DXGI's Present, whose type is `PresentFn`.
    unsafe {
        let instance: HINSTANCE = GetModuleHandleW(None).map_err(|e| Rejection::unsupported(format!("module handle: {e}")))?.into();
        let class = WNDCLASSW { lpfnWndProc: Some(probe_window_proc), hInstance: instance, lpszClassName: w!("monaka_present_probe"), ..Default::default() };
        RegisterClassW(&class);
        let window = CreateWindowExW(WINDOW_EX_STYLE(0), class.lpszClassName, w!(""), WS_OVERLAPPED, 0, 0, 16, 16, None, None, Some(instance), None)
            .map_err(|e| Rejection::unsupported(format!("probe window: {e}")))?;
        let desc = DXGI_SWAP_CHAIN_DESC {
            BufferDesc: DXGI_MODE_DESC { Width: 16, Height: 16, Format: DXGI_FORMAT_R8G8B8A8_UNORM, ..Default::default() },
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            OutputWindow: window,
            Windowed: true.into(),
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            ..Default::default()
        };
        let mut swap: Option<IDXGISwapChain> = None;
        let mut device: Option<ID3D11Device> = None;
        let created = D3D11CreateDeviceAndSwapChain(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            Default::default(),
            D3D11_CREATE_DEVICE_FLAG(0),
            None,
            D3D11_SDK_VERSION,
            Some(&desc),
            Some(&mut swap),
            Some(&mut device),
            None,
            None,
        );
        let result = match (created, swap) {
            (Ok(()), Some(swap)) => hooks
                .method_noted(original, "DXGI Present", swap.as_raw(), slot::SWAPCHAIN_PRESENT, detour, |why| log!("{why}; hooking the swapchain vtable instead"))
                .map_err(Rejection::from),
            (created, _) => Err(Rejection::unsupported(format!("could not create the probe swapchain: {:?}", created.err()))),
        };
        drop(device);
        let _ = DestroyWindow(window);
        let _ = UnregisterClassW(class.lpszClassName, Some(instance));
        result
    }
}
