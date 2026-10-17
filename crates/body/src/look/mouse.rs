//! The look delivered as the game's mouse ([`super::Delivery::Mouse`]), for a game that keeps no
//! look angles a producer can reach: the head's turn goes in as mouse counts ([`steering`]). The
//! game says how the counts reach it ([`Mouse::counts`]), and its eye site calls [`on_build`] with
//! the game camera's angles at each view build.
//!
//! The game can stay on its gamepad meanwhile (its button prompts and aim assist the pad's) though
//! the aim reaches it as mouse movement. It says where it picks its device in use from the frame's
//! input and which byte keeps it ([`Mouse::pad`]), and the pad is kept one of two ways:
//!
//! - the counts go into the game's mouse movement for the frame right after that pick
//!   ([`Counts::AfterPick`]), so they never make keyboard and mouse the device in use, and the
//!   game's look reads get that movement while the pad is in use ([`pad_look`]);
//! - or, for a game that reads the mouse with its pad in use too but picks whichever device moved
//!   last, the pick is put back on the pad after each frame's ([`keep_pad`], its own component:
//!   on unless `pin_gamepad=0`, whatever aims).
//!
//! Its options (not shown by the launcher): `mouse_scale_x` and `mouse_scale_y` (degrees per
//! count, both needed: no measuring at the start), `native_stick=1` (the game turns with the right
//! stick itself), and in a game whose left hand aims powers `power_aim` (default: with motion
//! controls).

mod steering;

use super::{Delivery, Looks, Share};
use monaka_core::feature::Feature;
use monaka_core::options::Options;
use monaka_core::pad::StickTurn;
use monaka_contract::protocol::HeadPose;
use monaka_core::sync::Locked;
use monaka_hook::module::Module;
use monaka_hook::{InFlight, Original, Site, mem};
use monaka_pad::mouse;
use monaka_producer::game::{Component, Game, Lack, Need, On, Start};
use monaka_producer::log;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering::*};
use steering::MouseAim;

/// How a game takes the turn as its mouse.
pub struct Mouse<G: Game> {
    pub counts: Counts<G>,
    /// Where the game picks its device in use, to keep it on its pad; `None`: the game picks as
    /// it likes (needed with [`Counts::AfterPick`]).
    pub pad: Option<Pad<G>>,
    /// Degrees per count (yaw, pitch) at the game's default sensitivity: steering starts from it.
    pub usual_scale: [f32; 2],
    /// The left hand aims its power while the left trigger is held (`power_aim`).
    pub powers: bool,
    /// Whether a weapon is in the left hand, for the power aim's log.
    pub left_weapon: Option<fn() -> bool>,
}

