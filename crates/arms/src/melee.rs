//! Melee by swinging: a controller swung like the weapon it holds presses the game's attack (its
//! light attack, by the actions provider: `monaka_pad::actions`; the right trigger on the VR pad
//! unless the game binds it otherwise), once per swing ([`monaka_core::swing`]). With a melee
//! weapon drawn the aim hand swings it; with bare hands either hand punches; a gun, a bow, a
//! throwable or a tool is never swung ([`swing_rules`]). The game's attack plays and hits as it
//! would from the trigger, aimed where the character looks; a producer can aim melee with the head
//! ([`holding_melee`]).

use monaka_core::protocol::{HandPose, LEFT_HAND, RIGHT_HAND};
use monaka_pad::actions::{self, Action};
use monaka_core::swing::{Swing, SwingRules};
use monaka_producer::log;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::*};
use std::time::Instant;

/// How long a swing holds the trigger by default: several game updates, so the game sees the press
/// whatever its frame rate, and short of what it would take for a held attack.
const PRESS_MS: u64 = 150;
/// How long a swing holds the trigger ([`set_press_ms`]).
static PRESS: AtomicU64 = AtomicU64::new(PRESS_MS);

/// The kind of what a hand holds: the held-item provider's kind (`crate::hand_rig::HandRig::held`;
/// the rest of the item, `crate::held`), and what a swing takes the arms to hold ([`update`]). Its
/// rules: [`Held::grips`], [`swing_rules`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Held {
    Gun,
    /// A blunt weapon (or one of unknown kind): a swing any way attacks.
    Melee,
    /// A sharp one: only a cut attacks ([`SwingRules::BLADE`]).
    Blade,
    Nothing,
    /// A bow, in the hand that holds it out (the other draws its string, empty).
    Bow,
    /// Something thrown from the hand: a knife, a grenade, a bottle.
    Throwable,
    /// A tool worked by hand: a lockpick, a lighter, a map.
    Tool,
}

/// How swings attack.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// Swings attack at all.
    pub swing: bool,
    /// How fast a weapon's tip must go to swing it (m/s, half a metre out; bare hands scale with it).
    pub speed: f32,
    /// The controller that holds the weapon (`LEFT_HAND` or `RIGHT_HAND`).
    pub aim_side: usize,
}

impl Settings {
    /// `swing_attack` (default on) and `swing_speed` (m/s, 0.5..=10; default the weapon rule's)
    /// from the options, the weapon in `aim_side`.
    pub fn from_options(options: &monaka_core::options::Options, aim_side: usize) -> Self {
        Self { swing: options.switch("swing_attack", true), speed: options.number("swing_speed", 0.5, 10.0, SwingRules::WEAPON.start_speed), aim_side }
    }
}

/// Physical melee (`physical_melee`, with the hand rig; default on): a swing hits where the weapon
/// in the hand is, through the game's own hit detection, which a game's producer points at the
/// hand and cuts to these lengths.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Physical {
    /// How far a held weapon reaches past the hand (`melee_blade`, metres).
    pub blade: f32,
    /// How far a fist reaches past the hand (`melee_fist`, metres).
    pub fist: f32,
    /// How long a swing holds the attack (`swing_press_ms`), for a game that starts an attack when
    /// it is let go (Dying Light 1, The Beast): a short press starts it, and its hit window, while
    /// the arm is still swinging. `None` keeps the default press.
    pub press_ms: Option<u64>,
}

impl Physical {
    /// From the options, with the hand rig (`rig`); `attacks_on_release` reads the swing's press.
    pub fn from_options(options: &monaka_core::options::Options, rig: bool, attacks_on_release: bool) -> Option<Self> {
        let on = options.switch("physical_melee", true);
        let physical = Self {
            blade: options.number("melee_blade", 0.2, 2.5, 1.1),
            fist: options.number("melee_fist", 0.05, 0.6, 0.15),
            press_ms: attacks_on_release.then(|| options.number("swing_press_ms", 15.0, 300.0, 40.0) as u64),
        };
        (rig && on).then_some(physical)
    }

    /// How far `held` reaches past the hand.
    pub fn reach(&self, held: Held) -> f32 {
        if held == Held::Nothing { self.fist } else { self.blade }
    }

    /// Sets the swing's press ([`set_press_ms`]) and logs the settings.
    pub fn apply(&self) {
        if let Some(ms) = self.press_ms {
            set_press_ms(ms);
        }
        log!("physical melee: blade {} m, fist {} m past the hand, swings press {} ms", self.blade, self.fist, PRESS.load(Relaxed));
    }
}

