//! The VR controllers as the game's gamepad: the viewer writes them as one Xbox-layout pad into
//! the hand block ([`monaka_core::pad`]); this hands that pad to the game as XInput pad 0, merged
//! with a real pad when one is plugged in. Every control and menu then works as with a real pad,
//! with the game's own prompts; a game may move some of the controllers' buttons onto others for
//! VR (`remap`).
//!
//! Three XInput calls are caught: reading the pad (`XInputGetState`, or `XInputGetStateEx`, #100,
//! as SDL reads it) and asking what is plugged in (`XInputGetCapabilities`, which reports a pad in
//! slot 0 while the VR pad is live and no real one is: the game sees one plugged in, and it
//! unplugs when the viewer stops). They are caught at both ends ([`install`]), because callers
//! reach XInput two ways: through the XInput DLLs' own code (libraries that look the functions up
//! themselves, as SDL does), and through the game modules' import tables, which the Steam overlay
//! points at its own XInput hooks, so those calls never reach the DLLs' code (in Dying Light
//! the engine's reads go engine, overlay, XInput, driver). A read that passes
//! both is merged once.
//!
//! Its parts: XInput's calls caught and each read answered (`xinput`), the VR pad's block and
//! telling the game a pad came (`vr_pad`), what the producer presses itself (`presses`), and the
//! right stick taken and the left trigger held back for the producer (`stick`).

pub mod actions;
pub mod feature;
pub mod focus;
pub mod gameinput;
pub mod mouse;
pub mod probe;
pub mod remap;
mod presses;
mod stick;
mod vr_pad;
mod xinput;

// The pad's calls, by the names the games and the other modules use.
pub use presses::{flick_right_stick, hold, hold_buttons, let_go, press, rewrite};
pub use stick::{buttons, gate_left_trigger, left_trigger, left_trigger_held_back, lend_right_stick_y, open_left_trigger, right_stick, take_right_stick, take_right_stick_y, turn_walk};
pub use vr_pad::{announce, live, mask_real, release, stop};
pub(crate) use vr_pad::{right_stick_rules, virtual_pad};
pub use xinput::{XInputCapabilities, XInputState, install, install_optional, report};

/// Tests that press the one VR pad take turns.
#[cfg(test)]
pub(crate) static TEST_PRESSES: std::sync::Mutex<()> = std::sync::Mutex::new(());

