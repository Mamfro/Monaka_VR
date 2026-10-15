use super::*;
use crate::TEST_PRESSES;
use crate::presses::pressed;
use monaka_producer::game::Adapter;
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

struct Pressing;
impl Game for Pressing {
    const MANIFESTS: &'static [monaka_contract::manifest::Manifest] = &[];
    const ADAPTER: Adapter<Self> = Adapter { components: &[] };
    type Build = ();
    type Frame = ();
    fn prepare(_: &'static monaka_producer::Session, _: &'static monaka_contract::manifest::Manifest, _: &monaka_core::options::Options) -> Result<(), monaka_producer::Rejection> {
        Ok(())
    }
}

/// What the game's own way was asked: (action index + 1) * 10 + the ask (1 tap, 2 hold, 3 let
/// go, 4 follow).
static ASKED: AtomicU32 = AtomicU32::new(0);

impl Actions for Pressing {
    fn binding(action: Action) -> Binding {
        match action {
            Action::Attack => Binding::Pad(Gamepad { right_trigger: 255, ..Gamepad::default() }),
            Action::Crouch | Action::Pick => Binding::Engine,
            Action::Flashlight => Binding::Pad(buttons(button::DPAD_UP)),
            _ => Binding::None,
        }
    }
    fn engine(action: Action, ask: Ask) -> bool {
        let code = match ask {
            Ask::Tap { .. } => 1,
            Ask::Hold => 2,
            Ask::LetGo => 3,
            Ask::Follow => 4,
        };
        ASKED.store((index(action) as u32 + 1) * 10 + code, Relaxed);
        true
    }
}

#[test]
fn the_standard_layout_answers_until_a_game_installs_its_own() {
    let _serial = TEST_PRESSES.locked();
    release();
    assert_eq!(binding(Action::Attack), Binding::Pad(Gamepad { right_trigger: 255, ..Gamepad::default() }));
    assert_eq!(binding(Action::Crouch), Binding::Toggle(buttons(button::B)));
    assert!(!can(Action::Heal), "the standard layout has no heal");
    install::<Pressing>();
    assert_eq!(binding(Action::Crouch), Binding::Engine);
    assert!(can(Action::Flashlight) && !can(Action::Jump));
    release();
    assert_eq!(binding(Action::Crouch), Binding::Toggle(buttons(button::B)));
}

#[test]
fn a_press_on_the_pad_is_the_bound_controls_under_the_actions_source() {
    let _serial = TEST_PRESSES.locked();
    release();
    // The attack a swing pressed before the provider: the right trigger, as its own source.
    assert!(press(Action::Attack, 60_000));
    assert_eq!(pressed(), Gamepad { right_trigger: 255, ..Gamepad::default() });
    crate::let_go(Action::Attack.source());
    install::<Pressing>();
    assert!(press(Action::Attack, 60_000));
    assert_eq!(pressed(), Gamepad { right_trigger: 255, ..Gamepad::default() }, "the game's table binds it alike");
    assert!(!press(Action::Jump, 60_000), "not bound: nothing pressed");
    assert!(press(Action::Crouch, 300));
    assert_eq!(ASKED.load(Relaxed), (index(Action::Crouch) as u32 + 1) * 10 + 1, "the game's own way tapped");
    crate::let_go(Action::Attack.source());
    assert_eq!(pressed(), Gamepad::default());
    release();
}