/// What the arms held at the latest update (a [`Held`] as its [`code`]).
static HELD: AtomicU64 = AtomicU64::new(0);

/// [`Held`] as kept in [`HELD`].
const fn code(held: Held) -> u64 {
    match held {
        Held::Gun => 0,
        Held::Melee => 1,
        Held::Nothing => 2,
        Held::Blade => 3,
        Held::Bow => 4,
        Held::Throwable => 5,
        Held::Tool => 6,
    }
}

/// [`code`] read back.
const fn of_code(code: u64) -> Held {
    match code {
        0 => Held::Gun,
        1 => Held::Melee,
        3 => Held::Blade,
        4 => Held::Bow,
        5 => Held::Throwable,
        6 => Held::Tool,
        _ => Held::Nothing,
    }
}
static SWINGS: AtomicU64 = AtomicU64::new(0);
/// When the latest swing attacked (`monaka_channel::tick`), 0 for never.
static LAST_SWING: AtomicU64 = AtomicU64::new(0);
static SETTLED_LOGGED: AtomicU64 = AtomicU64::new(0);
static ACTIVE: AtomicBool = AtomicBool::new(false);
/// Which way a held blade runs from the palm, in the controller's frame, as the producer last
/// measured it ([`set_blade`]); `None`: ahead.
static BLADE: Mutex<Option<[f32; 3]>> = Mutex::new(None);

/// The held blade runs `direction` (unit, controller frame: x right, y up, z back) from the palm,
/// as the game draws it: the swing's tip and its edge follow (a producer measuring the weapon
/// model each frame).
pub fn set_blade(direction: [f32; 3]) {
    if let Ok(mut blade) = BLADE.lock() {
        *blade = Some(direction);
    }
}

/// Whether the arms hold a melee weapon or nothing (not a gun) at the latest update. A producer
/// aims the character with the head then: the attack goes where the player looks, not where the
/// swinging controller happens to point when it lands.
pub fn holding_melee() -> bool {
    ACTIVE.load(Relaxed) && matches!(of_code(HELD.load(Relaxed)), Held::Melee | Held::Blade | Held::Nothing)
}

/// How long a swing holds the trigger, in milliseconds (default 150). A game that starts an
/// attack when the trigger is let go (Dying Light 1, The Beast) attacks sooner with a shorter
/// press ([`Physical::press_ms`]).
pub fn set_press_ms(ms: u64) {
    PRESS.store(ms, Relaxed);
}

/// What the arms held at the latest update, `None` before the first.
pub fn held() -> Option<Held> {
    if !ACTIVE.load(Relaxed) {
        return None;
    }
    Some(of_code(HELD.load(Relaxed)))
}

/// After a swing, how far the game's own animation moved the aim wrist (camera space): its attack
/// playing moves it a lot, holding still hardly at all. Logged for the first few swings.
struct Watch {
    swing: u64,
    updates: u32,
    low: [f32; 3],
    high: [f32; 3],
}

static WATCH: Mutex<Option<Watch>> = Mutex::new(None);
/// Updates watched after a swing (about half a second).
const WATCHED: u32 = 40;

/// The aim wrist where the game animated it this update, against its camera (from the hand rig).
pub fn note_animated_wrist(at: [f32; 3]) {
    let Ok(mut watch) = WATCH.lock() else { return };
    let Some(w) = watch.as_mut() else { return };
    w.low = [0, 1, 2].map(|k| w.low[k].min(at[k]));
    w.high = [0, 1, 2].map(|k| w.high[k].max(at[k]));
    w.updates += 1;
    if w.updates >= WATCHED {
        let extent = [0, 1, 2].map(|k| w.high[k] - w.low[k]);
        log!("melee: after swing {} the game's animated wrist moved over {extent:.2?} m in {} updates", w.swing, w.updates);
        *watch = None;
    }
}

struct Hands {
    swings: [Swing; 2],
    /// The latest sample fed per side (a pose is read every update, but changes once a present).
    last: [Option<HandPose>; 2],
    started: Option<Instant>,
    /// A weapon's swing under way that has not attacked yet, with its start's speed and direction:
    /// it attacks once the reach check says the weapon is about to reach something
    /// ([`set_reach_check`]).
    armed: [Option<(f32, [f32; 3])>; 2],
}

static HANDS: Mutex<Hands> = Mutex::new(Hands { swings: [Swing::new(), Swing::new()], last: [None, None], started: None, armed: [None; 2] });