/// How the aim's mouse counts reach the game.
pub enum Counts<G: Game> {
    /// As DirectInput mouse movement ([`monaka_pad::mouse::install`]).
    DirectInput,
    /// Into the game's own mouse movement for the frame, right after it picks its device in use
    /// ([`Mouse::pad`]), so they never make keyboard and mouse the device in use. `frame_mouse`:
    /// where that movement is (x then y, i32 counts, x right, y down), from the pick's object,
    /// while the game has one. `look_reads`: hooks the game's look reads, which add the frame's
    /// mouse movement while the pad is in use ([`pad_look`]).
    AfterPick { frame_mouse: fn(usize) -> Option<usize>, look_reads: fn(&mut Start<'_, G>) -> Result<(), Lack> },
}

/// A game's pick of its device in use: once a frame, from the frame's input, on the object that
/// keeps the choice.
pub type PickFn = unsafe extern "system" fn(usize);

/// Where a game picks its device in use, and where it keeps it: what keeping it on its pad needs.
pub struct Pad<G: Game> {
    /// The pick ([`PickFn`]): its module and site in this build.
    pub pick: fn(&G::Build) -> (&Module, &Site<PickFn>),
    /// The device in use: a byte in the pick's object.
    pub flag: DeviceFlag,
    /// After each pick that took keyboard and mouse the flag is put back on the pad
    /// ([`keep_pad`]), while the game has a pad (asked of the pick's object). `None`: the pick
    /// stands.
    pub put_back: Option<fn(usize) -> bool>,
}

/// The byte that says which device a game has in use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceFlag {
    /// Its offset in the pick's object.
    pub offset: usize,
    /// Its value with the pad in use, and with keyboard and mouse in use.
    pub pad: u8,
    pub mouse: u8,
}

/// Game `G`'s mouse delivery, where it has one.
fn facts<G: Looks>() -> Option<Mouse<G>> {
    match G::DELIVERY {
        Delivery::Mouse(mouse) => Some(mouse),
        _ => None,
    }
}

/// Game `G`'s device pick, where its look is by the mouse and it says one.
fn pad<G: Looks>() -> Option<Pad<G>> {
    facts::<G>()?.pad
}

/// The look component for a game whose turn reaches it as its mouse: on with head aim
/// (`aim=hand_rig`, `head` or `controller`).
pub(super) const fn look<G: Looks>() -> Component<G> {
    if let Delivery::Mouse(Mouse { counts: Counts::AfterPick { .. }, pad: None, .. }) = G::DELIVERY {
        panic!("the counts delivered after the device pick need the pick (Mouse::pad)");
    }
    Component {
        name: "look",
        features: &[Feature::Look],
        reports: G::REPORTS,
        needs_channel: true,
        settings: |options, _| {
            read(options, false);
        },
        on: |run| On::when(run.shared.aim.head, "aim is mouse and stick only"),
        install: install::<G>,
        arm: arm::<G>,
        give_back,
        release: super::release::<G>,
        summary: Some(summary::<G>),
        ..Component::NONE
    }
}

/// The game kept on its pad by putting its pick back ([`Pad::put_back`]) for game `G`: on unless
/// `pin_gamepad=0`, whatever aims. A game lists it after its gamepad (`needs`, by struct update).
pub const fn keep_pad<G: Looks>() -> Component<G> {
    match G::DELIVERY {
        Delivery::Mouse(Mouse { pad: Some(Pad { put_back: Some(_), .. }), counts: Counts::DirectInput, .. }) => {}
        Delivery::Mouse(Mouse { pad: Some(Pad { put_back: Some(_), .. }), .. }) => panic!("with the counts delivered after the pick, the look hooks the pick"),
        _ => panic!("keep_pad puts the pick back (Pad::put_back)"),
    }
    Component {
        needs: &[Need::Feature(Feature::Gamepad)],
        name: "keep pad",
        on: |run| On::when(run.options.switch("pin_gamepad", true), "the game switches between pad and mouse"),
        install: hook_pick::<G>,
        arm: || KEEPING.store(true, Release),
        quiet: || KEEPING.store(false, Release),
        summary: Some(keep_summary),
        ..Component::NONE
    }
}

/// This run's settings, from its install to its start.
struct Settings {
    scale: Option<[f32; 2]>,
    stick_turn: [StickTurn; 2],
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

fn install<G: Looks>(start: &mut Start<'_, G>) -> Result<(), Lack> {
    let Some(facts) = facts::<G>() else { return Err(Lack::Missing("the look is not by the mouse".into())) };
    match facts.counts {
        Counts::DirectInput => {
            monaka_pad::mouse::install(start.hooks)?;
        }
        Counts::AfterPick { look_reads, .. } => {
            hook_pick(start)?;
            look_reads(start)?;
        }
    }
    let aim = start.shared.aim;
    let (scale, native_stick, power_aim) = read(start.options, aim.rig);
    let power_aim = facts.powers && power_aim;
    // A controller aims all the time (`aim=controller`); with the arms on the controllers it
    // aims only as the power hand.
    let pointer = (aim.hand && !aim.rig).then_some(aim.side);
    if power_aim || pointer.is_some() {
        crate::controllers::open(start.channel()?);
    }
    *SETTINGS.locked() = Some(Settings { scale, stick_turn: start.shared.stick_speed.map(StickTurn::smooth), native_stick, power_aim, pointer });
    G::install(start)
}

/// Starts steering. Keep the mouse still for the first two seconds: the scale is measured then
/// unless known.
fn arm<G: Looks>() {
    let Some(settings) = SETTINGS.locked().take() else { return };
    let Some(facts) = facts::<G>() else { return };
    steering::start(MouseAim {
        scale: settings.scale,
        usual_scale: Some(facts.usual_scale),
        stick_turn: settings.stick_turn,
        native_stick: settings.native_stick,
        power_aim: settings.power_aim,
        // The arms on the controllers this run: the hand rig is on (asked once the hooks are in).
        rig: monaka_producer::game::feature_on::<G>(Feature::HandRig),
        pointer: settings.pointer,
        left_weapon: facts.left_weapon,
    });
}

/// The aim's share goes back out of the game's camera while the hooks are still in.
fn give_back() {
    if steering::AIMING.load(Acquire) && !steering::release(1500) {
        log!("head aim: the game did not read the mouse while stopping; its camera keeps the last aim");
    }
}

fn summary<G: Looks>() -> String {
    match facts::<G>().map(|facts| facts.counts) {
        Some(Counts::AfterPick { .. }) => format!("{}\n{}", steering::summary(), pad_look_summary()),
        _ => steering::summary(),
    }
}

/// Once per view build, on the game thread, with the game camera's angles (degrees: yaw left,
/// pitch up) before any change and the head pose the frame is drawn from: advances the calibration
/// or queues the head's counts, and records the head share the game camera carries for the eyes
/// ([`super::baked`], once head aim runs): the camera's angles less the body's, so the view
/// (camera, share swapped for the head) is exactly body plus head.
pub fn on_build(camera: [f32; 2], pose: &HeadPose) {
    let baked = steering::build(camera, pose);
    super::record(None, baked.map(|[yaw, pitch]| Share { yaw, pitch }));
}

// ---- The device pick ----

static PICK_ORIGINAL: Original<PickFn> = Original::new();
/// Set while [`keep_pad`] keeps the pad (from its arm to its quiet).
static KEEPING: AtomicBool = AtomicBool::new(false);
/// Picks of keyboard and mouse put back on the pad.
static PUT_BACK: AtomicU64 = AtomicU64::new(0);
/// The device in use after the last pick (the flag's byte, -1 unseen), and how often it changed.
static DEVICE: AtomicI32 = AtomicI32::new(-1);
static DEVICE_SWITCHES: AtomicU64 = AtomicU64::new(0);
/// Look reads with the pad in use that got the frame's mouse movement added, and those that had
/// some.
static PAD_LOOK_READS: AtomicU64 = AtomicU64::new(0);
static PAD_LOOK_MOVED: AtomicU64 = AtomicU64::new(0);

/// Hooks the game's device pick ([`Mouse::pad`]).
fn hook_pick<G: Looks>(start: &mut Start<'_, G>) -> Result<(), Lack> {
    let Some(pad) = pad::<G>() else { return Ok(()) };
    let (module, site) = (pad.pick)(start.build);
    start.hooks.site(&PICK_ORIGINAL, module, site, picked::<G>)?;
    Ok(())
}

/// The game's pick of its device in use, then [`after_pick`].
unsafe extern "system" fn picked<G: Looks>(object: usize) {
    let flight = InFlight::enter();
    // SAFETY: forwards the game's own call.
    unsafe { PICK_ORIGINAL.get(&flight)(object) };
    after_pick::<G>(object);
}

/// What follows the game's pick of its device in use on `object`: the pick put back on the pad
/// while [`keep_pad`] keeps it, a change of device logged, and with [`Counts::AfterPick`] the
/// aim's queued counts added to the frame's mouse movement (still while the look gives its share
/// back, after its component went quiet).
pub fn after_pick<G: Looks>(object: usize) {
    let Some(facts) = facts::<G>() else { return };
    let Some(pad) = facts.pad else { return };
    let flag = object + pad.flag.offset;
    if let Some(has_pad) = pad.put_back
        && KEEPING.load(Acquire)
        && has_pad(object)
        && mem::read::<u8>(flag) == Some(pad.flag.mouse)
        && mem::write(flag, pad.flag.pad)
        && PUT_BACK.fetch_add(1, Relaxed) == 0
    {
        log!("input device: the game took the mouse and keyboard for the device in use; kept on the pad");
    }
    if let Some(device) = mem::read::<u8>(flag) {
        let before = DEVICE.swap(i32::from(device), Relaxed);
        if before != i32::from(device) && DEVICE_SWITCHES.fetch_add(1, Relaxed) < 200 {
            log!("active device: {}", if device == pad.flag.pad { "pad" } else { "keyboard and mouse" });
        }
    }
    if let Counts::AfterPick { frame_mouse, .. } = facts.counts
        && let Some(at) = frame_mouse(object)
    {
        mouse::deliver_with(|[x, y]| match (mem::read::<i32>(at), mem::read::<i32>(at + 4)) {
            (Some(game_x), Some(game_y)) => mem::write(at, game_x.wrapping_add(x)) && mem::write(at + 4, game_y.wrapping_add(y)),
            _ => false,
        });
    }
}

/// What one of the game's look reads adds while its pad is in use and the aim is fed in, for
/// [`Counts::AfterPick`]'s look-read hooks: `movement()`, the frame's mouse movement on the
/// read's axis as the keyboard-and-mouse path would read it, asked only then; 0 otherwise (with
/// keyboard and mouse in use the game reads the mouse itself). `object` is the pick's.
pub fn pad_look<G: Looks>(object: usize, movement: impl FnOnce() -> Option<f32>) -> f32 {
    if !mouse::ACTIVE.load(Acquire) {
        return 0.0;
    }
    let Some(pad) = pad::<G>() else { return 0.0 };
    if mem::read::<u8>(object + pad.flag.offset) != Some(pad.flag.pad) {
        return 0.0;
    }
    let Some(delta) = movement().filter(|delta| delta.is_finite()) else { return 0.0 };
    PAD_LOOK_READS.fetch_add(1, Relaxed);
    if delta != 0.0 {
        PAD_LOOK_MOVED.fetch_add(1, Relaxed);
    }
    delta
}

/// What the look reads with the pad in use got.
fn pad_look_summary() -> String {
    format!("look reads with the pad active given the mouse movement: {} ({} with movement)", PAD_LOOK_READS.load(Relaxed), PAD_LOOK_MOVED.load(Relaxed))
}

fn keep_summary() -> String {
    format!("input device: put back on the pad {} times", PUT_BACK.load(Relaxed))
}
