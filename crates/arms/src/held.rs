//! The held-item provider: what each hand holds, for the hand rig and the features riding on it
//! (swinging, throwing, the bow, shots from the barrel, reloading).
//!
//! Its kind ([`Held`]) is the hand rig's own question ([`HandRig::held`]: a gun sits in the palm by
//! its grip, a gripping hand keeps the game's fingers), and so is the element it hangs from
//! ([`HandRig::holder`]). The rest a game says through [`HeldItems`]: which item it is ([`Item`]),
//! how both hands hold it ([`Grip`]), its line in the hand ([`Line`]: a blade toward its tip with
//! its edge, a barrel toward its muzzle, an arrow from its nock toward the bow) and its model (its
//! own bones and points). A feature reads what a hand holds at once with [`now`], with when it came
//! to hand ([`HeldItem::drawn_within`]: just drawn or switched), and where it is in the world once
//! the hand rig has posed the arms with [`in_world`].

use crate::Skeleton;
use crate::hand_rig::HandRig;
use crate::melee::Held;
use monaka_core::camera::Mat34;
use monaka_core::math::{Vec3, unit};
use monaka_core::sync::Locked;
use std::sync::Mutex;

impl Held {
    /// Whether a hand holding this keeps the game's grip (its fingers as the game animates them,
    /// never tracked): a weapon, a throwable or a tool does. An empty hand does not, nor a bow's:
    /// the player's own grip on the controller holds the bow.
    pub const fn grips(self) -> bool {
        !matches!(self, Held::Nothing | Held::Bow)
    }
}

/// How the hands hold an item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grip {
    /// As the game animates it: the hand rig tells from the pose whether the off hand is on it (a
    /// rifle's front hand), each update.
    Measured,
    /// In this hand alone: the other hand follows its own controller.
    One,
    /// In both hands: the off hand rides it wherever the game holds it.
    Both,
    /// In both hands, each on its own controller however close the game animates them: a bow and
    /// its string, a throw under way.
    Apart,
}

/// An item's line in the frame of its holder ([`HandRig::holder`]), in metres: where it starts,
/// which way it runs, how far to its end, and the side it cuts with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line {
    /// A blade's hilt, a barrel's breech, an arrow's nock as the string rests.
    pub from: Vec3,
    /// Which way it runs (unit): a blade toward its tip, a barrel toward its muzzle, an arrow
    /// toward the bow.
    pub along: Vec3,
    /// How far along it its end is (a blade's tip, the muzzle); none where the game does not say
    /// (a feature then takes its own length).
    pub length: Option<f32>,
    /// The side a blade cuts with (unit, square to `along`); none for an item without an edge.
    pub edge: Option<Vec3>,
}

/// A [`Line`] in the world, as its holder stands now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldLine {
    pub from: Vec3,
    pub along: Vec3,
    pub length: Option<f32>,
    pub edge: Option<Vec3>,
}

impl WorldLine {
    /// The point `distance` along it.
    pub fn at(&self, distance: f32) -> Vec3 {
        [0, 1, 2].map(|k| self.from[k] + self.along[k] * distance)
    }

    /// Its end (a blade's tip, the muzzle), where its length is known.
    pub fn end(&self) -> Option<Vec3> {
        self.length.map(|length| self.at(length))
    }
}

/// `v`, given in `frame`'s terms, in the world: `frame`'s axes each made one long (a game's
/// matrices may carry a scale), none where one has no length.
pub fn out_of(frame: &Mat34, v: Vec3) -> Option<Vec3> {
    let axes = [0, 1, 2].map(|c| unit([frame[c], frame[4 + c], frame[8 + c]]));
    let [Some(x), Some(y), Some(z)] = axes else { return None };
    Some([0, 1, 2].map(|k| x[k] * v[0] + y[k] * v[1] + z[k] * v[2]))
}

