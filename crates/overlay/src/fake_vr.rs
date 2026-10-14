//! The Fake VR panel: in the 2D window, the head and the controllers the run gets instead of a
//! headset's, so whatever needs them (the hand rig, guns, melee, head look) runs without one. The
//! head turns and tilts; the hands take a pose (held forward, aiming, at the sides) and can be
//! moved from it, and turn with the head as a body would. In the headset it only says the real
//! head and controllers are used.

use crate::menu::panel::{DIM, TEXT};
use egui::{DragValue, RichText};

#[cfg(test)]
mod tests;

/// One controller's aim: where it is from the head (metres: x right, y up, z back, before the
/// head's turn), and where it points (degrees: yaw left of ahead, pitch up).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hand {
    pub at: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
}

/// The hands' poses to start from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    Off,
    Forward,
    Aiming,
    AtSides,
}

impl Preset {
    pub const ALL: [Preset; 4] = [Preset::Off, Preset::Forward, Preset::Aiming, Preset::AtSides];

    pub const fn label(self) -> &'static str {
        match self {
            Preset::Off => "Off",
            Preset::Forward => "Forward",
            Preset::Aiming => "Aiming",
            Preset::AtSides => "At sides",
        }
    }

    /// Left then right.
    pub const fn hands(self) -> Option<[Hand; 2]> {
        match self {
            Preset::Off => None,
            Preset::Forward => Some(pair(Hand { at: [0.2, -0.35, -0.35], yaw: 0.0, pitch: -20.0 })),
            Preset::Aiming => Some([Hand { at: [-0.06, -0.2, -0.5], yaw: 0.0, pitch: 0.0 }, Hand { at: [0.12, -0.15, -0.42], yaw: 0.0, pitch: 0.0 }]),
            Preset::AtSides => Some(pair(Hand { at: [0.25, -0.65, -0.05], yaw: 0.0, pitch: -80.0 })),
        }
    }
}

/// The left hand mirroring `right`, then `right`.
const fn pair(right: Hand) -> [Hand; 2] {
    [Hand { at: [-right.at[0], right.at[1], right.at[2]], yaw: 0.0 - right.yaw, pitch: right.pitch }, right]
}

/// The fake head and controllers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FakeVr {
    /// The 2D window's (the headset's own head and controllers otherwise).
    pub desktop: bool,
    /// The head's turn and tilt (degrees).
    pub head: [f32; 2],
    pub preset: Preset,
    /// Left then right, none while the hands are off.
    pub hands: Option<[Hand; 2]>,
}

impl FakeVr {
    /// The headset's: nothing faked.
    pub const HEADSET: FakeVr = FakeVr { desktop: false, head: [0.0; 2], preset: Preset::Off, hands: None };

    /// The 2D window's at its start: the head straight ahead, the hands held forward.
    pub fn desktop() -> Self {
        FakeVr { desktop: true, head: [0.0; 2], preset: Preset::Forward, hands: Preset::Forward.hands() }
    }

    /// Each controller's aim in tracking space (position, orientation), left then right: placed
    /// from the head and turned with its turn (not its tilt).
    pub fn controllers(&self) -> [Option<([f32; 3], [f32; 4])>; 2] {
        let Some(hands) = self.hands else { return [None, None] };
        let body = turn(self.head[0], 0.0);
        hands.map(|hand| Some((monaka_core::math::rotate(body, hand.at), turn(self.head[0] + hand.yaw, hand.pitch))))
    }
}

/// A turn of `yaw` degrees about up, then `pitch` about right, as a quaternion (x, y, z, w).
pub fn turn(yaw: f32, pitch: f32) -> [f32; 4] {
    let (sy, cy) = (yaw.to_radians() * 0.5).sin_cos();
    let (sp, cp) = (pitch.to_radians() * 0.5).sin_cos();
    [cy * sp, sy * cp, -sy * sp, cy * cp]
}

/// The page; the fake head and controllers as changed on it.
pub fn layout(ui: &mut egui::Ui, now: &FakeVr) -> Option<FakeVr> {
    if !now.desktop {
        ui.label(RichText::new("In the 2D window only: the headset gives your real head and controllers.").size(13.0).color(DIM));
        return None;
    }
    let mut next = *now;
    ui.label(RichText::new("Head").size(14.0).color(TEXT));
    ui.horizontal(|ui| {
        ui.label(RichText::new("turn").color(DIM));
        ui.add(DragValue::new(&mut next.head[0]).range(-180.0..=180.0).speed(1.0).suffix("°"));
        ui.label(RichText::new("tilt").color(DIM));
        ui.add(DragValue::new(&mut next.head[1]).range(-80.0..=80.0).speed(1.0).suffix("°"));
        if ui.button("Ahead").clicked() {
            next.head = [0.0; 2];
        }
    });
    ui.add_space(6.0);
    ui.label(RichText::new("Hands").size(14.0).color(TEXT));
    ui.horizontal(|ui| {
        for preset in Preset::ALL {
            if ui.selectable_label(now.preset == preset, preset.label()).clicked() {
                next.preset = preset;
                next.hands = preset.hands();
            }
        }
    });
    if let Some(hands) = next.hands.as_mut() {
        for (hand, name) in hands.iter_mut().zip(["Left", "Right"]) {
            ui.horizontal(|ui| {
                ui.label(RichText::new(name).color(DIM));
                for (value, label) in hand.at.iter_mut().zip(["x", "y", "z"]) {
                    ui.add(DragValue::new(value).range(-1.5..=1.5).speed(0.01).max_decimals(2).prefix(format!("{label} ")));
                }
                ui.add(DragValue::new(&mut hand.yaw).range(-180.0..=180.0).speed(1.0).prefix("yaw ").suffix("°"));
                ui.add(DragValue::new(&mut hand.pitch).range(-90.0..=90.0).speed(1.0).prefix("pitch ").suffix("°"));
            });
        }
        ui.label(RichText::new("metres from the head: x right, y up, z back (drag a number)").size(12.0).color(DIM));
    }
    (next != *now).then_some(next)
}
