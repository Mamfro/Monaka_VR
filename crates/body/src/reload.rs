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
use monaka_core::protocol::{LEFT_HAND, RIGHT_HAND};
use monaka_core::sync::Locked;
use monaka_pad::actions::{self, Action, Actions};
use monaka_producer::game::{Component, Lack, On, Run, Start, Switch};
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

    /// Logs the hook's counts, the hooks out.
    fn report() {}
}

/// Reloading by hand for game `G`, with motion controls (`manual_reload=0` turns it off, for a
/// test). A game says what it works on top of by struct update.
pub const fn reload<G: Reloads>() -> Component<G> {
    Component {
        name: "reload",
        features: &[Feature::Reload],
        switch: Some(Switch::new("manual_reload", true, "reloading by hand is off")),
        on: on::<G>,
        install: install::<G>,
        arm,
        quiet,
        release: release::<G>,
        ..Component::NONE
    }
}

fn on<G: Reloads>(run: &Run<'_, G>) -> On {
    On::when(run.shared.aim.rig, "aim is not motion controls")
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
    G::report();
    if std::mem::take(&mut *INSTALLED.locked()) {
        log!("reload: {} reloads by the gesture", RELOADS.load(Relaxed));
    }
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
mod tests {
    use super::*;
    use crate::hand_trail::{Lost, TrailRules};
    use monaka_core::camera::from_back_up;
    use monaka_core::protocol::HandPose;
    use std::time::Duration;

    /// A palm at `x` metres along tracking x.
    fn palm(x: f32) -> HandPose {
        HandPose { position: [x, 1.2, -0.3], orientation: [0.0, 0.0, 0.0, 1.0], valid: 1, received_tick: 0 }
    }

    /// Dying Light 1's reading before this ready-made: a gun as the arms held it, then both
    /// controllers' latest places on its trail.
    fn old_dl1(gesture: &mut TouchAway, time: f64, gun: bool, trail: &HandTrails) -> bool {
        if !gun {
            return false;
        }
        let (Some(left), Some(right)) = (trail.position(LEFT_HAND), trail.position(RIGHT_HAND)) else { return false };
        let apart = length([0, 1, 2].map(|k| left[k] - right[k]));
        gesture.feed(time, apart, &TouchAwayRules::RELOAD)
    }

    /// The Chrome games' reading before it: both palms noted within 0.1 s.
    fn old_chrome(gesture: &mut TouchAway, time: f64, trail: &HandTrails) -> bool {
        let (Some(left), Some(right)) = (trail.palm(LEFT_HAND, 0.1), trail.palm(RIGHT_HAND, 0.1)) else { return false };
        let (left, right) = ([left[3], left[7], left[11]], [right[3], right[7], right[11]]);
        let apart = length([0, 1, 2].map(|k| left[k] - right[k]));
        gesture.feed(time, apart, &TouchAwayRules::RELOAD)
    }

    #[test]
    fn each_games_rules_are_the_ones_it_had() {
        assert_eq!(Reloading::FRESH.touch, TouchAwayRules { near: 0.15, away: 0.25, within: 1.0, cooldown: 1.5 });
        assert_eq!((Reloading::FRESH.fresh, Reloading::LATEST.fresh), (Some(0.1), None));
        assert_eq!(Reloading::LATEST.touch, Reloading::FRESH.touch);
    }

    #[test]
    fn the_gesture_reloads_when_each_games_old_reading_did() {
        let origin = from_back_up([0.3, 0.0, 0.95], [0.0, 1.0, 0.0], [4.0, 1.0, -2.0]).unwrap();
        // Dying Light 1's controllers' trail forgets a lost hand; the Chrome games' keeps it.
        let dl1 = HandTrails::new(TrailRules { kept: 0.6, span: 0.07, lost: Lost::Forgotten });
        let chrome = HandTrails::new(TrailRules { kept: 0.8, span: 0.06, lost: Lost::Kept });
        let (mut old_a, mut new_a, mut old_b, mut new_b) = (TouchAway::new(), TouchAway::new(), TouchAway::new(), TouchAway::new());
        let mut reloads = [0; 2];
        // The left hand to the gun and away, over and over; lost for a while; the gun put away for
        // a while; some moments reading stale notes (none taken for a while).
        let left_at = |i: usize| match i % 30 {
            0..=9 => 0.4 - 0.035 * (i % 30) as f32,
            10..=14 => 0.08,
            _ => 0.08 + 0.05 * (i % 30 - 14) as f32,
        };
        for i in 0..400 {
            let lost = (95..105).contains(&i);
            let stale = (200..230).contains(&i);
            let gun = !(300..340).contains(&i);
            if !stale {
                let palms = [(!lost).then(|| palm(left_at(i))), Some(palm(0.0))];
                dl1.note(&origin, &palms);
                chrome.note(&origin, &palms);
            }
            let time = hand_trail::now();
            let held = if gun { Kind::Gun } else { Kind::Nothing };
            let old = old_dl1(&mut old_a, time, gun, &dl1);
            let new = gestured(&mut new_a, time, held, apart(&dl1, Reloading::LATEST.fresh), &Reloading::LATEST.touch);
            assert_eq!(new, old, "Dying Light 1 at {i}");
            reloads[0] += new as usize;
            // The Chrome games read the gesture only with a gun ready: a gun in hand.
            let old = gun && old_chrome(&mut old_b, time, &chrome);
            let new = gestured(&mut new_b, time, held, apart(&chrome, Reloading::FRESH.fresh), &Reloading::FRESH.touch);
            assert_eq!(new, old, "Chrome at {i}");
            reloads[1] += new as usize;
            std::thread::sleep(Duration::from_millis(8));
        }
        assert!(reloads.iter().all(|&n| n >= 2), "the timeline reloads several times: {reloads:?}");
        assert_eq!((old_a, old_b), (new_a, new_b), "the gestures end in the same state");
    }

    #[test]
    fn an_empty_or_other_hand_feeds_nothing() {
        let mut gesture = TouchAway::new();
        let rules = TouchAwayRules::RELOAD;
        for held in [Kind::Nothing, Kind::Melee, Kind::Bow, Kind::Throwable] {
            assert!(!gestured(&mut gesture, 0.0, held, Some(0.1), &rules));
        }
        assert_eq!(gesture, TouchAway::new(), "no touch noted");
        assert!(!gestured(&mut gesture, 0.0, Kind::Gun, None, &rules), "a hand not placed");
        assert!(!gestured(&mut gesture, 0.1, Kind::Gun, Some(0.1), &rules));
        assert!(gestured(&mut gesture, 0.3, Kind::Gun, Some(0.3), &rules));
    }
}