impl Line {
    /// The line in the world, its holder at `holder` (its axes made one long); none where the holder
    /// has an axis of no length or a number not finite.
    pub fn in_world(&self, holder: &Mat34) -> Option<WorldLine> {
        let offset = out_of(holder, self.from)?;
        let from = [0, 1, 2].map(|k| holder[3 + 4 * k] + offset[k]);
        let along = out_of(holder, self.along)?;
        let edge = match self.edge {
            Some(edge) => Some(out_of(holder, edge)?),
            None => None,
        };
        from.iter().chain(&along).all(|v| v.is_finite()).then_some(WorldLine { from, along, length: self.length, edge })
    }

    /// The point of the line level with its holder's origin (the one nearest it).
    pub fn level_with_origin(&self) -> Vec3 {
        let along = monaka_core::math::dot(self.from, self.along);
        [0, 1, 2].map(|c| self.from[c] - self.along[c] * along)
    }
}

/// What one hand holds beyond its kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Item {
    /// Which item: the same while it stays in the hand (its object's address, the game's own item
    /// id). Another key is another item, just drawn.
    pub key: u64,
    pub grip: Grip,
}

/// A game's held items: what each hand holds beyond its kind ([`HandRig::held`]). A feature that
/// reads them bounds its game on this, so a game without it does not compile. Every method is
/// asked on the game's thread (a model is read through the game's own functions).
pub trait HeldItems: HandRig {
    /// How the game's item models are read (their bones and points).
    type Model: Skeleton;
    /// What hand `side` holds, when it holds something the game describes.
    fn item(side: usize) -> Option<Item>;
    /// Its line in its holder's frame, where the game knows it (it may read the item's model, so a
    /// feature asks only when it uses it).
    fn line(side: usize) -> Option<Line>;
    /// Its model: a weapon's blade tip, a bow's string and nock.
    fn model(side: usize) -> Option<Self::Model>;
}

/// What a hand holds, as a feature reads it ([`now`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeldItem {
    pub kind: Held,
    pub item: Option<Item>,
    /// When it came to hand (`monaka_channel::tick`): drawn, switched, or (for an empty hand) put
    /// away. What a hand held when first asked counts as come to hand then.
    pub since: u64,
}

impl HeldItem {
    /// Whether it came to hand less than `ms` before `now` (ticks).
    pub fn drawn_within(&self, now: u64, ms: u64) -> bool {
        now.saturating_sub(self.since) < ms
    }

    /// How both hands hold it (an empty hand: as the game animates them).
    pub fn grip(&self) -> Grip {
        self.item.map_or(Grip::Measured, |item| item.grip)
    }
}

/// When each hand's item came to hand: a change of kind or of item key is a new one.
#[derive(Clone, Copy, Debug, Default)]
pub struct Draws {
    sides: [Option<(Held, Option<u64>, u64)>; 2],
}

impl Draws {
    pub const fn new() -> Self {
        Self { sides: [None; 2] }
    }

    /// Hand `side` holds `kind` (item `key`, if any) at tick `now`: when it came to hand.
    pub fn note(&mut self, side: usize, kind: Held, key: Option<u64>, now: u64) -> u64 {
        let side = side.min(1);
        match self.sides[side] {
            Some((was, was_key, since)) if was == kind && was_key == key => since,
            _ => {
                self.sides[side] = Some((kind, key, now));
                now
            }
        }
    }
}

static DRAWS: Mutex<Draws> = Mutex::new(Draws::new());

/// What hand `side` of game `G` holds now (on the game's thread), with when it came to hand.
pub fn now<G: HeldItems>(side: usize) -> HeldItem {
    let kind = G::held(side);
    let item = G::item(side);
    let since = DRAWS.locked().note(side, kind, item.map(|i| i.key), monaka_channel::tick());
    HeldItem { kind, item, since }
}

/// Where a hand's item is in the world ([`in_world`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InWorld {
    /// Its holder as the hand rig left it.
    pub holder: Mat34,
    /// Its line there, where the game knows one.
    pub line: Option<WorldLine>,
    /// When the rig posed the arms (`monaka_channel::tick`): a feature takes it as fresh enough
    /// or not (the rig may leave the arms to the game a while, as it does while climbing).
    pub posed: u64,
}

