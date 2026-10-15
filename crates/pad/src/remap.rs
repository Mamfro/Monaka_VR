//! A game's VR button layout: some of the VR controllers' buttons reaching the game as others. A
//! game whose own pad layout puts something the VR layout wants on another button says once which
//! of the controllers' buttons reaches it as which ([`Remaps`]), and lists
//! `crate::feature::remap()`; from its arm to its stop every read of the VR pad is remapped.
//!
//! Only the controllers' buttons are remapped. A real pad keeps the game's own layout, and what the
//! producer presses itself (an action on the pad, [`crate::actions`]) reaches the game as pressed:
//! a ready-made can still press the button a controller's button was moved off (the crouch toggle
//! a duck taps on B while the controllers' B is the flashlight).
//!
//! A remap may apply only at some times ([`Remaps::applies`]: in play, not in a menu). Each button
//! keeps what it was as it went down until it is let go, so a remap that starts or stops applying
//! while a button is held never lets one control go and presses another under the player's thumb.

use monaka_contract::gamepad::Gamepad;
use monaka_core::sync::Locked;
use monaka_producer::game::Game;
use monaka_producer::log;
use std::sync::Mutex;

/// One of the controllers' buttons reaching the game as another.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Remap {
    /// The controllers' button (one XInput button bit, `monaka_contract::gamepad::button`).
    pub from: u16,
    /// What the game gets instead (XInput button bits).
    pub to: u16,
}

/// A game's VR button layout: which of the controllers' buttons reach it as which.
pub trait Remaps: Game {
    const REMAPS: &'static [Remap];

    /// Whether the remaps apply to a button going down now. Asked at each press of a remapped
    /// button, on the thread reading the pad: quick, and never waiting. Always, by default.
    fn applies() -> bool {
        true
    }
}

/// The installed game's remaps and when they apply.
#[derive(Clone, Copy)]
struct Table {
    remaps: &'static [Remap],
    applies: fn() -> bool,
}

/// The controllers' remapped buttons down at the last read, and which of them went down remapped.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Held {
    down: u16,
    remapped: u16,
}

impl Held {
    pub(crate) const NONE: Held = Held { down: 0, remapped: 0 };
}

static TABLE: Mutex<Option<Table>> = Mutex::new(None);
static HELD: Mutex<Held> = Mutex::new(Held::NONE);

/// Game `G`'s remaps from now on (its component's arm).
pub fn install<G: Remaps>() {
    *HELD.locked() = Held::NONE;
    *TABLE.locked() = Some(Table { remaps: G::REMAPS, applies: G::applies });
    let remaps: Vec<String> = G::REMAPS.iter().map(|r| format!("{:#06x} as {:#06x}", r.from, r.to)).collect();
    log!("VR buttons: the controllers' {}", remaps.join(", "));
}

/// The controllers' buttons reach the game as they are again (the component's stop).
pub fn release() {
    *TABLE.locked() = None;
    *HELD.locked() = Held::NONE;
}

/// The controllers' pad as the game gets it, with the installed remaps (as it is without them).
pub(crate) fn controllers(pad: Gamepad) -> Gamepad {
    let Some(table) = *TABLE.locked() else { return pad };
    let buttons = remapped(pad.buttons, table.remaps, table.applies, &mut HELD.locked());
    Gamepad { buttons, ..pad }
}

/// The controllers' `buttons` as the game gets them with `remaps`, `held` carried from the last
/// read: a remapped button going down now is remapped when `applies` says so, and stays as it went
/// down until it is let go.
pub(crate) fn remapped(buttons: u16, remaps: &[Remap], applies: impl FnOnce() -> bool, held: &mut Held) -> u16 {
    let from = remaps.iter().fold(0, |all, r| all | r.from);
    let down = buttons & from;
    let pressed = down & !held.down;
    if pressed != 0 && applies() {
        held.remapped |= pressed;
    }
    held.down = down;
    held.remapped &= down;
    remaps.iter().filter(|r| held.remapped & r.from != 0).fold(buttons & !held.remapped, |out, r| out | r.to)
}

#[cfg(test)]
mod tests;
