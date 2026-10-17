//! Physical melee, as a ready-made (`physical_melee`): a swing hits where the weapon in the
//! player's hand passes through an enemy, from the way it moved, through the game's own hit
//! detection. The swing that presses the attack is [`crate::melee`]'s.
//!
//! A game implements the melee-hit provider ([`Hits`]): its hit detection hooked so the hit window
//! opens at the attack's start, each hit keeps the direction the hand moved, and the blade is the
//! weapon in the hand ([`blade`]: its length for what the arms hold); and, where it has them, the
//! boxes of what its melee can hit near a point ([`Hits::near`], for attacking on reach) and where
//! it tells a cut from a blunt hit ([`Hits::install_blunt`], asking [`edge_led`]).
//!
//! Attack on reach (`melee_on_reach`, on by default): a weapon's swing presses the attack only once
//! the blade is about to reach something the game's melee can hit ([`crate::melee::set_reach_check`]):
//! a wind-up or a swing at the air starts no attack and costs no stamina. At the game's look moment
//! ([`look`]) the blade runs from the aim hand's holder along the held weapon's line, the weapon's
//! length long; where it is and where it will be [`LOOKAHEAD`] on at its speed is tested against
//! the boxes the game finds near it. Without a look lately every swing attacks.
//!
//! A blunt back (`back_is_blunt`, on by default, where the game tells): a cutting weapon's hit that
//! did not lead with its edge (the held line's edge) lands blunt: the back of an axe bludgeons.

use crate::held::{self, HeldItems, Kind};
use crate::melee::{self, Physical};
use monaka_core::feature::Feature;
use monaka_core::math::{Vec3, distance, dot, length};
use monaka_core::sync::Locked;
use monaka_producer::game::{Component, Lack, Need, On, Run, Start, lines};
use monaka_producer::log;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// A game's melee-hit provider: its hit detection, and what its melee can hit.
pub trait Hits: HeldItems {
    /// The game starts a light attack when its button is let go: a swing presses briefly
    /// ([`Physical::press_ms`]).
    const ATTACKS_ON_RELEASE: bool;

