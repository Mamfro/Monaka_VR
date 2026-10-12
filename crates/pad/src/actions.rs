//! The actions provider: a game action pressed or held by what it means (attack, crouch, reload,
//! heal, ...), whichever way the game takes it. A ready-made asks for an [`Action`] ([`press`],
//! [`hold`]) and whether the game has it now ([`can`]); the game says once how it takes each one
//! ([`Actions::binding`]):
//!
//! - on the pad (the default method): the controls it is bound to, pressed on the VR pad as their
//!   own source ([`crate::press`], [`crate::hold`]), merged with the controllers and every other
//!   source's presses;
//! - on the pad as a toggle: a tap turns it on and another off (a crouch toggle), so a hold taps
//!   it once going down and once coming up;
//! - the game's own way ([`Actions::engine`]): its input system pressed directly (a button event
//!   on a code no controller sends, an action handed to its listener), for an action the pad does
//!   not reach the way it is wanted (a hold-to-crouch bound only to a toggle);
//! - not at all.
//!
//! An analog action ([`Action::analog`]: a lockpick's angle, a screwdriver's turn) is a value along
//! an axis rather than a press: a ready-made has it follow a source ([`follow`], a [`Source`]) that
//! the game asks at each of its reads ([`value`]). On the pad (the default method) it is a stick
//! deflected as the value says; the game's own way writes the value where the game reads it.
//!
//! A game implements [`Actions`] (often only its binding table; nothing at all takes the
//! [`standard`] layout) and lists `crate::feature::actions()`. Until it is installed, and in a game
//! that does not list it, the standard layout answers.

use crate::{hold as hold_pad, let_go, press as press_pad};
use monaka_core::pad::{Gamepad, button};
use monaka_core::sync::Locked;
use monaka_producer::game::Game;
use monaka_producer::log;
use std::sync::Mutex;

/// A game action, by what it means to the player.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// A light attack: a swing, a punch, a shot.
    Attack,
    HeavyAttack,
    Block,
    /// Crouching, held for as long as the player is down.
    Crouch,
    Jump,
    Reload,
    Heal,
    Flashlight,
    /// Use what is in front of the player: a door, a ladder, a pickup.
    Interact,
    /// Throw the throwable or tool in hand.
    Throw,
    /// Aim down the sights, or hold a bow drawn.
    Aim,
    /// Analog: a lockpick's angle around the lock, in radians clockwise as the player sees it, from
    /// where it was as the lock began.
    Pick,
    /// Analog: a tool turned to work a lock (the screwdriver holding the pick), from -1 (fully
    /// left) to 1 (fully right), from where it was as the lock began.
    Turn,
}

impl Action {
    pub const ALL: [Action; 13] = [
        Action::Attack,
        Action::HeavyAttack,
        Action::Block,
        Action::Crouch,
        Action::Jump,
        Action::Reload,
        Action::Heal,
        Action::Flashlight,
        Action::Interact,
        Action::Throw,
        Action::Aim,
        Action::Pick,
        Action::Turn,
    ];

    /// Whether it is a value along an axis, followed ([`follow`]), rather than pressed or held.
    pub const fn analog(self) -> bool {
        matches!(self, Action::Pick | Action::Turn)
    }

    /// The pad source its presses are kept under ([`crate::press`]): one per action, so pressing one
    /// action never lets go of another.
    pub const fn source(self) -> &'static str {
        match self {
            Action::Attack => "attack",
            Action::HeavyAttack => "heavy attack",
            Action::Block => "block",
            Action::Crouch => "crouch",
            Action::Jump => "jump",
            Action::Reload => "reload",
            Action::Heal => "heal",
            Action::Flashlight => "flashlight",
            Action::Interact => "interact",
            Action::Throw => "throw",
            Action::Aim => "aim",
            Action::Pick => "pick",
            Action::Turn => "turn",
        }
    }
}

/// One of the pad's sticks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stick {
    Left,
    Right,
}

