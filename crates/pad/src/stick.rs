//! The right stick and the left trigger as the game's last pad read had them: taken from the game
//! for the producer ([`take_right_stick`], [`take_right_stick_y`], the vertical axis lent back to a
//! menu of the game's with [`lend_right_stick_y`]; read with [`right_stick`], [`left_trigger`],
//! [`buttons`]), and a new left trigger press held back until the producer lets it through
//! ([`gate_left_trigger`]).

use monaka_contract::gamepad::Gamepad;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::*};


/// Set while the right stick goes to the producer instead of the game ([`take_right_stick`]).
pub(super) static TAKE_RIGHT: AtomicBool = AtomicBool::new(false);
/// Set while only the right stick's vertical axis is taken ([`take_right_stick_y`]).
pub(super) static TAKE_RIGHT_Y: AtomicBool = AtomicBool::new(false);
/// The right stick taken at the last read (x and y as `i16` bits), and the tick of that read.
pub(super) static RIGHT_STICK: AtomicU32 = AtomicU32::new(0);
pub(super) static RIGHT_TICK: AtomicU64 = AtomicU64::new(0);
/// The buttons (VR and real pads merged) at the game's last pad read, with the right stick's tick.
pub(super) static BUTTONS: AtomicU32 = AtomicU32::new(0);
/// The left trigger handed to the game at that read (0..=255).
pub(super) static LEFT_TRIGGER: AtomicU32 = AtomicU32::new(0);
/// The left trigger's gate ([`gate_left_trigger`]): on, and the state of the press (idle, held
/// back since a tick, let through).
pub(super) static GATE_LEFT: AtomicBool = AtomicBool::new(false);
pub(super) static LEFT_PRESS: AtomicU32 = AtomicU32::new(PRESS_IDLE);
pub(super) static LEFT_PRESS_TICK: AtomicU64 = AtomicU64::new(0);
pub(super) const PRESS_IDLE: u32 = 0;
pub(super) const PRESS_HELD: u32 = 1;
pub(super) const PRESS_OPEN: u32 = 2;
/// XInput's own trigger threshold: a game acts on a pull past it.
pub(super) const TRIGGER_THRESHOLD: u8 = 30;
/// The longest a press is held back (ms), whatever the producer says.
pub(super) const GATE_MAX_MS: u64 = 250;

/// Holds each new left trigger press back from the game until the producer lets it through
/// ([`open_left_trigger`], at most [`GATE_MAX_MS`]): a producer that turns the game's aim on the
/// press (a left-hand weapon aimed by its controller) lets the shot go once the aim is there.
pub fn gate_left_trigger(on: bool) {
    GATE_LEFT.store(on, Release);
    if !on {
        LEFT_PRESS.store(PRESS_IDLE, Release);
    }
}

/// Lets a held-back left trigger press through to the game (until the trigger is let go).
pub fn open_left_trigger() {
    let _ = LEFT_PRESS.compare_exchange(PRESS_HELD, PRESS_OPEN, AcqRel, Acquire);
}

/// Whether a left trigger press is being held back from the game now.
pub fn left_trigger_held_back() -> bool {
    LEFT_PRESS.load(Acquire) == PRESS_HELD
}

/// Follows the left trigger's press through the gate; whether the game must see it released.
pub(super) fn gate_left(raw: u8) -> bool {
    let now = monaka_channel::tick();
    if raw < TRIGGER_THRESHOLD {
        LEFT_PRESS.store(PRESS_IDLE, Release);
        return false;
    }
    match LEFT_PRESS.load(Acquire) {
        PRESS_IDLE if GATE_LEFT.load(Acquire) => {
            LEFT_PRESS.store(PRESS_HELD, Release);
            LEFT_PRESS_TICK.store(now, Release);
            true
        }
        PRESS_IDLE => {
            LEFT_PRESS.store(PRESS_OPEN, Release);
            false
        }
        PRESS_HELD if now.saturating_sub(LEFT_PRESS_TICK.load(Acquire)) >= GATE_MAX_MS => {
            LEFT_PRESS.store(PRESS_OPEN, Release);
            false
        }
        PRESS_HELD => true,
        _ => false,
    }
}

/// Takes the right stick (VR and real pads merged) away from the game: the game reads it centred
/// and the producer reads it with [`right_stick`]. For a producer that feeds the player's look in
/// another way (as mouse input, so the game sees one look device: a game that also gets mouse
/// counts from head aim weakens its stick look while they come).
pub fn take_right_stick(on: bool) {
    TAKE_RIGHT.store(on, Release);
}

/// Takes only the right stick's vertical axis from the game (it reads it centred), for a producer
/// that leaves the game its stick turning but not its stick pitch.
pub fn take_right_stick_y(on: bool) {
    TAKE_RIGHT_Y.store(on, Release);
}

/// When the taken vertical axis was last lent back to the game ([`lend_right_stick_y`]; 0: never).
static LENT_Y_TICK: AtomicU64 = AtomicU64::new(0);
/// A lend older than this (ms) is over: whoever lent it no longer says the game needs it.
const LEND_FRESH_MS: u64 = 100;

/// Lends the taken vertical axis back to the game for a moment ([`LEND_FRESH_MS`]): a menu of the
/// game's own steered by the stick (a weapon wheel) gets the whole stick while it is open. Said
/// again at each frame the menu is open.
pub fn lend_right_stick_y() {
    LENT_Y_TICK.store(monaka_channel::tick().max(1), Release);
}

