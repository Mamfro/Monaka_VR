//! The bow by hand, as a ready-made (`bow`): the bow in the left hand, the string pulled back by
//! the right. How far the string hand is behind the nock, along the bow's line, sets the draw, and
//! the arrow leaves along that line instead of along the look.
//!
//! A game implements the fire hook (`crate::launch::Fires`: each arrow asked of the ready-made's
//! answer), the draw (`crate::launch::Draws`: where it takes how far the bow is drawn, as the arrow
//! leaves or at each of the bow's updates, and whether it is drawing), the held item (the bow in the
//! left hand, its model) and [`Bows`]: its rules ([`Bowing`]), where the string hand is, and how its
//! bow model keeps what is placed on it ([`Keeps`]). At its posing moment, after the hand rig, it
//! calls [`posed`]: the bow's line is read off the bow model there, the bow turned toward the left
//! reticle, and the model's string and nock go back as far as the hand is.
//!
//! The bow's line is the bow model's own, read from its shape alone ([`nock_axis`]): the arrow's
//! nock sits on the string, behind the bow bone it hangs from, so the arrow runs from the nock to
//! that bone, along the nock's axis lying that way. The line is the nock's as the draw began
//! (`from_nock`): a draw that moves the bow bone does not move the arrow's line.

mod from_nock;

use crate::held::{HeldItems, Line};
use crate::launch::{self, Draws, Fired, Fires, Shot, Weapon};
use crate::Skeleton;
use monaka_core::camera::Mat34;
use monaka_core::feature::Feature;
use monaka_core::gesture::PullRules;
use monaka_core::math::{self, Vec3};
use monaka_core::sync::Locked;
use monaka_producer::game::{Component, Lack, Need, Start, Switch, lines};
use monaka_producer::log;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::time::{Duration, Instant};

/// The bow's rules: the arrow leaves from the nock as the draw began (from the bow's root while not
/// drawn) along the nock's axis. The bow is turned in the hand about its root so the arrow points
/// at the left reticle, when within `most_turn` degrees of it. While drawn, the bow bone stays where
/// it rests in the hand (a draw that moves it does not) and the nock and the string go straight
/// back along the line as far as the string hand is behind the nock. The draw's share is the pull's
/// ([`PullRules::share`]), given whenever the game takes the draw; a pull under the least keeps the
/// game's draw. The model's elements are found by their names (exactly, else without case; the nock
/// else by an element whose name holds `nock_like`, where there is one).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bowing {
    pub pull: PullRules,
    pub most_turn: f32,
    pub nock: &'static str,
    pub nock_like: Option<&'static str>,
    pub string: &'static str,
    /// The bone the bow bends from (its limbs hang off it).
    pub bow: &'static str,
}

impl Bowing {
    /// The bow model's element named `name`: exactly, else without case.
    pub fn element(model: &impl Skeleton, name: &str) -> Option<i32> {
        find(model, name).or_else(|| find_any_case(model, name))
    }

    /// The bow model's nock: by its name, else by an element whose name holds `nock_like`.
    pub fn nock_in(&self, model: &impl Skeleton) -> Option<i32> {
        Self::element(model, self.nock).or_else(|| find_like(model, self.nock_like?))
    }
}

/// How a game's bow model keeps the elements the bow by hand places.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keeps {
    /// A world matrix set holds until the game poses the model again: the root is turned toward
    /// the reticle, and each element placed in the world.
    World,
    /// The engine works the world matrices out from the locals after the posing, and the model
    /// then hangs from the left hand's holder as the hand rig placed it (it may hang elsewhere while
    /// it is read): the line goes from that holder, the turn is carried by the bow bone, and each
    /// element is placed against its parent ([`Bows::set_local`]).
    Local,
}

/// A game's part in the bow by hand: its fire hook and its draw (the providers the bow is loosed
/// through), the bow it holds ([`HeldItems`]), its rules, where the string hand is, and how its
/// bow model keeps an element placed on it.
pub trait Bows: Fires + Draws + HeldItems {
    const BOWING: Bowing;
    /// How the bow model keeps what is placed on it: in the world by default.
    const KEEPS: Keeps = Keeps::World;

    /// Sets element `element` of the bow model against its parent (its children with it); false
    /// where the game cannot. Read where the model keeps its elements against their parents
    /// ([`Keeps::Local`]).
    fn set_local(_model: &Self::Model, _element: i32, _matrix: &Mat34) -> bool {
        false
    }

    /// The hooks' counts, for the stop's log (empty: nothing to say).
    fn summary() -> String {
        String::new()
    }
}

/// A draw the game has not confirmed for this long is over; a bow not seen held for this long is
/// put away.
const DRAW_FRESH: Duration = Duration::from_millis(200);

/// The bow by hand for game `G`, with motion controls (`bow_by_hand=0`: the game's shot along the
/// view, for a test; `bow_string=0`: the bow model's string as the game draws it). A game says what
/// it works on top of by struct update.
pub const fn bow<G: Bows>() -> Component<G> {
    Component {
        needs: &[Need::Feature(Feature::HandRig)],
        name: "bow",
        features: &[Feature::Bow],
        switch: Some(Switch::new("bow_by_hand", true, "the game's bow")),
        install: install::<G>,
        release,
        summary: Some(summary::<G>),
        ..Component::NONE
    }
}

static INSTALLED: AtomicBool = AtomicBool::new(false);
/// The bow model's string is drawn back as far as the hand is.
static STRING_FOLLOWS: AtomicBool = AtomicBool::new(true);
/// When the bow was last seen held, the bow by hand on.
static HELD_AT: Mutex<Option<Instant>> = Mutex::new(None);

