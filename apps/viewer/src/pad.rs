//! The VR controllers as one Xbox-layout gamepad for the game ([`monaka_core::pad`]). Each control
//! is an OpenXR action named for the gamepad control it drives, so the runtime's binding screen
//! (SteamVR's controller bindings) can remap any controller; the defaults below put every
//! controller's buttons where an Xbox pad has them: thumbsticks to sticks, triggers to triggers,
//! grips to bumpers, face buttons to A/B (right) and X/Y (left), menu to Start.
//!
//! A controller without a d-pad or a view button (Quest's Touch controllers) gets them through a
//! layer ([`layer`]): while it is on (the left thumb resting on the left thumbrest, by default), a
//! stick pushed is the d-pad and the menu button is Back. The thumbrest only senses touch, so it
//! takes nothing from the game; the thumb resting there leaves the right stick free for the d-pad.
//!
//! A game whose d-pad opens a choice that stays open only while the d-pad is held (Dying Light:
//! left for tools, right for weapons, picked with A, B, X or Y) names those directions
//! (`--dpad-hold`): pushed through the layer, such a direction stays held ([`Latch`]) until a pick
//! is made, so both thumbs are free for the face buttons.
//!
//! Index controllers have a view button but no d-pad: their layer (`pad_dpad_shift`, on the left
//! trackpad's touch) makes only the d-pad, since pressing that trackpad is Start.
//!
//! A controller proxy (`--act-like touch|index`) makes the controllers at hand act like another
//! kind, to try its layout without it: what that kind lacks reads as released, and the left grip
//! stands in for what it has instead ([`proxied`]).

use monaka_core::pad::{Gamepad, button, stick, trigger};
use monaka_openxr::{Action, ActionSet, Result, Session};
use std::cell::Cell;
use std::time::{Duration, Instant};

/// What an action drives on the pad.
#[derive(Clone, Copy)]
enum Control {
    Button(u16),
    LeftTrigger,
    RightTrigger,
    LeftStick,
    RightStick,
    /// Held: the d-pad layer ([`layer`]): the d-pad and Back.
    Layer,
    /// Held: the d-pad layer without Back (a controller with its own view button).
    Shift,
    /// The controller proxy's stand-in for the emulated controller's layer input ([`proxied`]).
    Rest,
}

/// Which controller the ones at hand act like (`--act-like`).
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
    pub fn parse(word: &str) -> std::result::Result<Self, String> {
        match word {
            "off" | "none" => Ok(Proxy::None),
            "touch" | "quest" => Ok(Proxy::Touch),
            "index" => Ok(Proxy::Index),
            other => Err(format!("--act-like: {other} is not touch or index")),
        }
    }
}

/// Name (also the binding key below), label shown in the runtime's binding screen, control.
const CONTROLS: [(&str, &str, Control); 21] = [
    ("pad_left_stick", "Left stick (move)", Control::LeftStick),
    ("pad_right_stick", "Right stick (look)", Control::RightStick),
    ("pad_left_trigger", "Left trigger", Control::LeftTrigger),
    ("pad_right_trigger", "Right trigger", Control::RightTrigger),
    ("pad_left_bumper", "Left bumper", Control::Button(button::LEFT_SHOULDER)),
    ("pad_right_bumper", "Right bumper", Control::Button(button::RIGHT_SHOULDER)),
    ("pad_a", "A", Control::Button(button::A)),
    ("pad_b", "B", Control::Button(button::B)),
    ("pad_x", "X", Control::Button(button::X)),
    ("pad_y", "Y", Control::Button(button::Y)),
    ("pad_start", "Start (menu)", Control::Button(button::START)),
    ("pad_back", "Back (view)", Control::Button(button::BACK)),
    ("pad_left_stick_click", "Left stick click", Control::Button(button::LEFT_THUMB)),
    ("pad_right_stick_click", "Right stick click", Control::Button(button::RIGHT_THUMB)),
    ("pad_dpad_up", "D-pad up", Control::Button(button::DPAD_UP)),
    ("pad_dpad_down", "D-pad down", Control::Button(button::DPAD_DOWN)),
    ("pad_dpad_left", "D-pad left", Control::Button(button::DPAD_LEFT)),
    ("pad_dpad_right", "D-pad right", Control::Button(button::DPAD_RIGHT)),
    ("pad_dpad_layer", "D-pad layer (while on: a stick is the d-pad, menu is Back)", Control::Layer),
    ("pad_dpad_shift", "D-pad shift (while on: a stick is the d-pad)", Control::Shift),
    ("pad_proxy_rest", "Controller test: stands in for the thumbrest or the trackpad touch", Control::Rest),
];