/// Where what hand `side` of game `G` holds is now, in the world: its holder as the hand rig last
/// posed the arms ([`crate::hand_rig::posed`]: read back after its write) and its line there. On the
/// game's thread, after the posing moment (its line may read the item's model). None before the rig
/// has posed the arms, or where the game names no holder for that hand.
pub fn in_world<G: HeldItems>(side: usize) -> Option<InWorld> {
    let posed = crate::hand_rig::posed()?;
    let holder = posed.holders[side.min(1)]?;
    Some(InWorld { holder, line: G::line(side).and_then(|line| line.in_world(&holder)), posed: posed.at })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_empty_hand_and_a_bow_hand_take_tracked_fingers() {
        // Every kind the hand rig knew before: only an empty hand was free.
        for kind in [Held::Gun, Held::Melee, Held::Blade, Held::Nothing] {
            assert_eq!(!kind.grips(), kind == Held::Nothing, "{kind:?}");
        }
        assert!(!Held::Bow.grips());
        assert!(Held::Throwable.grips() && Held::Tool.grips());
    }

    #[test]
    fn a_new_kind_or_item_comes_to_hand_and_the_same_stays() {
        let mut draws = Draws::new();
        assert_eq!(draws.note(1, Held::Nothing, None, 100), 100, "first asked: then");
        assert_eq!(draws.note(1, Held::Nothing, None, 400), 100);
        assert_eq!(draws.note(1, Held::Melee, Some(7), 500), 500, "drawn");
        assert_eq!(draws.note(1, Held::Melee, Some(7), 900), 500);
        assert_eq!(draws.note(1, Held::Melee, Some(8), 950), 950, "another weapon of the same kind: switched");
        assert_eq!(draws.note(0, Held::Bow, Some(3), 960), 960, "each hand its own");
        assert_eq!(draws.note(1, Held::Nothing, None, 1000), 1000, "put away");
        let held = HeldItem { kind: Held::Melee, item: None, since: 500 };
        assert!(held.drawn_within(600, 200) && !held.drawn_within(700, 200));
        assert_eq!(held.grip(), Grip::Measured);
    }

    /// A holder turned a quarter about up (its x along the world's -z), scaled by 2, at (1, 2, 3).
    const HOLDER: Mat34 = [0.0, 0.0, 2.0, 1.0, 0.0, 2.0, 0.0, 2.0, -2.0, 0.0, 0.0, 3.0];

    #[test]
    fn a_line_goes_into_the_world_on_its_holder_unscaled() {
        let blade = Line { from: [0.0, 0.1, 0.0], along: [1.0, 0.0, 0.0], length: Some(0.8), edge: Some([0.0, 0.0, 1.0]) };
        let world = blade.in_world(&HOLDER).unwrap();
        let close = |a: Vec3, b: Vec3| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-6);
        assert!(close(world.from, [1.0, 2.1, 3.0]), "the hilt 0.1 m up the holder, not 0.2: {:?}", world.from);
        assert!(close(world.along, [0.0, 0.0, -1.0]), "{:?}", world.along);
        assert!(close(world.edge.unwrap(), [1.0, 0.0, 0.0]));
        assert!(close(world.end().unwrap(), [1.0, 2.1, 2.2]));
        assert!(close(world.at(0.5), [1.0, 2.1, 2.5]));
        let flat = [0.0; 12];
        assert!(blade.in_world(&flat).is_none(), "a holder with no axes has no line");
    }

    #[test]
    fn a_line_starts_level_with_its_holders_origin() {
        // An arrow 0.2 m behind and 0.05 m beside the holder, running along x.
        let arrow = Line { from: [-0.2, 0.05, 0.0], along: [1.0, 0.0, 0.0], length: None, edge: None };
        assert_eq!(arrow.level_with_origin(), [0.0, 0.05, 0.0]);
    }
}