fn install<G: Bows>(start: &mut Start<'_, G>) -> Result<(), Lack> {
    <G as Fires>::install(start)?;
    <G as Draws>::install(start)?;
    STRING_FOLLOWS.store(start.options.switch("bow_string", true), Relaxed);
    launch::aim_shots(Weapon::Bow, Some(shot));
    launch::draw_by(Some(draw::<G>));
    INSTALLED.store(true, Relaxed);
    log!("bow by hand: the string drawn along the bow by the right hand, the arrow loosed along the bow");
    Ok(())
}

fn release() {
    launch::aim_shots(Weapon::Bow, None);
    launch::draw_by(None);
    INSTALLED.store(false, Relaxed);
}

fn summary<G: Bows>() -> String {
    lines(&[<G as Bows>::summary(), from_nock::summary()])
}

/// Whether the bow by hand is on.
pub fn installed() -> bool {
    INSTALLED.load(Relaxed)
}

/// Whether the left hand holds a bow, seen at the latest posings, with the bow by hand on.
pub fn held() -> bool {
    installed() && HELD_AT.locked().is_some_and(|at| at.elapsed() < DRAW_FRESH)
}

fn note_held() {
    *HELD_AT.locked() = Some(Instant::now());
}

/// Whether the bow is drawing now (the game saw it drawing lately).
fn drawing<G: Draws>() -> bool {
    G::drawn_ago().is_some_and(|ago| ago < DRAW_FRESH)
}

/// Game `G`'s posing moment, after the hand rig (`placed`: it placed the arms this time): the bow's
/// line read off the bow model, the bow turned toward the reticle, and, while it is drawn, its
/// string and nock placed as far back as the string hand is. `reticle`: where the viewer's left
/// reticle sits in the world. On the game's thread.
pub fn posed<G: Bows>(placed: bool, reticle: Option<Vec3>) {
    if installed() {
        from_nock::posed::<G>(&G::BOWING, placed, reticle, STRING_FOLLOWS.load(Relaxed));
    }
}

fn shot(fired: &Fired) -> Option<Shot> {
    from_nock::shot(fired)
}

fn draw<G: Bows>() -> Option<f32> {
    from_nock::draw(&G::BOWING)
}

// ---- The bow model's shape ----

/// The nock must be this far behind the bow it hangs from (metres)...
const NOCK_DEPTH: f32 = 0.05;
/// ...with one of its axes this close to that way (cosine).
const NOCK_ALONG: f32 = 0.8;

/// The first element of `model` named `name` exactly.
pub fn find(model: &impl Skeleton, name: &str) -> Option<i32> {
    (0..model.count()).find(|&e| model.name(e) == name)
}

/// The first element of `model` named `name` without case.
pub fn find_any_case(model: &impl Skeleton, name: &str) -> Option<i32> {
    (0..model.count()).find(|&e| model.name(e).eq_ignore_ascii_case(name))
}

/// The first element of `model` whose name, without case, holds `part`.
pub fn find_like(model: &impl Skeleton, part: &str) -> Option<i32> {
    (0..model.count()).find(|&e| model.name(e).to_ascii_lowercase().contains(part))
}

/// Which of the nock's axes the arrow lies along and which way along it is forward, from the model
/// alone: the nock sits on the string, behind the bow it hangs from (its parent), so forward runs
/// from the nock to that.
pub fn nock_axis(model: &impl Skeleton, nock: i32) -> Option<(usize, f32)> {
    arrow_axis(&model.world(nock)?, &model.world(model.parent(nock))?)
}

/// [`nock_axis`] from the nock's world matrix `at` and that of the bow it hangs from.
pub fn arrow_axis(at: &Mat34, bow: &Mat34) -> Option<(usize, f32)> {
    let forward = [0, 1, 2].map(|k| bow[3 + 4 * k] - at[3 + 4 * k]);
    if math::length(forward) < NOCK_DEPTH {
        return None;
    }
    let forward = unit_or_same(forward);
    let along = [0, 1, 2].map(|c| math::dot(unit_or_same([at[c], at[4 + c], at[8 + c]]), forward));
    let axis = (0..3).max_by(|&a, &b| along[a].abs().total_cmp(&along[b].abs()))?;
    (along[axis].abs() >= NOCK_ALONG).then(|| (axis, along[axis].signum()))
}

/// The arrow's line of a bow model in its root's frame, from the nock at `at` along its axis `axis`
/// that way `sign` ([`nock_axis`]); `grip` is the root (both world matrices).
pub fn nock_line(at: &Mat34, grip: &Mat34, axis: usize, sign: f32) -> Line {
    let way = unit_or_same([at[axis], at[4 + axis], at[8 + axis]]).map(|c| c * sign);
    Line { from: into(grip, [0, 1, 2].map(|c| at[3 + 4 * c] - grip[3 + 4 * c])), along: into(grip, way), length: None, edge: None }
}

/// `v` made one long, or as it is where it has no length.
fn unit_or_same(v: Vec3) -> Vec3 {
    math::unit(v).unwrap_or(v)
}

/// A world direction in `frame`'s terms (its axes made square and one long).
fn into(frame: &Mat34, v: Vec3) -> Vec3 {
    monaka_core::camera::rigid_axes(frame).map(|axis| math::dot(axis, v))
}

#[cfg(test)]
mod tests;
