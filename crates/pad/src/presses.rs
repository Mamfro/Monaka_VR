//! What the producer presses on the VR pad itself, on top of the controllers': a press for a
//! while or held by its source ([`press`], [`hold`], [`let_go`]), buttons held ([`hold_buttons`]),
//! a right-stick flick ([`flick_right_stick`]) and a last rewrite of every pad ([`rewrite`]).

use crate::actions;
use monaka_core::pad::merge;
use monaka_contract::gamepad::Gamepad;
use monaka_core::sync::Locked;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering::*};

/// Controls the producer presses itself, on top of the controllers': by who presses them, each
/// with the tick (`monaka_channel::tick`) it lets go at.
pub(super) static PRESSED: Mutex<Vec<(&'static str, Gamepad, u64)>> = Mutex::new(Vec::new());

/// Presses `pad`'s controls on the VR pad for `for_ms`, as `source` (a producer's own input: a
/// melee swing's attack, a climb's stick push), merged like a real pad's. Replaces an earlier press
/// of the same source still held; other sources' presses stay.
pub fn press(source: &'static str, pad: Gamepad, for_ms: u64) {
    pressed_until(source, pad, monaka_channel::tick().saturating_add(for_ms));
}

/// Holds `pad`'s controls down on the VR pad as `source` until it lets go ([`let_go`]), as
/// [`press`] does for a while: a press of the same source replaces it.
pub fn hold(source: &'static str, pad: Gamepad) {
    pressed_until(source, pad, HELD_UNTIL_LET_GO);
}

/// The tick a [`hold`] lasts until: never, until let go.
pub(super) const HELD_UNTIL_LET_GO: u64 = u64::MAX;

pub(super) fn pressed_until(source: &'static str, pad: Gamepad, until: u64) {
    let mut pressed = PRESSED.locked();
    pressed.retain(|(s, _, _)| *s != source);
    pressed.push((source, pad, until));
}

/// Lets go of what `source` presses ([`press`], [`hold`]), at once.
pub fn let_go(source: &'static str) {
    PRESSED.locked().retain(|(s, _, _)| *s != source);
}

/// Buttons the producer holds down for as long as it says ([`hold_buttons`]).
pub(super) static HELD_BUTTONS: AtomicU32 = AtomicU32::new(0);

/// Holds `buttons` (XInput button bits) down on the VR pad while `on`, on top of the controllers'
/// and of a [`press`] (a guard pose held: Dishonored's block), until called again with `on` false.
pub fn hold_buttons(buttons: u16, on: bool) {
    if on {
        HELD_BUTTONS.fetch_or(u32::from(buttons), Relaxed);
    } else {
        HELD_BUTTONS.fetch_and(!u32::from(buttons), Relaxed);
    }
}

/// A right-stick flick the producer makes ([`flick_right_stick`]): the stick and the tick it lets
/// go at.
pub(super) static FLICK: Mutex<([i16; 2], u64)> = Mutex::new(([0; 2], 0));

/// Pushes the right stick to `stick` (x right, y up, -1..=1) for `for_ms`, through a take
/// ([`take_right_stick`], [`take_right_stick_y`]): the producer moves the game's stick on purpose
/// (Kingdom Come picks a sword's attack direction with it), where the controllers' own stick stays
/// taken. Replaces an earlier flick still held.
pub fn flick_right_stick(stick: [f32; 2], for_ms: u64) {
    let axis = |v: f32| (v.clamp(-1.0, 1.0) * 32767.0) as i16;
    if let Ok(mut flick) = FLICK.lock() {
        *flick = ([axis(stick[0]), axis(stick[1])], monaka_channel::tick() + for_ms);
    }
}

/// The flick held now, if any.
pub(super) fn flick() -> Option<[i16; 2]> {
    FLICK.lock().ok().filter(|f| monaka_channel::tick() < f.1).map(|f| f.0)
}

/// The producer's rewrite of the pad the game reads ([`rewrite`]).
pub(super) static REWRITE: Mutex<Option<fn(&mut Gamepad)>> = Mutex::new(None);

/// Has `rewrite` change every pad handed to the game, last (after the takes, flicks and gates):
/// a producer that turns the sticks from the game camera's heading into the player's. `None`
/// stops it.
pub fn rewrite(rewrite: Option<fn(&mut Gamepad)>) {
    *REWRITE.locked() = rewrite;
}

pub(super) fn rewritten(pad: &mut Gamepad) {
    if let Some(rewrite) = *REWRITE.locked() {
        rewrite(pad);
    }
}

/// What [`press`], [`hold_buttons`], [`flick_right_stick`] and the analog actions followed on the
/// sticks ([`actions::follow`]) hold now: every source's presses
/// merged as two pads held together would be.
pub(super) fn pressed() -> Gamepad {
    let now = monaka_channel::tick();
    let mut pressed = PRESSED.locked();
    pressed.retain(|(_, _, until)| now < *until);
    let mut pad = pressed.iter().fold(Gamepad::default(), |merged, (_, pad, _)| merge(merged, *pad));
    drop(pressed);
    pad = merge(pad, actions::sticks());
    pad.buttons |= HELD_BUTTONS.load(Relaxed) as u16;
    if let Some([x, y]) = flick() {
        (pad.thumb_rx, pad.thumb_ry) = (x, y);
    }
    pad
}

#[cfg(test)]
mod tests;
