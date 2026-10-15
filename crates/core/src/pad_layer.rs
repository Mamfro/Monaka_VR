//! What the controllers' gamepad goes through after it is read and before anything else takes it
//! ([`Layers`]): the controller proxy, the d-pad layer and the held d-pad choices. The viewer reads
//! the controls from the runtime; the harness gives them by script; both apply these the same way.
//!
//! A controller without a d-pad or a view button (Quest's Touch controllers) gets them through a
//! layer ([`layer`]): while it is on (the left thumb resting on the left thumbrest, by default), a
//! stick pushed is the d-pad and the menu button is Back. The thumbrest only senses touch, so it
//! takes nothing from the game; the thumb resting there leaves the right stick free for the d-pad.
//!
//! A game whose d-pad opens a choice that stays open only while the d-pad is held (Dying Light:
//! left for tools, right for weapons, picked with A, B, X or Y) names those directions: pushed
//! through the layer, such a direction stays held ([`Latch`]) until a pick is made, so both thumbs
//! are free for the face buttons.
//!
//! A controller proxy makes the controllers at hand act like another kind, to try its layout
//! without it: what that kind lacks reads as released, and the left grip stands in for what it has
//! instead ([`proxied`]).

use monaka_contract::{gamepad::Gamepad, gamepad::button};
use std::time::{Duration, Instant};

/// Which controller the ones at hand act like.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Proxy {
    #[default]
    None,
    /// Quest's Touch controllers: no d-pad, no view button; the thumbrest makes them.
    Touch,
    /// Valve Index controllers: no d-pad; touching the left trackpad makes it.
    Index,
}

impl Proxy {
    pub fn parse(word: &str) -> Result<Self, String> {
        match word {
            "off" | "none" => Ok(Proxy::None),
            "touch" | "quest" => Ok(Proxy::Touch),
            "index" => Ok(Proxy::Index),
            other => Err(format!("{other} is not touch or index")),
        }
    }
}

/// The layer controls as read this time: the d-pad layer (with Back), the layer without Back (a
/// controller with its own view button), and the proxy's stand-in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Held {
    pub layer: bool,
    pub shift: bool,
    pub rest: bool,
}

/// The proxy, the layer and the latch, kept between reads.
#[derive(Clone, Copy, Debug, Default)]
pub struct Layers {
    /// The d-pad directions that stay held once pushed through the layer.
    pub hold: u16,
    pub proxy: Proxy,
    /// The layer was used during the current hold ([`layer`]).
    used: bool,
    latch: Latch,
}

impl Layers {
    pub const fn new(hold: u16, proxy: Proxy) -> Self {
        Self { hold, proxy, used: false, latch: Latch { held: 0, since: None, picked: false, last_pushed: 0 } }
    }

    /// `pad` as the game should get it, from the controls as read (`pad`, and `held`, the layer
    /// controls).
    pub fn apply(&mut self, pad: &mut Gamepad, held: Held, now: Instant) {
        let (proxy_layer, proxy_shift) = proxied(pad, self.proxy, held.rest);
        let (layer_held, shift_held) = (held.layer || proxy_layer, held.shift || proxy_shift);
        let pushed = layer(pad, layer_held || shift_held, layer_held, &mut self.used);
        self.latch.step(pad, pushed, self.hold, now);
    }
}

/// How far a stick goes (of its range) before the layer reads a d-pad direction.
const LAYER_PUSH: f32 = 0.6;

