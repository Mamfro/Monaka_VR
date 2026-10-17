//! What features hang on the present loop: a feature publishing its own pairs at the loop's step
//! ([`join_present`]).

use super::Driver;
use monaka_core::sync::Locked;
use monaka_producer::log;
use std::sync::Mutex;
use windows::Win32::Graphics::Dxgi::IDXGISwapChain;

/// A feature publishing its own pairs at the present loop's step (an upscaled eye, a pair made with
/// depth), after the game's own step ([`Install::before`]): true when it took the frame, so the
/// loop publishes nothing itself.
pub type OwnPairs = fn(&Driver, &IDXGISwapChain, u64) -> bool;

/// The features publishing their own pairs, in the order they joined; copied out at each present
/// (no allocation there).
pub(super) static OWN_PAIRS: Mutex<[Option<OwnPairs>; MAX_OWN_PAIRS]> = Mutex::new([None; MAX_OWN_PAIRS]);
const MAX_OWN_PAIRS: usize = 8;

/// A feature (at its start) publishes its own pairs at each present while it runs.
pub fn join_present(step: OwnPairs) {
    let mut steps = OWN_PAIRS.locked();
    if steps.iter().flatten().any(|&s| std::ptr::fn_addr_eq(s, step)) {
        return;
    }
    match steps.iter_mut().find(|s| s.is_none()) {
        Some(free) => *free = Some(step),
        None => log!("no room for another feature publishing its own pairs ({MAX_OWN_PAIRS} already); it publishes none"),
    }
}

/// The feature stops publishing its own pairs (at its release).
pub fn leave_present(step: OwnPairs) {
    let mut steps = OWN_PAIRS.locked();
    let kept: Vec<OwnPairs> = steps.iter().flatten().copied().filter(|&s| !std::ptr::fn_addr_eq(s, step)).collect();
    *steps = [None; MAX_OWN_PAIRS];
    for (slot, s) in steps.iter_mut().zip(kept) {
        *slot = Some(s);
    }
}

