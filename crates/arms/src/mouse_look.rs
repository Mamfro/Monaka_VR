//! Head aim through the game's mouse as a ready-made component: the shared mouse steering
//! ([`crate::mouse_aim`]) for a game that keeps no look angles a producer can reach. A game lists
//! `monaka_arms::mouse_look::look()` among its components (with its own `aim` choices and what it
//! works on top of, by struct update), says once how the counts reach it ([`AimsByMouse::AIM`]),
//! and calls [`crate::mouse_aim::on_build`] from its view hook with the game camera's angles.
//!
//! Its options (not shown by the launcher): `mouse_scale_x` and `mouse_scale_y` (degrees per
//! count, both needed: no measuring at the start), `native_stick=1` (the game turns with the right
//! stick itself), and in a game whose left hand aims powers `power_aim` (default: with motion
//! controls).

use crate::mouse_aim::{self, MouseAim};
use monaka_core::feature::Feature;
use monaka_core::options::Options;
use monaka_producer::game::{Component, Game, Lack, On, Run, Start};
use monaka_producer::log;
use std::sync::Mutex;
use monaka_core::sync::Locked;

/// How the aim's mouse counts reach the game.
pub enum Delivery<G: Game> {
    /// As DirectInput mouse movement ([`monaka_pad::mouse::install`]).
    DirectInput,
    /// Through the game's own input, by hooks this installs (they hand the counts over with
    /// [`monaka_pad::mouse::deliver_with`]).
    Game(fn(&mut Start<'_, G>) -> Result<(), Lack>),
}

/// What a game's head aim through the mouse needs from it.
pub struct Aim<G: Game> {
    pub delivery: Delivery<G>,
    /// Degrees per count (yaw, pitch) at the game's default sensitivity: steering starts from it.
    pub usual_scale: [f32; 2],
    /// The left hand aims its power while the left trigger is held (`power_aim`).
    pub powers: bool,
    /// Whether the arms are on the controllers in this run (asked once the hooks are in).
    pub rig: fn() -> bool,
    /// Whether a weapon is in the left hand, for the power aim's log.
    pub left_weapon: Option<fn() -> bool>,
}

/// A game whose look is steered through its mouse.
pub trait AimsByMouse: Game {
    const AIM: Aim<Self>;
}

/// The head aim component for game `G`: on with head aim (`aim=hand_rig`, `head` or
/// `controller`).
pub const fn look<G: AimsByMouse>() -> Component<G> {
    Component {
        name: "look",
        features: &[Feature::Look],
        needs_channel: true,
        settings: |options, _| {
            read(options, false);
        },
        on: on::<G>,
        install: install::<G>,
        arm: arm::<G>,
        give_back,
        release: crate::controllers::close,
        summary: Some(mouse_aim::summary),
        ..Component::NONE
    }
}

/// This run's settings, from its install to its start.
struct Settings {
    scale: Option<[f32; 2]>,
    stick_speed: [f32; 2],
    native_stick: bool,
    power_aim: bool,
    pointer: Option<usize>,
}

static SETTINGS: Mutex<Option<Settings>> = Mutex::new(None);

/// The mouse scale (both axes given, each 0.0005 to 2 degrees per count in size), the native
/// stick and the power aim (on unless set when `powers_default`).
fn read(options: &Options, powers_default: bool) -> (Option<[f32; 2]>, bool, bool) {
    let scale = |key: &str| options.value::<f32>(key).filter(|v| v.is_finite() && (0.0005..=2.0).contains(&v.abs()));
    let scale = match (scale("mouse_scale_x"), scale("mouse_scale_y")) {
        (Some(x), Some(y)) => Some([x, y]),
        _ => None,
    };
    (scale, options.switch("native_stick", false), options.switch("power_aim", powers_default))
}

fn on<G: AimsByMouse>(run: &Run<'_, G>) -> On {
    On::when(run.shared.aim.head, "aim is mouse and stick only")
}

fn install<G: AimsByMouse>(start: &mut Start<'_, G>) -> Result<(), Lack> {
    match G::AIM.delivery {
        Delivery::DirectInput => {
            monaka_pad::mouse::install(start.hooks)?;
        }
        Delivery::Game(install) => install(start)?,
    }
    let aim = start.shared.aim;
    let (scale, native_stick, power_aim) = read(start.options, aim.rig);
    let power_aim = G::AIM.powers && power_aim;
    // A controller aims all the time (`aim=controller`); with the arms on the controllers it
    // aims only as the power hand.
    let pointer = (aim.hand && !aim.rig).then_some(aim.side);
    if power_aim || pointer.is_some() {
        crate::controllers::open(start.channel()?);
    }
    *SETTINGS.locked() = Some(Settings { scale, stick_speed: start.shared.stick_speed, native_stick, power_aim, pointer });
    Ok(())
}

/// Starts steering. Keep the mouse still for the first two seconds: the scale is measured then
/// unless known.
fn arm<G: AimsByMouse>() {
    let Some(settings) = SETTINGS.locked().take() else { return };
    mouse_aim::start(MouseAim {
        scale: settings.scale,
        usual_scale: Some(G::AIM.usual_scale),
        stick_speed: settings.stick_speed,
        native_stick: settings.native_stick,
        power_aim: settings.power_aim,
        rig: (G::AIM.rig)(),
        pointer: settings.pointer,
        left_weapon: G::AIM.left_weapon,
    });
}

/// The aim's share goes back out of the game's camera while the hooks are still in.
fn give_back() {
    if mouse_aim::AIMING.load(std::sync::atomic::Ordering::Acquire) && !mouse_aim::release(1500) {
        log!("head aim: the game did not read the mouse while stopping; its camera keeps the last aim");
    }
}
