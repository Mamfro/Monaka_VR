//! Vulkan games' frames out to Monaka. A Vulkan device shares nothing unless it was made with the
//! external memory and semaphore extensions, so the game's device is hooked as it is made (the
//! producer joins the game as it starts, `monaka_contract::manifest::Launch::AtStart`), the extensions
//! added; then at each present the frame is copied, on the game's own queue, into buffers a D3D12
//! device of Monaka's own made and shares, and handed to the game's frame handler there as a D3D12
//! texture ([`share::Frame`]), for the D3D12 side of Monaka (pairs, warps) to take it on.
//!
//! - [`device`]: the hooks on the game executable's Vulkan imports (the device, its queues, its
//!   swapchains with their images also a copy's source, its presents);
//! - [`share`]: the ring of shared buffers and the two shared fences, the copy at each present;
//! - `ffi`: the few Vulkan declarations used.

pub mod device;
mod ffi;
pub mod share;

pub use share::{Depth, DepthFn, Frame, FrameFn, Layer, LayerFn, on_depth, on_frame, on_layer};

use monaka_hook::Hooks;
use monaka_hook::module::Module;
use monaka_producer::Rejection;

/// Hooks `module`'s Vulkan imports (the game executable's), so its device is made able to share
/// and each present hands its frame to [`on_frame`]'s handler.
///
/// # Safety
/// Called once at start, before the hooks are enabled, before the game makes its device.
pub unsafe fn install(hooks: &mut Hooks, module: &Module) -> Result<(), Rejection> {
    // SAFETY: the caller's guarantee.
    unsafe { device::install(hooks, module)? };
    // Every copy's present (the game keeps the first copy's) now calls this copy's.
    device::take_presents(share::present_handler);
    Ok(())
}

/// The sharing's line for the stop.
pub fn summary() -> String {
    share::summary()
}

/// Gives the presents back and lets go of the shared buffers (after the hooks are out).
pub fn close() {
    device::give_back_presents(share::present_handler);
    share::close();
}