/// Whether the right stick's vertical axis is taken from the game now: taken and not lent back.
pub(super) fn takes_y() -> bool {
    let lent = LENT_Y_TICK.load(Acquire);
    TAKE_RIGHT_Y.load(Acquire) && (lent == 0 || monaka_channel::tick().saturating_sub(lent) > LEND_FRESH_MS)
}

/// The right stick (VR and real pads merged, with a producer's [`press`]) at the game's last pad
/// read, taken or not, x right and y up in -1..=1, if that read was at most `max_age_ms` ago.
pub fn right_stick(max_age_ms: u64) -> Option<[f32; 2]> {
    let tick = RIGHT_TICK.load(Acquire);
    if tick == 0 || monaka_channel::tick().saturating_sub(tick) > max_age_ms {
        return None;
    }
    let bits = RIGHT_STICK.load(Acquire);
    let axis = |v: u16| (v as i16 as f32 / 32767.0).clamp(-1.0, 1.0);
    Some([axis(bits as u16), axis((bits >> 16) as u16)])
}

/// The left trigger (VR and real pads merged) at the game's last pad read, 0..=1, if that read
/// was at most `max_age_ms` ago: a producer aiming a left-hand action while it is held.
pub fn left_trigger(max_age_ms: u64) -> Option<f32> {
    let tick = RIGHT_TICK.load(Acquire);
    if tick == 0 || monaka_channel::tick().saturating_sub(tick) > max_age_ms {
        return None;
    }
    Some(LEFT_TRIGGER.load(Acquire) as f32 / 255.0)
}

/// The buttons (VR and real pads merged, with a producer's [`press`]) at the game's last pad read,
/// if that read was at most `max_age_ms` ago: a producer following what the player holds (a menu
/// the game keeps open while a button is held).
pub fn buttons(max_age_ms: u64) -> Option<u16> {
    let tick = RIGHT_TICK.load(Acquire);
    if tick == 0 || monaka_channel::tick().saturating_sub(tick) > max_age_ms {
        return None;
    }
    Some(BUTTONS.load(Acquire) as u16)
}

/// How far the game's look is turned from the head (radians, positive left; `f32` bits), and when
/// it was said (`monaka_channel::tick`, 0: never) ([`turn_walk`]).
static WALK_TURN: AtomicU32 = AtomicU32::new(0);
static WALK_TURN_TICK: AtomicU64 = AtomicU64::new(0);
/// A walk turn older than this (ms) is let go: whoever said it no longer steers.
const WALK_TURN_FRESH_MS: u64 = 100;

/// The game's look is turned `radians` (positive left) from where the player looks, as a
/// producer's hand aim turns it toward a gun while the view stays the head's: the game walks
/// along its look, so the left stick is turned back by as much ([`walk_turned`]) and the player
/// walks where they look. Said again at each update; it lapses after [`WALK_TURN_FRESH_MS`].
pub fn turn_walk(radians: f32) {
    WALK_TURN.store(radians.to_bits(), Release);
    WALK_TURN_TICK.store(monaka_channel::tick().max(1), Release);
}

/// `pad`'s left stick turned back by the walk turn ([`turn_walk`]) while it is fresh.
pub(super) fn walk_turned(pad: &mut Gamepad) {
    let tick = WALK_TURN_TICK.load(Acquire);
    if tick == 0 || monaka_channel::tick().saturating_sub(tick) > WALK_TURN_FRESH_MS {
        return;
    }
    (pad.thumb_lx, pad.thumb_ly) = turned(pad.thumb_lx, pad.thumb_ly, f32::from_bits(WALK_TURN.load(Acquire)));
}

/// A stick (x right, y forward) as the game must read it to move along the stick in the player's
/// frame when its own forward is turned `turn` radians left of the player's: turned `turn` right.
fn turned(x: i16, y: i16, turn: f32) -> (i16, i16) {
    if turn.abs() < 1e-3 || !turn.is_finite() {
        return (x, y);
    }
    let (sin, cos) = turn.sin_cos();
    let (x, y) = (f32::from(x), f32::from(y));
    let axis = |v: f32| v.round().clamp(-32767.0, 32767.0) as i16;
    (axis(x * cos + y * sin), axis(y * cos - x * sin))
}

/// Keeps `pad`'s right stick, left trigger and buttons for the producer ([`right_stick`],
/// [`left_trigger`], [`buttons`]); with [`TAKE_RIGHT`] also centres the stick for the game.
pub(super) fn take_right(pad: &mut Gamepad) {
    LEFT_TRIGGER.store(u32::from(pad.left_trigger), Release);
    BUTTONS.store(u32::from(pad.buttons), Release);
    if gate_left(pad.left_trigger) {
        pad.left_trigger = 0;
    }
    RIGHT_STICK.store(pad.thumb_rx as u16 as u32 | (pad.thumb_ry as u16 as u32) << 16, Release);
    RIGHT_TICK.store(monaka_channel::tick().max(1), Release);
    if TAKE_RIGHT.load(Acquire) {
        pad.thumb_rx = 0;
        pad.thumb_ry = 0;
    } else if takes_y() {
        pad.thumb_ry = 0;
    }
}

#[cfg(test)]
mod tests;