/// How a game takes an action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Binding {
    /// These controls on the pad, held down for as long as the action is.
    Pad(Gamepad),
    /// These controls on the pad, a tap turning the action on and the next one off.
    Toggle(Gamepad),
    /// An analog action on a pad stick: an angle deflects it fully that way (clockwise from up), an
    /// amount sideways that far.
    Stick(Stick),
    /// The game's own way ([`Actions::engine`]).
    Engine,
    /// The game has no such action, or cannot take it now.
    None,
}

/// What is asked of an action the game takes its own way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ask {
    /// Pressed once, let go after about `ms` (the game may keep its own pulse).
    Tap { ms: u64 },
    /// Pressed and kept down.
    Hold,
    /// Let go of a hold, or of an analog action followed.
    LetGo,
    /// An analog action follows its source from now on: the game asks [`value`] at each of its
    /// reads and writes what it answers, until let go.
    Follow,
}

/// What the game says as it reads an analog action ([`value`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Reading {
    /// The first read of the action since the game began taking it afresh (a new lock): a source
    /// measuring from where it was then takes its start now.
    pub fresh: bool,
}

/// Where an analog action's value comes from ([`follow`]), asked as the game reads it.
#[derive(Clone, Copy)]
pub struct Source {
    /// Whether it has a value now (the hand that sets it tracked); a read without one is left to
    /// the game's own input, as if nothing followed it.
    pub ready: fn(Action) -> bool,
    /// Its value now, in the action's terms ([`Action::Pick`], [`Action::Turn`]).
    pub value: fn(Action, Reading) -> Option<f32>,
}

/// A game's actions: how it takes each one. By default every action is where the [`standard`]
/// layout has it, on the pad.
pub trait Actions: Game {
    /// How the game takes `action` now (a binding that depends on the game's own controls being
    /// changed, or on a hook being in, may change during the run).
    fn binding(action: Action) -> Binding {
        standard(action)
    }

    /// Does what `ask` says with `action` the game's own way (for [`Binding::Engine`]); false when
    /// it cannot now. Called from any thread: a game whose input runs on its own thread keeps the
    /// ask for its next input pass.
    fn engine(_action: Action, _ask: Ask) -> bool {
        false
    }
}

/// The layout first-person games most often share on a pad: the right trigger attacks, the left
/// trigger aims, A jumps, B toggles crouching and X reloads; a lockpick's pick on the left stick,
/// the turn on the right. The rest differ too much between games to guess.
pub fn standard(action: Action) -> Binding {
    let pad = |gamepad: Gamepad| Binding::Pad(gamepad);
    match action {
        Action::Attack => pad(Gamepad { right_trigger: 255, ..Gamepad::default() }),
        Action::Aim => pad(Gamepad { left_trigger: 255, ..Gamepad::default() }),
        Action::Jump => pad(buttons(button::A)),
        Action::Crouch => Binding::Toggle(buttons(button::B)),
        Action::Reload => pad(buttons(button::X)),
        Action::Pick => Binding::Stick(Stick::Left),
        Action::Turn => Binding::Stick(Stick::Right),
        _ => Binding::None,
    }
}

/// A pad with only `buttons` down.
pub const fn buttons(buttons: u16) -> Gamepad {
    Gamepad { buttons, left_trigger: 0, right_trigger: 0, thumb_lx: 0, thumb_ly: 0, thumb_rx: 0, thumb_ry: 0 }
}

/// How long a toggle's tap holds its controls (milliseconds): several game updates.
const TOGGLE_TAP_MS: u64 = 100;

/// The installed game's binding table and its own way.
#[derive(Clone, Copy)]
struct Table {
    binding: fn(Action) -> Binding,
    engine: fn(Action, Ask) -> bool,
}

fn no_engine(_: Action, _: Ask) -> bool {
    false
}

const STANDARD: Table = Table { binding: standard, engine: no_engine };

