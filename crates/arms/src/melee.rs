//! Melee by swinging: a controller swung like the weapon it holds presses the game's attack (the
//! right trigger, on the VR pad: `monaka_pad::press`), once per swing ([`monaka_core::swing`]). With a
//! melee weapon drawn the aim hand swings it; with bare hands either hand punches; a gun is never
//! fired by a swing. The game's attack plays and hits as it would from the trigger, aimed where the
//! character looks; a producer can aim melee with the head ([`holding_melee`]).

use monaka_core::pad::Gamepad;
use monaka_core::protocol::{HandPose, LEFT_HAND, RIGHT_HAND};
use monaka_core::swing::{Swing, SwingRules};
use monaka_producer::log;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::*};
use std::time::Instant;

/// How long a swing holds the trigger by default: several game updates, so the game sees the press
/// whatever its frame rate, and short of what it would take for a held attack.
const PRESS_MS: u64 = 150;
/// How long a swing holds the trigger ([`set_press_ms`]).
static PRESS: AtomicU64 = AtomicU64::new(PRESS_MS);

/// What the first-person arms hold.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Held {
    Gun,
    /// A blunt weapon (or one of unknown kind): a swing any way attacks.
    Melee,
    /// A sharp one: only a cut attacks ([`SwingRules::BLADE`]).
    Blade,
    Nothing,
}

impl Held {
    /// What the arms' weapon visual `weapon` (0 for none) is: a gun ([`crate::is_gun`]), another
    /// weapon, or nothing.
    pub fn of(weapon: usize) -> Self {
        match weapon {
            0 => Held::Nothing,
            w if crate::is_gun(w) => Held::Gun,
            _ => Held::Melee,
        }
    }
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

/// What the arms held at the latest update (a [`Held`] as 0 gun, 1 melee, 2 nothing).
static HELD: AtomicU64 = AtomicU64::new(0);
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
    ACTIVE.load(Relaxed) && HELD.load(Relaxed) != 0
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
    Some(match HELD.load(Relaxed) {
        0 => Held::Gun,
        1 => Held::Melee,
        3 => Held::Blade,
        _ => Held::Nothing,
    })
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
}

static HANDS: Mutex<Hands> = Mutex::new(Hands { swings: [Swing::new(), Swing::new()], last: [None, None], started: None });

/// One arms update: notes what is held and feeds the controllers (`palms`, by side, in tracking
/// space) to their swing detectors.
pub fn update(held: Held, palms: [Option<HandPose>; 2], settings: &Settings) {
    let code = match held {
        Held::Gun => 0,
        Held::Melee => 1,
        Held::Nothing => 2,
        Held::Blade => 3,
    };
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
        return;
    }
    let time = hands.started.get_or_insert_with(Instant::now).elapsed().as_secs_f64();
    let speed = settings.speed / SwingRules::WEAPON.start_speed;
    for side in [LEFT_HAND, RIGHT_HAND] {
        let rules = match held {
            Held::Melee if side == settings.aim_side => SwingRules::WEAPON.faster(speed),
            Held::Blade if side == settings.aim_side => {
                let blade = BLADE.lock().ok().and_then(|b| *b).unwrap_or(SwingRules::BLADE.blade);
                SwingRules::BLADE.faster(speed).pointing(blade)
            }
            Held::Nothing => SwingRules::FIST.faster(speed),
            _ => {
                hands.swings[side].reset();
                continue;
            }
        };
        let Some(palm) = palms[side] else {
            hands.swings[side].reset();
            continue;
        };
        if hands.last[side] == Some(palm) {
            continue;
        }
        hands.last[side] = Some(palm);
        let step = hands.swings[side].feed(time, palm.position, palm.orientation, &rules);
        if let Some(started) = step.started {
            monaka_pad::press(Gamepad { right_trigger: 255, ..Gamepad::default() }, PRESS.load(Relaxed));
            LAST_SWING.store(monaka_channel::tick(), Relaxed);
            let n = SWINGS.fetch_add(1, Relaxed) + 1;
            if n <= 8
                && let Ok(mut watch) = WATCH.lock()
            {
                *watch = Some(Watch { swing: n, updates: 0, low: [f32::MAX; 3], high: [f32::MIN; 3] });
            }
            if n <= 200 {
                log!("melee: swing {n} ({held:?}, {} hand) at {:.1} m/s toward {:.2?}", side_name(side), started.speed, started.direction);
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
        log!("melee: {} swings pressed the attack", SWINGS.load(Relaxed));
    }
}