/// The producer's reach check (a `fn() -> bool`), 0 for none.
static REACH_CHECK: AtomicUsize = AtomicUsize::new(0);
/// Weapon swings that settled without reaching anything, so attacked nothing.
static AIR_SWINGS: AtomicU64 = AtomicU64::new(0);

/// Has a weapon's swing attack only when the weapon is about to reach something it can hit:
/// `check` says whether it is now (the producer looks a moment ahead, for the attack to start in
/// time). A swing then arms the attack and presses it at that moment; a swing that settles first
/// (a wind-up, a swing at the air) attacks nothing, so it costs no stamina and leaves no attack
/// recovery for the next swing to fall into. Swings any way still count, backhands included. Fists
/// and games without a check press at the swing's start.
pub fn set_reach_check(check: fn() -> bool) {
    REACH_CHECK.store(check as usize, Relaxed);
}

fn reach_check() -> Option<fn() -> bool> {
    let at = REACH_CHECK.load(Relaxed);
    // SAFETY: stored from an `fn() -> bool` by `set_reach_check`.
    (at != 0).then(|| unsafe { std::mem::transmute::<usize, fn() -> bool>(at) })
}

/// Presses the attack for a swing, and notes it.
fn attack(held: Held, side: usize, speed: f32, direction: [f32; 3]) {
    actions::press(Action::Attack, PRESS.load(Relaxed));
    LAST_SWING.store(monaka_channel::tick(), Relaxed);
    let n = SWINGS.fetch_add(1, Relaxed) + 1;
    if n <= 8
        && let Ok(mut watch) = WATCH.lock()
    {
        *watch = Some(Watch { swing: n, updates: 0, low: [f32::MAX; 3], high: [f32::MIN; 3] });
    }
    if n <= 200 {
        log!("melee: swing {n} ({held:?}, {} hand) at {speed:.1} m/s toward {direction:.2?}", side_name(side));
    }
}

/// One arms update: notes what is held and feeds the controllers (`palms`, by side, in tracking
/// space) to their swing detectors.
pub fn update(held: Held, palms: [Option<HandPose>; 2], settings: &Settings) {
    let code = code(held);
    if HELD.swap(code, Relaxed) != code || !ACTIVE.swap(true, Relaxed) {
        log!("melee: holding {held:?}");
    }
    if !settings.swing {
        return;
    }
    let Ok(mut hands) = HANDS.lock() else { return };
    // Climbing or hanging: the game drives the arms, and a swing would only press the trigger.
    if crate::traverse::traversing() {
        hands.swings.iter_mut().for_each(Swing::reset);
        hands.armed = [None; 2];
        return;
    }
    let time = hands.started.get_or_insert_with(Instant::now).elapsed().as_secs_f64();
    let blade = BLADE.lock().ok().and_then(|b| *b);
    for side in [LEFT_HAND, RIGHT_HAND] {
        let Some(rules) = swing_rules(held, side, settings, blade) else {
            hands.swings[side].reset();
            hands.armed[side] = None;
            continue;
        };
        let Some(palm) = palms[side] else {
            hands.swings[side].reset();
            hands.armed[side] = None;
            continue;
        };
        if hands.last[side] == Some(palm) {
            continue;
        }
        hands.last[side] = Some(palm);
        let step = hands.swings[side].feed(time, palm.position, palm.orientation, &rules);
        let check = reach_check().filter(|_| matches!(held, Held::Melee | Held::Blade));
        if let Some(started) = step.started {
            match check {
                Some(_) => hands.armed[side] = Some((started.speed, started.direction)),
                None => attack(held, side, started.speed, started.direction),
            }
        }
        if let Some((speed, direction)) = hands.armed[side]
            && check.is_some_and(|reaches| reaches())
        {
            hands.armed[side] = None;
            attack(held, side, speed, direction);
        }
        if step.settled.is_some() && hands.armed[side].take().is_some() {
            let n = AIR_SWINGS.fetch_add(1, Relaxed) + 1;
            if n <= 50 {
                log!("melee: a {} hand swing reached nothing: no attack (air swing {n})", side_name(side));
            }
        }
        if let Some((peak, swung)) = step.settled
            && SETTLED_LOGGED.fetch_add(1, Relaxed) < 400
        {
            let what = match (swung, step.uncut) {
                (true, _) => "a swing",
                (false, true) => "not a cut: the flat or the point",
                (false, false) => "too slow to swing",
            };
            log!("melee: {} hand settled from {peak:.1} m/s ({what})", side_name(side));
        }
    }
}