    /// Hooks the game's hit detection: the hit window opened at the attack's start, each hit's
    /// direction the hand's, the blade cut to [`blade`]'s length.
    fn install(start: &mut Start<'_, Self>) -> Result<(), Lack>;

    /// Readies the search for what the melee can hit ([`Hits::near`]) and the look moment, where
    /// the game calls [`look`] (unless it calls it from its posing moment anyway). Err: every swing
    /// attacks, and no hit is told blunt.
    fn install_look(_start: &mut Start<'_, Self>) -> Result<(), Lack> {
        Err(Lack::Missing("the game's melee targets are not read for this build".into()))
    }

    /// Hooks where the game types each hit, asking [`edge_led`] for the player's cuts.
    fn install_blunt(_start: &mut Start<'_, Self>) -> Result<(), Lack> {
        Err(Lack::Missing("the game's hits are not typed by the blade".into()))
    }

    /// What the game's melee can hit within `radius` of `centre` (world), the player left out:
    /// each by an id of the game's and its box (least x, y, z, then most); none where the game cannot
    /// search now.
    fn near(_centre: Vec3, _radius: f32) -> Option<Vec<(usize, [f32; 6])>> {
        None
    }

    /// What `id` (from [`Hits::near`]) is, for the log.
    fn name(_id: usize) -> String {
        String::new()
    }

    /// The hooks' counts, for the stop's log (empty: nothing to say).
    fn summary() -> String {
        String::new()
    }
}

/// How far ahead the blade's path is looked at (seconds): the press, the game's next update and
/// the attack's start all come before the attack can hit.
pub const LOOKAHEAD: f32 = 0.15;
/// How near an object's box the blade counts as reaching it (metres).
const MARGIN: f32 = 0.1;
/// The least height of a box that counts (metres): a body's, lying or standing.
const LEAST_HEIGHT: f32 = 0.4;
/// A reach seen within this is current (ms); the check answers yes to anything older than
/// [`STALE_MS`] since the last look (the look not running: every swing attacks).
const REACH_FRESH_MS: u64 = 60;
const STALE_MS: u64 = 300;
/// An object is found by its own place (its feet): the search's radius takes in a body's height.
const BODY: f32 = 2.0;
/// A cutting weapon's hit cuts when the tip moved at least this share along the edge's side.
const EDGE_LEAD: f32 = 0.5;

/// Physical melee for game `G`, with the hand rig (`physical_melee`, on by default; its lengths
/// and press are [`Physical`]'s options). A game says what it works on top of by struct update.
pub const fn physical_melee<G: Hits>() -> Component<G> {
    Component { needs: &[Need::Feature(Feature::HandRig)], name: "melee", features: &[Feature::Melee], on: on::<G>, install: install::<G>, summary: Some(summary::<G>), ..Component::NONE }
}

fn on<G: Hits>(run: &Run<'_, G>) -> On {
    On::when(Physical::from_options(run.options, run.shared.aim.rig, G::ATTACKS_ON_RELEASE).is_some(), "physical melee is off")
}

static PHYSICAL: OnceLock<Physical> = OnceLock::new();
/// The hand whose weapon the look follows.
static AIM_SIDE: AtomicUsize = AtomicUsize::new(monaka_contract::protocol::RIGHT_HAND);
static LOOK: ReachLook = ReachLook::new();
/// The blade at the last two looks: [before, latest].
static BLADES: Mutex<[Option<held::WorldLine>; 2]> = Mutex::new([None; 2]);
static DESCRIBED: AtomicU64 = AtomicU64::new(0);

fn install<G: Hits>(start: &mut Start<'_, G>) -> Result<(), Lack> {
    let physical = Physical::from_options(start.options, start.shared.aim.rig, G::ATTACKS_ON_RELEASE).ok_or_else(|| Lack::Missing("physical melee is off".into()))?;
    let _ = PHYSICAL.set(physical);
    AIM_SIDE.store(start.shared.aim.side, Relaxed);
    G::install(start)?;
    let on_reach = start.options.switch("melee_on_reach", true);
    let blunt = start.options.switch("back_is_blunt", true);
    if on_reach || blunt {
        match G::install_look(start) {
            Ok(()) if on_reach => {
                melee::set_reach_check(reaches);
                log!("physical melee: a weapon's swing attacks once the blade is about to reach something");
            }
            Ok(()) => {}
            Err(Lack::Missing(why)) => log!("physical melee: every swing attacks: {why}"),
            Err(other) => return Err(other),
        }
    }
    if blunt {
        match G::install_blunt(start) {
            Ok(()) => log!("physical melee: a cutting weapon's hit that does not lead with the edge lands blunt"),
            Err(Lack::Missing(why)) => log!("physical melee: hits land as the game types them: {why}"),
            Err(other) => return Err(other),
        }
    }
    physical.apply();
    Ok(())
}

fn summary<G: Hits>() -> String {
    let looks = LOOK.looks();
    lines(&[G::summary(), if looks > 0 { format!("physical melee: {looks} looks for what the blade reaches") } else { String::new() }])
}

/// The blade's length for `held` (metres: the weapon in the hand, or a fist's reach), once
/// physical melee is on: what the game's hit detection is cut to.
pub fn blade(held: Kind) -> Option<f32> {
    PHYSICAL.get().map(|p| p.reach(held))
}

/// What the aim hand held at the hand rig's latest posing, as melee takes it
/// ([`held::wielded`]); none before the rig's first posing.
pub fn wielded() -> Option<Kind> {
    held::wielded(AIM_SIDE.load(Relaxed))
}

fn reaches() -> bool {
    LOOK.reaches()
}

/// Game `G`'s look moment (on the game's thread, the arms posed): where the aim hand's weapon is,
/// as the swings take it, and whether the blade, where it is and where it will be [`LOOKAHEAD`]
/// from now, touches the box of anything the game's melee can hit.
pub fn look<G: Hits>() {
    let Some(held) = wielded().filter(|h| matches!(h, Kind::Melee | Kind::Blade)) else { return };
    let Some(length) = blade(held) else { return };
    let side = AIM_SIDE.load(Relaxed);
    let Some(line) = held::in_world::<G>(side).and_then(|posed| posed.line) else { return };
    {
        let mut blades = BLADES.locked();
        *blades = [blades[1], Some(line)];
    }
    let (palm, tip) = (line.from, line.at(length));
    let Some(sweep) = LOOK.sweep(palm, tip) else { return };
    let Some(found) = G::near(sweep.centre, sweep.spread + BODY) else { return };
    let looks = LOOK.counted();
    let describe = looks.is_multiple_of(200) && DESCRIBED.fetch_add(1, Relaxed) < 30;
    if describe {
        log!("physical melee look {looks}: blade {palm:.2?} to {tip:.2?}, {} found near {:.2?}", found.len(), sweep.centre);
    }
    for (id, b) in found {
        if describe {
            log!("  {} ({id:#x}), box {b:.2?}", G::name(id));
        }
        if sweep.touches(&b) {
            LOOK.reached(|| format!("{} ({id:#x}, box {b:.2?})", G::name(id)));
            break;
        }
    }
}

/// Whether the player's latest hit led with the edge: the blade's tip (a metre along it) moved over
/// the last two looks at least [`EDGE_LEAD`] along the edge's side. None without an edge or a
/// motion (the hit then stays as the game types it).
pub fn edge_led() -> Option<bool> {
    let [Some(before), Some(now)] = *BLADES.locked() else { return None };
    lead(&before, &now).map(|lead| lead >= EDGE_LEAD)
}

/// How much of the blade tip's way from `before` to `now` ran along `now`'s edge (-1 to 1); none
/// without an edge, or for a tip that hardly moved (2 mm).
pub fn lead(before: &held::WorldLine, now: &held::WorldLine) -> Option<f32> {
    let edge = now.edge?;
    let (a, b) = (before.at(1.0), now.at(1.0));
    let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let moved = length(d);
    if moved.is_nan() || moved <= 0.002 {
        return None;
    }
    Some(dot(d, edge) / moved)
}

// ---- The reach look ----

/// The blade's hand end and tip at a look, and when.
type Blade = (Vec3, Vec3, Instant);

/// A reach look's state between looks and its answer for the swings ([`ReachLook::reaches`]): the
/// blade sampled where it is and where it will be, the boxes the game finds tested against it.
pub struct ReachLook {
    /// The blade's hand end and tip at the previous look, and when.
    last: Mutex<Option<Blade>>,
    /// When the last look ran, and when it last found the blade about to reach something (ticks).
    looked: AtomicU64,
    reached: AtomicU64,
    looks: AtomicU64,
}

/// One look at the blade: points along it now and where it will be, their centre and how far
/// they spread from it.
pub struct Sweep {
    pub points: Vec<Vec3>,
    pub centre: Vec3,
    pub spread: f32,
}

impl ReachLook {
    pub const fn new() -> Self {
        Self { last: Mutex::new(None), looked: AtomicU64::new(0), reached: AtomicU64::new(0), looks: AtomicU64::new(0) }
    }