/// The d-pad layer on `pad`, while it is `held`: the stick pushed furthest past [`LAYER_PUSH`]
/// presses the d-pad toward its larger axis (and neither moves nor turns), and the menu button is
/// Back instead of Start. With the thumbrest that is the right stick (the left thumb rests); bound
/// to a button the pad already has (the left grip, which is also the left bumper), once the layer
/// has been used in a hold (`used`) the bumper is let go for the rest of that hold, so a plain hold
/// still works as the bumper.
/// `back`: the menu button is Back while held (not for a controller with its own view button).
/// Returns the d-pad direction the layer pressed (0: none).
fn layer(pad: &mut Gamepad, held: bool, back: bool, used: &mut bool) -> u16 {
    if !held {
        *used = false;
        return 0;
    }
    let mut pressed = 0;
    let reach = |x: i16, y: i16| (x as f32 / i16::MAX as f32, y as f32 / i16::MAX as f32);
    let (left, right) = (reach(pad.thumb_lx, pad.thumb_ly), reach(pad.thumb_rx, pad.thumb_ry));
    let push = |(x, y): (f32, f32)| x.abs().max(y.abs());
    let (stick, is_left) = if push(left) >= push(right) { (left, true) } else { (right, false) };
    if push(stick) >= LAYER_PUSH {
        let (x, y) = stick;
        pressed = match (x.abs() > y.abs(), x > 0.0, y > 0.0) {
            (true, true, _) => button::DPAD_RIGHT,
            (true, false, _) => button::DPAD_LEFT,
            (false, _, true) => button::DPAD_UP,
            (false, _, false) => button::DPAD_DOWN,
        };
        pad.buttons |= pressed;
        if is_left {
            (pad.thumb_lx, pad.thumb_ly) = (0, 0);
        } else {
            (pad.thumb_rx, pad.thumb_ry) = (0, 0);
        }
        *used = true;
    }
    if back && pad.buttons & button::START != 0 {
        pad.buttons = (pad.buttons & !button::START) | button::BACK;
        *used = true;
    }
    if *used {
        pad.buttons &= !button::LEFT_SHOULDER;
    }
    pressed
}

const DPAD: u16 = button::DPAD_UP | button::DPAD_DOWN | button::DPAD_LEFT | button::DPAD_RIGHT;

/// The controller proxy on `pad`: what the emulated controller lacks reads as released, and the
/// stand-in (`rest`, the left grip) turns on what it has instead. Returns whether the stand-in
/// holds the layer (with Back) or the shift (without). The stand-in's own binding (the grip is the
/// left stick click on the Frame) reads as released while it stands in.
fn proxied(pad: &mut Gamepad, proxy: Proxy, rest: bool) -> (bool, bool) {
    let lacks = match proxy {
        Proxy::None => return (false, false),
        Proxy::Touch => DPAD | button::BACK,
        Proxy::Index => DPAD,
    };
    pad.buttons &= !lacks;
    if rest {
        pad.buttons &= !button::LEFT_THUMB;
    }
    (rest && proxy == Proxy::Touch, rest && proxy == Proxy::Index)
}

/// The face buttons a held-d-pad choice is picked with.
const FACE: u16 = button::A | button::B | button::X | button::Y;
/// A latched direction lets go by itself after this long without a pick.
const LATCH_FOR: Duration = Duration::from_secs(5);

/// A d-pad direction held for the player: pushed through the layer (a new push), a direction of
/// [`Layers::hold`] stays held until a face button has been pressed and let go (the game sees the
/// d-pad held through the pick, as a player holding it would), the same direction is pushed again,
/// or [`LATCH_FOR`] passes.
#[derive(Clone, Copy, Debug, Default)]
struct Latch {
    held: u16,
    since: Option<Instant>,
    /// A face button was pressed while held: the pick is under way.
    picked: bool,
    /// The direction the layer pressed last read (a push counts once).
    last_pushed: u16,
}

impl Latch {
    fn step(&mut self, pad: &mut Gamepad, pushed: u16, hold: u16, now: Instant) {
        let new = pushed & !self.last_pushed;
        self.last_pushed = pushed;
        if new & hold != 0 {
            if self.held == new {
                *self = Latch { last_pushed: pushed, ..Latch::default() };
                return;
            }
            *self = Latch { held: new & hold, since: Some(now), picked: false, last_pushed: pushed };
        }
        if self.held == 0 {
            return;
        }
        if self.since.is_some_and(|since| now.duration_since(since) >= LATCH_FOR) {
            *self = Latch { last_pushed: pushed, ..Latch::default() };
            return;
        }
        pad.buttons |= self.held;
        if pad.buttons & FACE != 0 {
            self.picked = true;
        } else if self.picked {
            // The pick let go: so does the d-pad, from the next read.
            *self = Latch { last_pushed: pushed, ..Latch::default() };
        }
    }
}

/// The d-pad directions a comma-separated list names (`left,right`; `up`, `down` too).
pub fn dpad_directions(list: &str) -> Result<u16, String> {
    list.split(',').map(str::trim).filter(|d| !d.is_empty()).try_fold(0u16, |bits, d| {
        Ok(bits
            | match d {
                "up" => button::DPAD_UP,
                "down" => button::DPAD_DOWN,
                "left" => button::DPAD_LEFT,
                "right" => button::DPAD_RIGHT,
                other => return Err(format!("{other} is not up, down, left or right")),
            })
    })
}

#[cfg(test)]
mod tests;
