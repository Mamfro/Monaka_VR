//! What features hang on the present loop: a game's pose report for each published pair
//! ([`Report`]), and a feature publishing its own pairs at the loop's step ([`join_present`]).

use super::Driver;
use monaka_core::protocol::HeadPose;
use monaka_core::sync::Locked;
use std::sync::Mutex;
use windows::Win32::Graphics::Dxgi::IDXGISwapChain;

/// A game's pose report for each eye of a published pair ([`Install::report`]).
pub type Report = fn(&Driver, &mut HeadPose);

/// A feature publishing its own pairs at the present loop's step (an upscaled eye, a pair made with
/// depth), after the game's own step ([`Install::before`]): true when it took the frame, so the
/// loop publishes nothing itself.
pub type OwnPairs = fn(&Driver, &IDXGISwapChain, u64) -> bool;

pub(super) static OWN_PAIRS: Mutex<Vec<OwnPairs>> = Mutex::new(Vec::new());

/// A feature (at its start) publishes its own pairs at each present while it runs.
pub fn join_present(step: OwnPairs) {
    let mut steps = OWN_PAIRS.locked();
    if !steps.iter().any(|&s| std::ptr::fn_addr_eq(s, step)) {
        steps.push(step);
    }
}

/// The feature stops publishing its own pairs (at its release).
pub fn leave_present(step: OwnPairs) {
    OWN_PAIRS.locked().retain(|&s| !std::ptr::fn_addr_eq(s, step));
}