#[test]
fn a_hold_stays_until_let_go_and_a_toggle_taps_only_on_a_change() {
    let _serial = TEST_PRESSES.locked();
    release();
    install::<Pressing>();
    assert!(hold(Action::Flashlight, true));
    assert_eq!(pressed().buttons, button::DPAD_UP);
    assert!(held(Action::Flashlight));
    assert!(hold(Action::Flashlight, false));
    assert_eq!(pressed(), Gamepad::default());
    assert!(hold(Action::Crouch, true));
    assert_eq!(ASKED.load(Relaxed) % 10, 2, "held the game's own way");
    release();
    assert_eq!(ASKED.load(Relaxed) % 10, 3, "let go at the release");
    assert!(!held(Action::Crouch));
    // The standard crouch is a toggle: one tap going down, none while down, one coming up.
    assert!(hold(Action::Crouch, true));
    assert_eq!(pressed().buttons, button::B);
    crate::let_go(Action::Crouch.source());
    assert!(hold(Action::Crouch, true));
    assert_eq!(pressed(), Gamepad::default(), "already down: no second tap");
    assert!(hold(Action::Crouch, false));
    assert_eq!(pressed().buttons, button::B, "coming up taps it again");
    crate::let_go(Action::Crouch.source());
    assert!(!hold(Action::Heal, true), "not bound");
    assert!(hold(Action::Heal, false), "letting go of what is not held is done");
}

/// A source at a quarter turn clockwise (an amount of a half), ready, and the freshness of its
/// latest read (1 fresh, 2 not).
static READ_FRESH: AtomicU32 = AtomicU32::new(0);
const QUARTER: Source = Source {
    ready: |_| true,
    value: |action, reading| {
        READ_FRESH.store(if reading.fresh { 1 } else { 2 }, Relaxed);
        Some(if action == Action::Pick { std::f32::consts::FRAC_PI_2 } else { -0.5 })
    },
};

#[test]
fn an_analog_action_on_the_pad_deflects_its_stick_at_each_read() {
    let _serial = TEST_PRESSES.locked();
    release();
    assert_eq!((binding(Action::Pick), binding(Action::Turn)), (Binding::Stick(Stick::Left), Binding::Stick(Stick::Right)));
    assert!(!press(Action::Pick, 100) && !hold(Action::Turn, true), "followed, never pressed");
    assert_eq!(pressed(), Gamepad::default(), "nothing followed: the sticks are the player's");
    assert!(follow(Action::Pick, Some(QUARTER)) && follow(Action::Turn, Some(QUARTER)));
    let pad = pressed();
    assert_eq!((pad.thumb_lx, pad.thumb_ly), (32767, 0), "the pick a quarter turn clockwise: the left stick right");
    assert_eq!((pad.thumb_rx, pad.thumb_ry), (-16383, 0), "the turn half left");
    assert_eq!(READ_FRESH.load(Relaxed), 1, "the first read is fresh");
    pressed();
    assert_eq!(READ_FRESH.load(Relaxed), 2);
    assert!(held(Action::Pick));
    release();
    assert!(!held(Action::Pick) && !ready(Action::Pick));
    assert_eq!(pressed(), Gamepad::default(), "let go at the release");
}

#[test]
fn the_games_own_way_is_told_to_follow_and_reads_the_source() {
    let _serial = TEST_PRESSES.locked();
    release();
    install::<Pressing>();
    assert!(!follow(Action::Turn, Some(QUARTER)), "not bound");
    assert!(follow(Action::Pick, Some(QUARTER)));
    assert_eq!(ASKED.load(Relaxed), (index(Action::Pick) as u32 + 1) * 10 + 4);
    assert_eq!(pressed(), Gamepad::default(), "the game's own way: no stick");
    assert!(ready(Action::Pick));
    assert_eq!(value(Action::Pick, Reading { fresh: true }), Some(std::f32::consts::FRAC_PI_2));
    assert_eq!(READ_FRESH.load(Relaxed), 1, "the game says when a read is fresh");
    assert!(!follow(Action::Crouch, Some(QUARTER)), "not an analog action");
    let_go_all();
    assert!(held(Action::Pick) && ready(Action::Pick), "a stop keeps what is followed");
    release();
    assert_eq!(ASKED.load(Relaxed), (index(Action::Pick) as u32 + 1) * 10 + 3, "let go at the release");
    assert_eq!(value(Action::Pick, Reading::default()), None);
}
