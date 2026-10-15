//! The D3D12 queue a swapchain presents on, read from the swapchain object itself. A flip-model
//! D3D12 swapchain is created on a command queue and DXGI keeps a pointer to it inside its own
//! object. Where is not documented, so it is learned once on a swapchain made here, on a queue made
//! here, then read from the game's swapchain at the same place and checked before use: it must be a
//! live direct queue of the swapchain's own device. Nothing of the game's is touched to learn it.

use monaka_hook::mem;
use monaka_producer::log;
use std::ffi::c_void;
use std::sync::OnceLock;
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
use windows::Win32::Graphics::Direct3D12::{
    D3D12_COMMAND_LIST_TYPE_DIRECT, D3D12_COMMAND_QUEUE_DESC, D3D12CreateDevice, ID3D12CommandQueue, ID3D12Device,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory2, DXGI_CREATE_FACTORY_FLAGS, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
    DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIFactory2, IDXGISwapChain,
};
use windows::core::{IUnknown, Interface};

/// How far into a swapchain object the queue pointer is looked for.
const SEARCH: usize = 0x1000;

/// Byte offsets inside a DXGI D3D12 swapchain object that hold its queue (learned once).
static OFFSETS: OnceLock<Result<Vec<usize>, String>> = OnceLock::new();

/// Learns where DXGI keeps a swapchain's queue: makes a small composition swapchain (no window) on
/// a queue of the default adapter's device and finds that queue's pointer in it. Run at start.
pub fn learn() -> Result<(), String> {
    OFFSETS.get_or_init(|| {
        let offsets = learn_now();
        match &offsets {
            Ok(found) => log!("D3D12 swapchain keeps its queue at {found:#x?}"),
            Err(why) => log!("D3D12 swapchain queue offset not learned: {why}"),
        }
        offsets
    });
    OFFSETS.get().unwrap().as_ref().map(|_| ()).map_err(Clone::clone)
}

/// A direct queue on the default adapter's device (the game's own device when it uses that
/// adapter: D3D12 hands back the existing device).
fn direct_queue() -> Result<ID3D12CommandQueue, String> {
    let mut device: Option<ID3D12Device> = None;
    // SAFETY: plain D3D12 creation calls on objects owned here.
    unsafe {
        D3D12CreateDevice(None, D3D_FEATURE_LEVEL_11_0, &mut device).map_err(|e| format!("device: {e}"))?;
        let desc = D3D12_COMMAND_QUEUE_DESC { Type: D3D12_COMMAND_LIST_TYPE_DIRECT, ..Default::default() };
        device.ok_or("no device")?.CreateCommandQueue(&desc).map_err(|e| format!("queue: {e}"))
    }
}

/// A small window-less (composition) flip-model swapchain on `queue`.
fn composition_chain(queue: &ID3D12CommandQueue) -> Result<IDXGISwapChain, String> {
    let desc = DXGI_SWAP_CHAIN_DESC1 {
        Width: 64,
        Height: 64,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
        BufferCount: 2,
        Scaling: DXGI_SCALING_STRETCH,
        SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
        AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
        ..Default::default()
    };
    // SAFETY: plain DXGI creation calls on objects owned here.
    unsafe {
        let factory: IDXGIFactory2 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)).map_err(|e| format!("factory: {e}"))?;
        let chain = factory.CreateSwapChainForComposition(queue, &desc, None).map_err(|e| format!("swapchain: {e}"))?;
        chain.cast::<IDXGISwapChain>().map_err(|e| e.to_string())
    }
}

fn learn_now() -> Result<Vec<usize>, String> {
    let queue = direct_queue()?;
    let chain = composition_chain(&queue)?;
    let wanted = [queue.as_raw() as usize, queue.cast::<IUnknown>().map_err(|e| e.to_string())?.as_raw() as usize];
    let mut bytes = vec![0u8; SEARCH];
    if !mem::read_bytes(chain.as_raw() as usize, &mut bytes) {
        return Err("swapchain object unreadable".into());
    }
    let found: Vec<usize> = (0..SEARCH / 8)
        .map(|i| i * 8)
        .filter(|&at| wanted.contains(&(u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap()) as usize)))
        .collect();
    if found.is_empty() { Err("the queue is not inside the swapchain object".into()) } else { Ok(found) }
}

/// The direct queue the live swapchain `swap` presents on, checked to be a direct queue of the
/// swapchain's own device; `None` if it cannot be shown to be.
///
/// # Safety
/// `swap` is a live DXGI swapchain, borrowed for this call (as in its Present).
pub unsafe fn of(swap: *mut c_void) -> Option<ID3D12CommandQueue> {
    let offsets = OFFSETS.get()?.as_ref().ok()?;
    // SAFETY: the caller's live swapchain, borrowed.
    let chain = unsafe { IDXGISwapChain::from_raw_borrowed(&swap) }?;
    // SAFETY: plain query of the swapchain's device.
    let device: ID3D12Device = unsafe { chain.GetDevice() }.ok()?;
    let device_identity = device.cast::<IUnknown>().ok()?.as_raw();
    let mut queues = offsets.iter().filter_map(|&at| {
        let pointer = mem::read::<usize>(swap as usize + at)?;
        if !monaka_hook::module::plausible_object(pointer, 3) {
            return None;
        }
        let raw = pointer as *mut c_void;
        // SAFETY: a live COM object the swapchain holds a reference to (checked plausible above);
        // borrowed and queried, which adds our own reference.
        let queue = unsafe { IUnknown::from_raw_borrowed(&raw) }?.cast::<ID3D12CommandQueue>().ok()?;
        // SAFETY: plain query of a live queue.
        let kind = unsafe { queue.GetDesc() }.Type;
        let owner = monaka_channel::d3d12::device_of(&queue).ok()?;
        (kind == D3D12_COMMAND_LIST_TYPE_DIRECT && owner.cast::<IUnknown>().ok()?.as_raw() == device_identity).then_some(queue)
    });
    let queue = queues.next()?;
    // Several places may hold it (an interface and its object); they must agree.
    queues.all(|other| other.as_raw() == queue.as_raw()).then_some(queue)
}

#[cfg(test)]
mod tests;