    /// The blade from `palm` to `tip` now and [`LOOKAHEAD`] on, moving on as it moved since the
    /// previous look; none when it is not finite.
    pub fn sweep(&self, palm: Vec3, tip: Vec3) -> Option<Sweep> {
        self.sweep_at(palm, tip, Instant::now())
    }

    /// [`ReachLook::sweep`] at `now`.
    pub fn sweep_at(&self, palm: Vec3, tip: Vec3, now: Instant) -> Option<Sweep> {
        if !palm.iter().chain(&tip).all(|v| v.is_finite()) {
            return None;
        }
        let previous = self.last.lock().ok().and_then(|mut last| last.replace((palm, tip, now)));
        let (palm_ahead, tip_ahead) = match previous {
            Some((p0, t0, then)) if (now - then).as_secs_f32() > 0.0 && (now - then).as_secs_f32() < 0.1 => {
                let scale = LOOKAHEAD / (now - then).as_secs_f32();
                ([0, 1, 2].map(|k| palm[k] + (palm[k] - p0[k]) * scale), [0, 1, 2].map(|k| tip[k] + (tip[k] - t0[k]) * scale))
            }
            _ => (palm, tip),
        };
        let mut points = Vec::with_capacity(15);
        for when in [0.0f32, 0.5, 1.0] {
            let p = [0, 1, 2].map(|k| palm[k] + (palm_ahead[k] - palm[k]) * when);
            let t = [0, 1, 2].map(|k| tip[k] + (tip_ahead[k] - tip[k]) * when);
            for share in [0.0f32, 0.25, 0.5, 0.75, 1.0] {
                points.push([0, 1, 2].map(|k| p[k] + (t[k] - p[k]) * share));
            }
        }
        let centre = [0, 1, 2].map(|k| points.iter().map(|p| p[k]).sum::<f32>() / points.len() as f32);
        let spread = points.iter().map(|p| distance(*p, centre)).fold(0.0f32, f32::max);
        Some(Sweep { points, centre, spread })
    }

    /// A look runs now (the swings' check trusts it from here): its number.
    pub fn counted(&self) -> u64 {
        self.looked.store(monaka_channel::tick(), Relaxed);
        self.looks.fetch_add(1, Relaxed)
    }

    /// The blade is about to reach `what` (logged at most every half second).
    pub fn reached(&self, what: impl FnOnce() -> String) {
        let before = self.reached.swap(monaka_channel::tick(), Relaxed);
        if monaka_channel::tick().saturating_sub(before) > 500 && self.looks.load(Relaxed) < 100_000 {
            log!("physical melee: the blade is about to reach {}", what());
        }
    }

    /// Whether the blade was about to reach something at the latest look (or the look is not
    /// running: every swing attacks).
    pub fn reaches(&self) -> bool {
        let now = monaka_channel::tick();
        now.saturating_sub(self.looked.load(Relaxed)) > STALE_MS || now.saturating_sub(self.reached.load(Relaxed)) <= REACH_FRESH_MS
    }

    pub fn looks(&self) -> u64 {
        self.looks.load(Relaxed)
    }
}

impl Default for ReachLook {
    fn default() -> Self {
        Self::new()
    }
}

impl Sweep {
    /// Whether box `b` (least x, y, z, then most) is a body's (at least [`LEAST_HEIGHT`] tall) and
    /// the blade comes within [`MARGIN`] of it.
    pub fn touches(&self, b: &[f32; 6]) -> bool {
        let inside = |p: &Vec3| (0..3).all(|k| p[k] >= b[k] - MARGIN && p[k] <= b[k + 3] + MARGIN);
        b.iter().all(|v| v.is_finite()) && b[4] - b[1] >= LEAST_HEIGHT && self.points.iter().any(inside)
    }
}

#[cfg(test)]
mod tests;
