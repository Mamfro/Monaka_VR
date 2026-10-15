//! Melee by swinging: a controller swung like the weapon it holds presses the game's attack (its
//! light attack, by the actions provider: `monaka_pad::actions`; the right trigger on the VR pad
//! unless the game binds it otherwise), once per swing ([`monaka_core::swing`]). With a melee
//! weapon drawn the aim hand swings it; with bare hands either hand punches; a gun, a bow, a
//! throwable or a tool is never swung ([`swing_rules`]). The game's attack plays and hits as it
//! would from the trigger, aimed where the character looks; a producer can aim melee with the head
//! (`crate::held::wielded`). What a hand holds is the held item's kind ([`Kind`]). A producer that
//! opens the game's hit detection on the swing itself has swings press nothing ([`set_presses`]).

use crate::held::Kind;
use monaka_contract::protocol::{HandPose, LEFT_HAND, RIGHT_HAND};
use monaka_pad::actions::{self, Action};
use monaka_core::swing::{Swing, SwingRules};
use monaka_producer::log;
use monaka_core::sync::Locked;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::*};
use std::time::Instant;

/// How long a swing holds the trigger by default: several game updates, so the game sees the press
/// whatever its frame rate, and short of what it would take for a held attack.
const PRESS_MS: u64 = 150;
/// How long a swing holds the trigger ([`set_press_ms`]).
static PRESS: AtomicU64 = AtomicU64::new(PRESS_MS);

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
            fist: options.number("melee_fist", 0.05, 1.0, 0.75),
            press_ms: attacks_on_release.then(|| options.number("swing_press_ms", 15.0, 300.0, 40.0) as u64),
        };
        (rig && on).then_some(physical)
    }

    /// How far `held` reaches past the hand.
    pub fn reach(&self, held: Kind) -> f32 {
        if held == Kind::Nothing { self.fist } else { self.blade }
    }

    /// Sets the swing's press ([`set_press_ms`]) and logs the settings.
    pub fn apply(&self) {
        if let Some(ms) = self.press_ms {
            set_press_ms(ms);
        }
        log!("physical melee: blade {} m, fist {} m past the hand, swings press {} ms", self.blade, self.fist, PRESS.load(Relaxed));
    }
}

/// What the swings took the arms to hold at the latest update, for the log; none before the first.
static SWUNG: Mutex<Option<Kind>> = Mutex::new(None);
static SWINGS: AtomicU64 = AtomicU64::new(0);
/// When the latest swing attacked (`monaka_channel::tick`), 0 for never.
static LAST_SWING: AtomicU64 = AtomicU64::new(0);
static SETTLED_LOGGED: AtomicU64 = AtomicU64::new(0);
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

/// Swings press the game's attack ([`set_presses`]).
static PRESSES: AtomicBool = AtomicBool::new(true);

/// Whether a swing presses the game's attack (default yes). Off, a swing only counts
/// ([`swung_at`], [`swinging`]): for a producer that opens the game's hit detection on the swing
/// itself, where the attack would only play the game's canned attack animation.
pub fn set_presses(on: bool) {
    PRESSES.store(on, Relaxed);
}

/// How long a swing holds the trigger, in milliseconds (default 150). A game that starts an
/// attack when the trigger is let go (Dying Light 1, The Beast) attacks sooner with a shorter
/// press ([`Physical::press_ms`]).
pub fn set_press_ms(ms: u64) {
    PRESS.store(ms, Relaxed);
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

/// Presses the attack for a swing (unless swings press nothing), and notes it.
fn attack(held: Kind, side: usize, speed: f32, direction: [f32; 3]) {
    if PRESSES.load(Relaxed) {
        actions::press(Action::Attack, PRESS.load(Relaxed));
    }
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
pub fn update(held: Kind, palms: [Option<HandPose>; 2], settings: &Settings) {
    if SWUNG.locked().replace(held) != Some(held) {
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
        let check = reach_check().filter(|_| matches!(held, Kind::Melee | Kind::Blade));
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
pub fn swing_rules(held: Kind, side: usize, settings: &Settings, blade: Option<[f32; 3]>) -> Option<SwingRules> {
    let speed = settings.speed / SwingRules::WEAPON.start_speed;
    match held {
        Kind::Melee if side == settings.aim_side => Some(SwingRules::WEAPON.faster(speed)),
        Kind::Blade if side == settings.aim_side => Some(SwingRules::BLADE.faster(speed).pointing(blade.unwrap_or(SwingRules::BLADE.blade))),
        Kind::Nothing => Some(SwingRules::FIST.faster(speed)),
        _ => None,
    }
}

/// Whether hand `side` is swinging now: a swing started and its tip has not slowed below the
/// settle speed since (a hit window that lasts as long as the swing's motion does).
pub fn swinging(side: usize) -> bool {
    HANDS.lock().is_ok_and(|hands| hands.swings[side.min(1)].swinging())
}

/// Whether a swing attacked within the last `ms` milliseconds.
pub fn swung_within(ms: u64) -> bool {
    swung_at().is_some_and(|at| monaka_channel::tick().saturating_sub(at) < ms)
}

/// When the latest swing attacked (`monaka_channel::tick`); none before the first.
pub fn swung_at() -> Option<u64> {
    Some(LAST_SWING.load(Relaxed)).filter(|&at| at != 0)
}

fn side_name(side: usize) -> &'static str {
    if side == LEFT_HAND { "left" } else { "right" }
}

pub fn report() {
    if SWUNG.locked().is_some() {
        log!("{}", summary());
    }
}

/// The swings so far, for the stop's log.
pub fn summary() -> String {
    let pressed = if PRESSES.load(Relaxed) { "pressed the attack" } else { "pressed nothing" };
    format!("melee: {} swings {pressed}, {} reached nothing", SWINGS.load(Relaxed), AIR_SWINGS.load(Relaxed))
}

#[cfg(test)]
mod tests;