const L: &str = "/user/hand/left/input/";
const R: &str = "/user/hand/right/input/";

/// Default bindings per interaction profile: (action name, hand prefix, input). Controls a
/// controller lacks are left for the binding screen (the d-pad, mostly).
pub fn default_bindings(profile: &str) -> Vec<(&'static str, String)> {
    let b = |name: &'static str, hand: &str, input: &str| (name, format!("{hand}{input}"));
    let sticks_triggers_grips = |grip: &str| {
        vec![
            b("pad_left_stick", L, "thumbstick"),
            b("pad_right_stick", R, "thumbstick"),
            b("pad_left_trigger", L, "trigger/value"),
            b("pad_right_trigger", R, "trigger/value"),
            b("pad_left_bumper", L, grip),
            b("pad_right_bumper", R, grip),
            b("pad_left_stick_click", L, "thumbstick/click"),
            b("pad_right_stick_click", R, "thumbstick/click"),
        ]
    };
    match profile.rsplit('/').next().unwrap_or_default() {
        // Valve's Steam Frame controllers are an Xbox pad split in two (SteamVR's
        // `frame_controller_profile.json`): every control maps one to one. Their grips have no
        // Xbox counterpart: see [`extra_bindings`].
        "frame_controller" => vec![
            b("pad_left_stick", L, "thumbstick"),
            b("pad_right_stick", R, "thumbstick"),
            b("pad_left_trigger", L, "trigger/value"),
            b("pad_right_trigger", R, "trigger/value"),
            b("pad_left_bumper", L, "bumper/click"),
            b("pad_right_bumper", R, "bumper/click"),
            b("pad_left_stick_click", L, "thumbstick/click"),
            b("pad_right_stick_click", R, "thumbstick/click"),
            b("pad_a", R, "a/click"),
            b("pad_b", R, "b/click"),
            b("pad_x", R, "x/click"),
            b("pad_y", R, "y/click"),
            b("pad_start", R, "menu/click"),
            b("pad_back", L, "view/click"),
            b("pad_dpad_up", L, "dpad_up/click"),
            b("pad_dpad_down", L, "dpad_down/click"),
            b("pad_dpad_left", L, "dpad_left/click"),
            b("pad_dpad_right", L, "dpad_right/click"),
        ],
        "touch_controller" => {
            let mut v = sticks_triggers_grips("squeeze/value");
            v.extend([b("pad_a", R, "a/click"), b("pad_b", R, "b/click"), b("pad_x", L, "x/click"), b("pad_y", L, "y/click"), b("pad_start", L, "menu/click")]);
            v
        }
        "index_controller" => {
            let mut v = sticks_triggers_grips("squeeze/value");
            v.extend([
                b("pad_a", R, "a/click"),
                b("pad_b", R, "b/click"),
                b("pad_x", L, "a/click"),
                b("pad_y", L, "b/click"),
                b("pad_start", L, "trackpad/force"),
                b("pad_back", R, "trackpad/force"),
            ]);
            v
        }
        "motion_controller" => {
            let mut v = sticks_triggers_grips("squeeze/click");
            v.extend([b("pad_a", R, "trackpad/click"), b("pad_x", L, "trackpad/click"), b("pad_start", L, "menu/click"), b("pad_back", R, "menu/click")]);
            v
        }
        "vive_controller" => vec![
            b("pad_left_stick", L, "trackpad"),
            b("pad_right_stick", R, "trackpad"),
            b("pad_left_trigger", L, "trigger/value"),
            b("pad_right_trigger", R, "trigger/value"),
            b("pad_left_bumper", L, "squeeze/click"),
            b("pad_right_bumper", R, "squeeze/click"),
            b("pad_a", R, "trackpad/click"),
            b("pad_left_stick_click", L, "trackpad/click"),
            b("pad_start", L, "menu/click"),
            b("pad_back", R, "menu/click"),
        ],
        "simple_controller" => vec![b("pad_right_trigger", R, "select/click"), b("pad_start", L, "menu/click")],
        _ => Vec::new(),
    }
}

