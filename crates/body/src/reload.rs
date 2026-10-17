//! Reloading by hand, as a ready-made (`reload`): with a gun in the aiming hand, the left controller
//! brought to the gun hand's and taken away again ([`monaka_core::gesture::TouchAway`]) presses the
//! game's reload ([`Action::Reload`], through the actions provider), as the reload button would.
//! The controllers count, not the arms: with a gun held two-handed the off arm rides the gun.
//!
//! A game implements [`Reloads`] (how fresh its controllers on the hand rig's trail must be, and
//! the hook giving the moment a reload can be taken) and lists [`reload`]. At that moment (its
//! input pass, its gun's update while the gun is ready) it calls [`update`], which feeds the
//! gesture and presses the reload when the gesture finishes. The gesture is read only then, so one
//! made while the game could not reload (the gun firing) is not used up.

use crate::hand_trail::{self, HandTrails};
use crate::held::{self, HeldItems};
use crate::held::Kind;
use monaka_core::feature::Feature;
use monaka_core::gesture::{TouchAway, TouchAwayRules};
use monaka_core::math::{Vec3, length};
use monaka_contract::protocol::{LEFT_HAND, RIGHT_HAND};
use monaka_core::sync::Locked;
use monaka_pad::actions::{self, Action, Actions};
use monaka_producer::game::{Component, Lack, Need, Start, Switch, lines};
use monaka_producer::log;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

/// How a game's reload by hand is read and pressed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reloading {
    /// The gesture: the left controller within 0.15 m of the right touches the gun, then more than
    /// 0.25 m from it within 1 s reloads; not again for 1.5 s.
    pub touch: TouchAwayRules,
    /// How recently each controller's palm must have been noted (seconds); none: its latest place,
    /// however old (a trail that forgets a lost hand).
    pub fresh: Option<f64>,
    /// How long the reload is pressed (milliseconds; a game pressing it its own way may keep its
    /// own pulse).
    pub press_ms: u64,
}

impl Reloading {
    /// Each palm noted within 0.1 s (the Chrome games).
    pub const FRESH: Self = Self { touch: TouchAwayRules::RELOAD, fresh: Some(0.1), press_ms: 100 };
    /// Each controller's latest place (Dying Light 1, whose trail forgets a hand it loses).
    pub const LATEST: Self = Self { fresh: None, ..Self::FRESH };
}

/// A game's part in reloading by hand: what the hands hold ([`HeldItems`]), its actions
/// ([`Actions`]), and the hook that gives the moment it can take a
/// reload (which calls [`update`]).
pub trait Reloads: HeldItems + Actions {
    const RELOADING: Reloading = Reloading::FRESH;

    /// Hooks the moment a reload can be taken, where the game's own input pass does not give it.
    fn install(_start: &mut Start<'_, Self>) -> Result<(), Lack> {
        Ok(())
    }

    /// The hook's state put back once its hooks are out.
    fn release() {}
    /// The hook's counts, for the stop's log (empty: nothing to say).
    fn summary() -> String {
        String::new()
    }
}

/// Reloading by hand for game `G`, with motion controls (`manual_reload=0` turns it off, for a
/// test). A game says what it works on top of by struct update.
pub const fn reload<G: Reloads>() -> Component<G> {
    Component {
        needs: &[Need::Feature(Feature::HandRig)],
        name: "reload",
        features: &[Feature::Reload],
        switch: Some(Switch::new("manual_reload", true, "reloading by hand is off")),
        install: install::<G>,
        arm,
        quiet,
        release: release::<G>,
        summary: Some(summary::<G>),
        ..Component::NONE
    }
}

/// The gesture, the aiming hand, and whether reloads may be pressed (the component on).
struct State {
    gesture: TouchAway,
    aim: usize,
    on: bool,
}

static STATE: Mutex<State> = Mutex::new(State { gesture: TouchAway::new(), aim: RIGHT_HAND, on: false });
static RELOADS: AtomicU64 = AtomicU64::new(0);
static INSTALLED: Mutex<bool> = Mutex::new(false);

fn install<G: Reloads>(start: &mut Start<'_, G>) -> Result<(), Lack> {
    G::install(start)?;
    STATE.locked().aim = start.shared.aim.side;
    *INSTALLED.locked() = true;
    Ok(())
}

fn arm() {
    STATE.locked().on = true;
}

fn quiet() {
    STATE.locked().on = false;
}

fn release<G: Reloads>() {
    G::release();
    *INSTALLED.locked() = false;
}

fn summary<G: Reloads>() -> String {
    lines(&[G::summary(), format!("reload: {} reloads by the gesture", RELOADS.load(Relaxed))])
}

/// How far apart the left and right controllers are on `trail`: their latest places, or (with
/// `fresh`) their palms noted within that many seconds. None unless both are there.
pub fn apart(trail: &HandTrails, fresh: Option<f64>) -> Option<f32> {
    let place = |side: usize| match fresh {
        Some(fresh) => trail.palm(side, fresh).map(|palm| [palm[3], palm[7], palm[11]]),
        None => trail.position(side),
    };
    let (left, right): (Vec3, Vec3) = (place(LEFT_HAND)?, place(RIGHT_HAND)?);
    Some(length([0, 1, 2].map(|k| left[k] - right[k])))
}

/// One moment's reading: with `held` in the aiming hand and the controllers `apart` (none: not both
/// placed) at `time`, whether the gesture just finished. Only a gun's hand feeds the gesture.
pub fn gestured(gesture: &mut TouchAway, time: f64, held: Kind, apart: Option<f32>, rules: &TouchAwayRules) -> bool {
    held == Kind::Gun && apart.is_some_and(|apart| gesture.feed(time, apart, rules))
}

/// At a moment game `G` can take a reload (on its thread): the gesture fed, and the reload pressed
/// when it finishes. Whether a reload was pressed.
pub fn update<G: Reloads>() -> bool {
    let aim = {
        let state = STATE.locked();
        if !state.on {
            return false;
        }
        state.aim
    };
    if !actions::can(Action::Reload) {
        return false;
    }
    let held = held::now::<G>(aim).kind;
    let apart = if held == Kind::Gun { G::trail().and_then(|trail| apart(trail, G::RELOADING.fresh)) } else { None };
    if !gestured(&mut STATE.locked().gesture, hand_trail::now(), held, apart, &G::RELOADING.touch) {
        return false;
    }
    let pressed = actions::press(Action::Reload, G::RELOADING.press_ms);
    let n = RELOADS.fetch_add(1, Relaxed) + 1;
    if n <= 20 {
        log!("reload: {n} by the gesture{}", if pressed { "" } else { ", not taken" });
    }
    pressed
}

#[cfg(test)]
mod tests;