static GAME: Mutex<Option<Table>> = Mutex::new(None);

/// How many actions there are.
const ACTIONS: usize = Action::ALL.len();

/// Which actions are held or followed now ([`hold`], [`follow`]), by [`Action::ALL`]'s order, with
/// the binding each was held by (a toggle's state; what to let go of).
static HELD: Mutex<[Option<Binding>; ACTIONS]> = Mutex::new([None; ACTIONS]);

/// What each analog action follows ([`follow`]), and whether the pad's next read of it is its
/// first.
static SOURCES: Mutex<[Option<Source>; ACTIONS]> = Mutex::new([None; ACTIONS]);
static PAD_FRESH: Mutex<[bool; ACTIONS]> = Mutex::new([false; ACTIONS]);

fn index(action: Action) -> usize {
    Action::ALL.iter().position(|&a| a == action).unwrap_or(0)
}

fn table() -> Table {
    GAME.locked().unwrap_or(STANDARD)
}

/// Game `G`'s actions for the run (its component's install).
pub fn install<G: Actions>() {
    *GAME.locked() = Some(Table { binding: G::binding, engine: G::engine });
    let bound: Vec<String> = Action::ALL.iter().map(|&a| format!("{a:?} {}", describe(G::binding(a)))).collect();
    log!("actions: {}", bound.join(", "));
}

fn describe(binding: Binding) -> &'static str {
    match binding {
        Binding::Pad(_) => "on the pad",
        Binding::Toggle(_) => "a pad toggle",
        Binding::Stick(_) => "a pad stick",
        Binding::Engine => "the game's own way",
        Binding::None => "none",
    }
}

/// Lets go of every action still held (the run's stop, before the pad stops). An analog action
/// followed stays followed: a stopped pad presses nothing, and the game's own way reads only while
/// the game takes it.
pub fn let_go_all() {
    for action in Action::ALL.into_iter().filter(|a| !a.analog()) {
        hold(action, false);
    }
}

/// The run is over: every action let go, the analog ones no longer followed, and the standard
/// layout answers again.
pub fn release() {
    let_go_all();
    for action in Action::ALL.into_iter().filter(|a| a.analog()) {
        follow(action, None);
    }
    *GAME.locked() = None;
}

/// How the game takes `action` now (the standard layout before a game's actions are installed).
pub fn binding(action: Action) -> Binding {
    (table().binding)(action)
}

/// Whether the game has `action` now.
pub fn can(action: Action) -> bool {
    binding(action) != Binding::None
}

/// Presses `action` once, for about `for_ms`. False when the game has no such action now, or its own
/// way refused (an analog action is followed, never pressed).
pub fn press(action: Action, for_ms: u64) -> bool {
    let table = table();
    match (table.binding)(action) {
        Binding::Pad(pad) | Binding::Toggle(pad) => {
            press_pad(action.source(), pad, for_ms);
            true
        }
        Binding::Engine => (table.engine)(action, Ask::Tap { ms: for_ms }),
        Binding::Stick(_) | Binding::None => false,
    }
}

/// Holds `action` down (`on`) or lets go of it, until asked again. A toggle is tapped only when its
/// state changes. False when the game has no such action now, or its own way refused.
pub fn hold(action: Action, on: bool) -> bool {
    let table = table();
    let was = HELD.locked()[index(action)];
    // Let go the way it was held, whatever the binding says now.
    let binding = if on { (table.binding)(action) } else { was.unwrap_or(Binding::None) };
    let done = match (binding, on) {
        (Binding::Pad(pad), true) => {
            hold_pad(action.source(), pad);
            true
        }
        (Binding::Pad(_), false) => {
            let_go(action.source());
            true
        }
        (Binding::Toggle(pad), _) => {
            if was.is_some() != on {
                press_pad(action.source(), pad, TOGGLE_TAP_MS);
            }
            true
        }
        (Binding::Engine, _) => (table.engine)(action, if on { Ask::Hold } else { Ask::LetGo }),
        (Binding::Stick(_) | Binding::None, false) => true,
        (Binding::Stick(_) | Binding::None, true) => false,
    };
    if done {
        HELD.locked()[index(action)] = on.then_some(binding);
    }
    done
}

