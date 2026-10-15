//! The VR controllers as one Xbox-layout gamepad for the game ([`monaka_core::pad`]). Each control
//! is an OpenXR action named for the gamepad control it drives, so the runtime's binding screen
//! (SteamVR's controller bindings) can remap any controller; the defaults below put every
//! controller's buttons where an Xbox pad has them: thumbsticks to sticks, triggers to triggers,
//! grips to bumpers, face buttons to A/B (right) and X/Y (left), menu to Start.
//!
//! What the pad goes through once read (the controller proxy, `controller_proxy`; the d-pad
//! layer; the held d-pad choices, `dpad_menus`) is [`monaka_core::pad_layer`], which the harness applies too.
//! The layer's controls are actions of their own here: `pad_dpad_layer` (the d-pad and Back, on
//! the left thumbrest by default), `pad_dpad_shift` (the d-pad only: Index controllers' left
//! trackpad touch, since pressing that trackpad is Start) and the proxy's stand-in.

use monaka_core::pad::{stick, trigger};
use monaka_contract::{gamepad::Gamepad, gamepad::button};
pub use monaka_core::pad_layer::{Proxy, dpad_directions};
use monaka_core::pad_layer::{Held, Layers};
use monaka_openxr::{Action, ActionSet, Result, Session};
use std::cell::Cell;
use std::time::Instant;

/// What an action drives on the pad.
#[derive(Clone, Copy)]
enum Control {
    Button(u16),
    LeftTrigger,
    RightTrigger,
    LeftStick,
    RightStick,
    /// Held: the d-pad layer: the d-pad and Back.
    Layer,
    /// Held: the d-pad layer without Back (a controller with its own view button).
    Shift,
    /// The controller proxy's stand-in for the emulated controller's layer input.
    Rest,
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
    /// The proxy (which [`PadActions::set_proxy`] changes while VR runs), the layer and the latch.
    layers: Cell<Layers>,
}

impl<'s> PadActions<'s> {
    /// `hold`: the d-pad directions that stay held once pushed through the layer; `proxy`: the
    /// controller the ones at hand act like.
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
        Ok(Self { actions, layers: Cell::new(Layers::new(hold, proxy)) })
    }

    /// The d-pad directions held and the proxy of another game, from the next read (a game linked
    /// into the home's viewer).
    pub fn set_layers(&self, hold: u16, proxy: Proxy) {
        self.layers.set(Layers::new(hold, proxy));
    }

    /// The controllers act like `proxy` from the next read (set in the headset's home).
    pub fn set_proxy(&self, proxy: Proxy) {
        let mut layers = self.layers.get();
        layers.proxy = proxy;
        self.layers.set(layers);
    }

    /// The action for a binding key.
    pub fn action(&self, name: &str) -> Option<&Action<'s>> {
        self.actions.iter().find(|(n, _, _)| *n == name).map(|(_, _, a)| a)
    }

    /// The pad as the controllers hold it now (after the session's action sync). Unreadable
    /// controls read as released.
    pub fn read(&self, session: &Session<'_>) -> Gamepad {
        let mut pad = Gamepad::default();
        let mut held = Held::default();
        for (_, control, action) in &self.actions {
            match *control {
                Control::Layer => held.layer = session.boolean(action).unwrap_or(false),
                Control::Shift => held.shift = session.boolean(action).unwrap_or(false),
                Control::Rest => held.rest = session.boolean(action).unwrap_or(false),
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
        let mut layers = self.layers.get();
        layers.apply(&mut pad, held, Instant::now());
        self.layers.set(layers);
        pad
    }
}

#[cfg(test)]
mod tests;