/// Bindings beyond [`default_bindings`] that a profile may refuse (an input path not known for
/// sure): alternatives tried in order, each added to the defaults, before the defaults alone.
/// The Steam Frame's left grip clicks the left stick: games sprint on that click (Dying Light), and
/// the Frame's stick is hard to press in mid-push. SteamVR names the grip `grip` in
/// the Frame's own profile; OpenXR profiles mostly call it `squeeze`.
pub fn extra_bindings(profile: &str) -> Vec<Vec<(&'static str, String)>> {
    match profile.rsplit('/').next().unwrap_or_default() {
        // The left grip is also the controller proxy's stand-in ([`Proxy`]).
        "frame_controller" => ["grip/value", "squeeze/value", "grip/click", "squeeze/click"]
            .iter()
            .map(|grip| vec![("pad_left_stick_click", format!("{L}{grip}")), ("pad_proxy_rest", format!("{L}{grip}"))])
            .collect(),
        // No d-pad and no view button: the left thumbrest's touch turns on the layer that makes
        // them; a runtime that does not offer the thumbrest gets the left grip instead.
        "touch_controller" => ["thumbrest/touch", "squeeze/value"].iter().map(|input| vec![("pad_dpad_layer", format!("{L}{input}"))]).collect(),
        // No d-pad (the view button is the right trackpad): the left trackpad's touch makes it.
        "index_controller" => vec![vec![("pad_dpad_shift", format!("{L}trackpad/touch"))]],
        _ => Vec::new(),
    }
}

/// The pad's actions, created in the viewer's action set.
pub struct PadActions<'s> {
    actions: Vec<(&'static str, Control, Action<'s>)>,
    /// The layer was used during the current hold ([`layer`]).
    layer_used: Cell<bool>,
    /// The d-pad directions that stay held once pushed through the layer (`--dpad-hold`).
    hold: u16,
    latch: Cell<Latch>,
    proxy: Proxy,
}

impl<'s> PadActions<'s> {
    /// `hold`: the d-pad directions that stay held once pushed through the layer ([`Latch`]);
    /// `proxy`: the controller the ones at hand act like.
    pub fn create(set: &'s ActionSet<'_>, hold: u16, proxy: Proxy) -> Result<Self> {
        let mut actions = Vec::with_capacity(CONTROLS.len());
        for (name, label, control) in CONTROLS {
            let action = match control {
                Control::LeftStick | Control::RightStick => set.vector2_action(name, label)?,
                Control::LeftTrigger | Control::RightTrigger => set.float_action(name, label)?,
                Control::Button(_) | Control::Layer | Control::Shift | Control::Rest => set.boolean_action(name, label)?,
            };
            actions.push((name, control, action));
        }
        Ok(Self { actions, layer_used: Cell::new(false), hold, latch: Cell::new(Latch::default()), proxy })
    }