/// How hand `side` swings while the arms hold `held` (the weapon in the aim hand, `settings`), or
/// whether it does not: the aim hand swings a melee weapon (a blade along `blade`, the way the
/// producer measured it, else ahead), either hand punches with bare hands, and nothing held as a
/// gun, a bow, a throwable or a tool swings. The speed to swing scales with `settings.speed`.
pub fn swing_rules(held: Held, side: usize, settings: &Settings, blade: Option<[f32; 3]>) -> Option<SwingRules> {
    let speed = settings.speed / SwingRules::WEAPON.start_speed;
    match held {
        Held::Melee if side == settings.aim_side => Some(SwingRules::WEAPON.faster(speed)),
        Held::Blade if side == settings.aim_side => Some(SwingRules::BLADE.faster(speed).pointing(blade.unwrap_or(SwingRules::BLADE.blade))),
        Held::Nothing => Some(SwingRules::FIST.faster(speed)),
        _ => None,
    }
}

/// Whether a swing attacked within the last `ms` milliseconds.
pub fn swung_within(ms: u64) -> bool {
    let at = LAST_SWING.load(Relaxed);
    at != 0 && monaka_channel::tick().saturating_sub(at) < ms
}

fn side_name(side: usize) -> &'static str {
    if side == LEFT_HAND { "left" } else { "right" }
}

pub fn report() {
    if ACTIVE.load(Relaxed) {
        log!("{}", summary());
    }
}

/// The swings so far, for the stop's log.
pub fn summary() -> String {
    format!("melee: {} swings pressed the attack, {} reached nothing", SWINGS.load(Relaxed), AIR_SWINGS.load(Relaxed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use monaka_core::pad::Gamepad;
    use monaka_pad::actions::Binding;

    const KINDS: [Held; 7] = [Held::Gun, Held::Melee, Held::Blade, Held::Nothing, Held::Bow, Held::Throwable, Held::Tool];

    /// The rules [`update`] chose inline before [`swing_rules`].
    fn rules_before(held: Held, side: usize, settings: &Settings, blade: Option<[f32; 3]>) -> Option<SwingRules> {
        let speed = settings.speed / SwingRules::WEAPON.start_speed;
        match held {
            Held::Melee if side == settings.aim_side => Some(SwingRules::WEAPON.faster(speed)),
            Held::Blade if side == settings.aim_side => {
                let blade = blade.unwrap_or(SwingRules::BLADE.blade);
                Some(SwingRules::BLADE.faster(speed).pointing(blade))
            }
            Held::Nothing => Some(SwingRules::FIST.faster(speed)),
            Held::Gun | Held::Melee | Held::Blade => None,
            _ => unreachable!("not a kind before"),
        }
    }

    #[test]
    fn the_swing_rules_are_the_ones_chosen_before_and_new_kinds_never_swing() {
        for aim_side in [LEFT_HAND, RIGHT_HAND] {
            for speed in [0.5, 2.2, 7.0] {
                let settings = Settings { swing: true, speed, aim_side };
                for blade in [None, Some([0.0, 0.6, -0.8])] {
                    for side in [LEFT_HAND, RIGHT_HAND] {
                        for held in [Held::Gun, Held::Melee, Held::Blade, Held::Nothing] {
                            assert_eq!(swing_rules(held, side, &settings, blade), rules_before(held, side, &settings, blade), "{held:?} {side} {aim_side} {speed}");
                        }
                        for held in [Held::Bow, Held::Throwable, Held::Tool] {
                            assert!(swing_rules(held, side, &settings, blade).is_none(), "{held:?}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn each_kind_is_kept_and_read_back_and_only_melee_or_fists_aim_by_the_head() {
        for held in KINDS {
            assert_eq!(of_code(code(held)), held);
        }
        // The kinds before: everything but a gun aimed melee by the head.
        for held in [Held::Gun, Held::Melee, Held::Blade, Held::Nothing] {
            assert_eq!(matches!(held, Held::Melee | Held::Blade | Held::Nothing), held != Held::Gun);
        }
        assert_eq!([code(Held::Gun), code(Held::Melee), code(Held::Nothing), code(Held::Blade)], [0, 1, 2, 3], "the codes kept before");
    }

    #[test]
    fn a_swing_presses_the_right_trigger_as_before() {
        // No game's actions installed in this test run: the standard layout, the trigger a swing held.
        assert_eq!(actions::binding(Action::Attack), Binding::Pad(Gamepad { right_trigger: 255, ..Gamepad::default() }));
    }
}