/// Whether `action` is held or followed now ([`hold`], [`follow`]).
pub fn held(action: Action) -> bool {
    HELD.locked()[index(action)].is_some()
}

/// Has analog `action` follow `source` from now on, or lets it go (`None`): the game asks the source
/// at each of its reads ([`value`]); on the pad its stick is deflected as the source says at each
/// read of the pad. A source follows only while the game takes the action (a lock being picked: on
/// the pad nothing else tells). False when the game has no such action now, or its own way
/// refused.
pub fn follow(action: Action, source: Option<Source>) -> bool {
    let table = table();
    let at = index(action);
    let was = HELD.locked()[at];
    // Let go the way it was followed, whatever the binding says now.
    let binding = if source.is_some() { (table.binding)(action) } else { was.unwrap_or(Binding::None) };
    let done = match (binding, source.is_some()) {
        (Binding::Stick(_), _) => action.analog(),
        (Binding::Engine, on) => action.analog() && (table.engine)(action, if on { Ask::Follow } else { Ask::LetGo }),
        (_, false) => true,
        (_, true) => false,
    };
    if done {
        HELD.locked()[at] = source.map(|_| binding);
        SOURCES.locked()[at] = source;
        PAD_FRESH.locked()[at] = source.is_some();
    }
    done
}

/// Whether what analog `action` follows has a value now (a game's own way asks before it reads).
pub fn ready(action: Action) -> bool {
    let source = SOURCES.locked()[index(action)];
    source.is_some_and(|source| (source.ready)(action))
}

/// The value analog `action` follows now, as the game reads it (`reading`): none where nothing
/// follows it or its source has none, and the game's own input stands.
pub fn value(action: Action, reading: Reading) -> Option<f32> {
    let source = SOURCES.locked()[index(action)]?;
    (source.value)(action, reading)
}

/// A stick deflected for analog `action` at `value`: an angle fully that way (x right, y up), an
/// amount sideways.
fn deflection(action: Action, value: f32) -> [f32; 2] {
    match action {
        Action::Pick => [value.sin(), value.cos()],
        _ => [value, 0.0],
    }
}

/// The sticks as the analog actions bound to them on the pad say now, for each read of the pad.
pub(crate) fn sticks() -> Gamepad {
    let mut pad = Gamepad::default();
    let binding = table().binding;
    for action in Action::ALL.into_iter().filter(|a| a.analog()) {
        let Binding::Stick(stick) = binding(action) else { continue };
        if !ready(action) {
            continue;
        }
        let fresh = std::mem::take(&mut PAD_FRESH.locked()[index(action)]);
        let Some([x, y]) = value(action, Reading { fresh }).map(|v| deflection(action, v)) else { continue };
        let axis = |v: f32| (v.clamp(-1.0, 1.0) * 32767.0) as i16;
        match stick {
            Stick::Left => (pad.thumb_lx, pad.thumb_ly) = (axis(x), axis(y)),
            Stick::Right => (pad.thumb_rx, pad.thumb_ry) = (axis(x), axis(y)),
        }
    }
    pad
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TEST_PRESSES;
    use crate::presses::pressed;
    use monaka_producer::game::Adapter;
    use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

    struct Pressing;
    impl Game for Pressing {
        const MANIFESTS: &'static [monaka_core::manifest::Manifest] = &[];
        const ADAPTER: Adapter<Self> = Adapter { components: &[] };
        type Build = ();
        type Frame = ();
        fn prepare(_: &'static monaka_producer::Session, _: &'static monaka_core::manifest::Manifest, _: &monaka_core::options::Options) -> Result<(), monaka_producer::Rejection> {
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
}