    /// The action for a binding key.
    pub fn action(&self, name: &str) -> Option<&Action<'s>> {
        self.actions.iter().find(|(n, _, _)| *n == name).map(|(_, _, a)| a)
    }

    /// The pad as the controllers hold it now (after the session's action sync). Unreadable
    /// controls read as released.
    pub fn read(&self, session: &Session<'_>) -> Gamepad {
        let mut pad = Gamepad::default();
        let (mut layer_held, mut shift_held, mut rest_held) = (false, false, false);
        for (_, control, action) in &self.actions {
            match *control {
                Control::Layer => layer_held = session.boolean(action).unwrap_or(false),
                Control::Shift => shift_held = session.boolean(action).unwrap_or(false),
                Control::Rest => rest_held = session.boolean(action).unwrap_or(false),
                Control::Button(bit) => {
                    if session.boolean(action).unwrap_or(false) {
                        pad.buttons |= bit;
                    }
                }
                Control::LeftTrigger => pad.left_trigger = trigger(session.float(action).unwrap_or(0.0)),
                Control::RightTrigger => pad.right_trigger = trigger(session.float(action).unwrap_or(0.0)),
                Control::LeftStick | Control::RightStick => {
                    let v = session.vector2(action).unwrap_or_default();
                    let (x, y) = (stick(v.x), stick(v.y));
                    if matches!(control, Control::LeftStick) {
                        (pad.thumb_lx, pad.thumb_ly) = (x, y);
                    } else {
                        (pad.thumb_rx, pad.thumb_ry) = (x, y);
                    }
                }
            }
        }
        let (proxy_layer, proxy_shift) = proxied(&mut pad, self.proxy, rest_held);
        let (layer_held, shift_held) = (layer_held || proxy_layer, shift_held || proxy_shift);
        let mut used = self.layer_used.get();
        let pushed = layer(&mut pad, layer_held || shift_held, layer_held, &mut used);
        self.layer_used.set(used);
        let mut latch = self.latch.get();
        latch.step(&mut pad, pushed, self.hold, Instant::now());
        self.latch.set(latch);
        pad
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
/// `--dpad-hold` stays held until a face button has been pressed and let go (the game sees the
/// d-pad held through the pick, as a player holding it would), the same direction is pushed again,
/// or [`LATCH_FOR`] passes.
#[derive(Clone, Copy, Default)]
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

/// The d-pad directions named in `--dpad-hold` (`left,right`; `up`, `down` too).
pub fn dpad_directions(list: &str) -> std::result::Result<u16, String> {
    list.split(',').map(str::trim).filter(|d| !d.is_empty()).try_fold(0u16, |bits, d| {
        Ok(bits
            | match d {
                "up" => button::DPAD_UP,
                "down" => button::DPAD_DOWN,
                "left" => button::DPAD_LEFT,
                "right" => button::DPAD_RIGHT,
                other => return Err(format!("--dpad-hold: {other} is not up, down, left or right")),
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_layer_makes_a_dpad_and_back_and_then_lets_go_of_the_bumper() {
        let mut used = false;
        // Held alone (the grip as a bumper): nothing changes.
        let mut pad = Gamepad { buttons: button::LEFT_SHOULDER, thumb_lx: 3000, ..Default::default() };
        layer(&mut pad, true, true, &mut used);
        assert_eq!((pad.buttons, pad.thumb_lx, used), (button::LEFT_SHOULDER, 3000, false));
        // The stick pushed left: d-pad left, no movement, the bumper let go.
        let mut pad = Gamepad { buttons: button::LEFT_SHOULDER, thumb_lx: -30000, thumb_ly: 5000, ..Default::default() };
        layer(&mut pad, true, true, &mut used);
        assert_eq!((pad.buttons, pad.thumb_lx, pad.thumb_ly, used), (button::DPAD_LEFT, 0, 0, true));
        // Still the same hold: the bumper stays let go.
        let mut pad = Gamepad { buttons: button::LEFT_SHOULDER | button::START, thumb_ly: -30000, ..Default::default() };
        layer(&mut pad, true, true, &mut used);
        assert_eq!(pad.buttons, button::DPAD_DOWN | button::BACK);
        // With the thumb on the left thumbrest, the right stick: d-pad right, no turning.
        let mut pad = Gamepad { thumb_rx: 30000, thumb_lx: 2000, ..Default::default() };
        layer(&mut pad, true, true, &mut used);
        assert_eq!((pad.buttons, pad.thumb_rx, pad.thumb_lx), (button::DPAD_RIGHT, 0, 2000));
        // Let go of the layer: everything as it was, Start is Start again.
        let mut pad = Gamepad { buttons: button::LEFT_SHOULDER | button::START, thumb_ly: -30000, ..Default::default() };
        layer(&mut pad, false, true, &mut used);
        assert_eq!((pad.buttons, pad.thumb_ly, used), (button::LEFT_SHOULDER | button::START, -30000, false));
    }

    #[test]
    fn a_proxy_hides_what_the_controller_lacks_and_the_grip_stands_in() {
        let all = DPAD | button::BACK | button::START | button::A | button::LEFT_THUMB;
        // As Quest controllers: no d-pad, no view button; the grip is the thumbrest (the layer).
        let mut pad = Gamepad { buttons: all, ..Default::default() };
        assert_eq!(proxied(&mut pad, Proxy::Touch, true), (true, false));
        assert_eq!(pad.buttons, button::START | button::A);
        // As Index controllers: the view button stays; the grip is the trackpad touch (the shift).
        let mut pad = Gamepad { buttons: all, ..Default::default() };
        assert_eq!(proxied(&mut pad, Proxy::Index, true), (false, true));
        assert_eq!(pad.buttons, button::BACK | button::START | button::A);
        // Not standing in: the stick click stays; no proxy: nothing changes.
        let mut pad = Gamepad { buttons: all, ..Default::default() };
        assert_eq!(proxied(&mut pad, Proxy::Index, false), (false, false));
        assert!(pad.buttons & button::LEFT_THUMB != 0);
        let mut pad = Gamepad { buttons: all, ..Default::default() };
        assert_eq!((proxied(&mut pad, Proxy::None, true), pad.buttons), ((false, false), all));
        // The shift makes the d-pad but leaves Start as Start.
        let mut used = false;
        let mut pad = Gamepad { buttons: button::START, thumb_rx: -30000, ..Default::default() };
        layer(&mut pad, true, false, &mut used);
        assert_eq!(pad.buttons, button::START | button::DPAD_LEFT);
        assert_eq!(Proxy::parse("quest"), Ok(Proxy::Touch));
        assert!(Proxy::parse("vive").is_err());
    }

    #[test]
    fn a_held_choice_stays_open_through_the_pick() {
        let hold = dpad_directions("left, right").unwrap();
        let t = Instant::now();
        let ms = |n: u64| t + Duration::from_millis(n);
        let mut latch = Latch::default();
        let read = |latch: &mut Latch, buttons: u16, pushed: u16, at: Instant| {
            let mut pad = Gamepad { buttons, ..Default::default() };
            latch.step(&mut pad, pushed, hold, at);
            pad.buttons
        };
        // Pushed left (the tools): held, and stays held once the stick comes back.
        assert_eq!(read(&mut latch, button::DPAD_LEFT, button::DPAD_LEFT, ms(0)), button::DPAD_LEFT);
        assert_eq!(read(&mut latch, 0, 0, ms(100)), button::DPAD_LEFT);
        // X picks a tool with the d-pad still held; letting go of X lets go of the d-pad after.
        assert_eq!(read(&mut latch, button::X, 0, ms(200)), button::DPAD_LEFT | button::X);
        assert_eq!(read(&mut latch, 0, 0, ms(300)), button::DPAD_LEFT);
        assert_eq!(read(&mut latch, 0, 0, ms(400)), 0);
        // Up is not a held choice: only while pushed.
        assert_eq!(read(&mut latch, button::DPAD_UP, button::DPAD_UP, ms(500)), button::DPAD_UP);
        assert_eq!(read(&mut latch, 0, 0, ms(600)), 0);
        // Pushed right, then right again: let go. And on its own after a while.
        read(&mut latch, button::DPAD_RIGHT, button::DPAD_RIGHT, ms(700));
        read(&mut latch, 0, 0, ms(800));
        assert_eq!(read(&mut latch, button::DPAD_RIGHT, button::DPAD_RIGHT, ms(900)), button::DPAD_RIGHT, "the push itself still reads");
        assert_eq!(read(&mut latch, 0, 0, ms(1000)), 0);
        read(&mut latch, button::DPAD_RIGHT, button::DPAD_RIGHT, ms(1100));
        assert_eq!(read(&mut latch, 0, 0, ms(1200)), button::DPAD_RIGHT);
        assert_eq!(read(&mut latch, 0, 0, ms(1100 + 5000)), 0);
        assert!(dpad_directions("sideways").is_err());
    }

    #[test]
    fn every_binding_names_a_control_and_a_known_profile_has_some() {
        for profile in ["valve/frame_controller", "oculus/touch_controller", "valve/index_controller", "microsoft/motion_controller", "htc/vive_controller", "khr/simple_controller"] {
            let bindings = default_bindings(&format!("/interaction_profiles/{profile}"));
            assert!(!bindings.is_empty(), "{profile}");
            for (name, path) in &bindings {
                assert!(CONTROLS.iter().any(|(n, _, _)| n == name), "{profile}: {name}");
                assert!(path.starts_with("/user/hand/"), "{path}");
            }
            let mut names: Vec<_> = bindings.iter().map(|(n, _)| *n).collect();
            names.sort_unstable();
            names.dedup();
            assert_eq!(names.len(), bindings.len(), "{profile}: one binding per control");
        }
        assert!(default_bindings("/interaction_profiles/unknown/thing").is_empty());
        for profile in ["valve/frame_controller", "oculus/touch_controller", "valve/index_controller"] {
            let extras = extra_bindings(&format!("/interaction_profiles/{profile}"));
            assert!(!extras.is_empty(), "{profile}");
            for (name, path) in extras.iter().flatten() {
                assert!(CONTROLS.iter().any(|(n, _, _)| n == name) && path.starts_with("/user/hand/left/input/"), "{name} {path}");
            }
        }
    }
}
