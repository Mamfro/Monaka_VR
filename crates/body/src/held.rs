//! The held-item provider: what each hand holds, for the hand rig and the features riding on it
//! (swinging, throwing, the bow, shots from the barrel, reloading).
//!
//! Its kind ([`Kind`]) is the hand rig's own question ([`HandRig::held`]: a gun sits in the palm by
//! its grip, a gripping hand keeps the game's fingers), and so are the item ([`HandRig::item`]) and
//! the element it hangs from ([`HandRig::holder`]). The rest a game says through [`HeldItems`]: how
//! both hands hold it ([`Grip`]), its line in the hand ([`Line`]: a blade toward its tip with its
//! edge, a barrel toward its muzzle, an arrow from its nock toward the bow) and its model (its own
//! bones and points). A feature reads what a hand holds at once with [`now`], with when it came to
//! hand ([`HeldItem::drawn_within`]: just drawn or switched), and where it is in the world once the
//! hand rig has posed the arms with [`in_world`]. Code without the game type reads what the hands
//! held at the rig's latest posing with [`latest`] (and the aim hand as melee takes it with
//! [`wielded`]).

use crate::Skeleton;
use crate::hand_rig::HandRig;
use monaka_core::camera::Mat34;
use monaka_core::math::{Vec3, unit};
use monaka_contract::protocol::{LEFT_HAND, RIGHT_HAND};
use monaka_core::sync::Locked;
use std::sync::Mutex;

/// The kind of what a hand holds: the held-item provider's kind ([`HandRig::held`]; the rest of the
/// item, [`HeldItems`]). Its rules: [`Kind::grips`], `crate::melee::swing_rules`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Gun,
    /// A blunt weapon (or one of unknown kind): a swing any way attacks.
    Melee,
    /// A sharp one: only a cut attacks (`monaka_core::swing::SwingRules::BLADE`).
    Blade,
    Nothing,
    /// A bow, in the hand that holds it out (the other draws its string, empty).
    Bow,
    /// Something thrown from the hand: a knife, a grenade, a bottle.
    Throwable,
    /// A tool worked by hand: a lockpick, a lighter, a map.
    Tool,
}

impl Kind {
    /// Whether a hand holding this keeps the game's grip (its fingers as the game animates them,
    /// never tracked): a weapon, a throwable or a tool does. An empty hand does not, nor a bow's:
    /// the player's own grip on the controller holds the bow.
    pub const fn grips(self) -> bool {
        !matches!(self, Kind::Nothing | Kind::Bow)
    }

    /// Whether a hand holding this fights as melee does: a melee weapon, or bare fists.
    pub const fn melee(self) -> bool {
        matches!(self, Kind::Melee | Kind::Blade | Kind::Nothing)
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

/// A game's held items: what each hand holds beyond its kind and item ([`HandRig::held`],
/// [`HandRig::item`]). A feature that reads them bounds its game on this, so a game without it does
/// not compile. Every method is asked on the game's thread (a model is read through the game's own
/// functions).
pub trait HeldItems: HandRig {
    /// How the game's item models are read (their bones and points).
    type Model: Skeleton;
    /// Its line in its holder's frame, where the game knows it (it may read the item's model, so a
    /// feature asks only when it uses it).
    fn line(side: usize) -> Option<Line>;
    /// Its model: a weapon's blade tip, a bow's string and nock.
    fn model(side: usize) -> Option<Self::Model>;
    /// Where the game itself put what hand `side` holds in the world, for a game that places the
    /// item (a gun on its controller, the arms hanging from it) rather than the hand rig's holder:
    /// the holder, its line there, when it was put. None: where the rig posed it ([`in_world`]).
    fn placed(_side: usize) -> Option<InWorld> {
        None
    }
}

/// An item model that reads nothing, for a game whose items' models are not read
/// ([`HeldItems::Model`]).
pub struct NoModel;

impl Skeleton for NoModel {
    fn id(&self) -> usize {
        0
    }

    fn count(&self) -> i32 {
        0
    }

    fn name(&self, _: i32) -> String {
        String::new()
    }

    fn parent(&self, _: i32) -> i32 {
        -1
    }

    fn world(&self, _: i32) -> Option<Mat34> {
        None
    }

    fn set_world(&self, _: i32, _: &Mat34) {}

    fn is_bone(&self, _: i32) -> bool {
        false
    }
}

/// What a hand holds, as a feature reads it ([`now`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeldItem {
    pub kind: Kind,
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
    sides: [Option<(Kind, Option<u64>, u64)>; 2],
}

impl Draws {
    pub const fn new() -> Self {
        Self { sides: [None; 2] }
    }

    /// Hand `side` holds `kind` (item `key`, if any) at tick `now`: when it came to hand.
    pub fn note(&mut self, side: usize, kind: Kind, key: Option<u64>, now: u64) -> u64 {
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

/// What both hands held at the hand rig's latest posing, with their own draws (the rig's notes do
/// not move [`now`]'s).
struct Latest {
    sides: [Option<HeldItem>; 2],
    draws: Draws,
}

static LATEST: Mutex<Latest> = Mutex::new(Latest { sides: [None; 2], draws: Draws::new() });

/// The hand rig notes what game `G`'s hands hold at a posing (on the game's thread, with the rest
/// of what it captures).
pub(crate) fn note_latest<G: HandRig>(kinds: [Kind; 2]) {
    let now = monaka_channel::tick();
    let items = [LEFT_HAND, RIGHT_HAND].map(G::item);
    LATEST.locked().note(kinds, items, now);
}

impl Latest {
    /// Both hands as held at tick `now`.
    fn note(&mut self, kinds: [Kind; 2], items: [Option<Item>; 2], now: u64) {
        for side in [LEFT_HAND, RIGHT_HAND] {
            let (kind, item) = (kinds[side], items[side]);
            let since = self.draws.note(side, kind, item.map(|i| i.key), now);
            self.sides[side] = Some(HeldItem { kind, item, since });
        }
    }
}

/// What hand `side` held at the hand rig's latest posing, read without the game type (any thread);
/// none before the rig's first posing.
pub fn latest(side: usize) -> Option<HeldItem> {
    LATEST.locked().sides[side.min(1)]
}

/// What the aim hand (`aim`) held at the hand rig's latest posing as melee takes it: a throwable as
/// the melee weapon it is until it leaves, and, with a bow in either hand, a gun (a bow swings
/// nothing: its draw is the attack). None before the rig's first posing.
pub fn wielded(aim: usize) -> Option<Kind> {
    let sides = LATEST.locked().sides;
    wielded_of(sides, aim)
}

/// [`wielded`] from both hands' items.
fn wielded_of(sides: [Option<HeldItem>; 2], aim: usize) -> Option<Kind> {
    let kind = sides[aim.min(1)]?.kind;
    if sides.iter().flatten().any(|side| side.kind == Kind::Bow) {
        return Some(Kind::Gun);
    }
    Some(if kind == Kind::Throwable { Kind::Melee } else { kind })
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

/// Where what hand `side` of game `G` holds is now, in the world: where the game put it itself
/// ([`HeldItems::placed`]), else its holder as the hand rig last posed the arms
/// ([`crate::hand_rig::posed`]: read back after its write) and its line there. On the game's
/// thread, after the posing moment (its line may read the item's model). None before the rig has
/// posed the arms, or where the game names no holder for that hand.
pub fn in_world<G: HeldItems>(side: usize) -> Option<InWorld> {
    if let Some(placed) = G::placed(side) {
        return Some(placed);
    }
    let posed = crate::hand_rig::posed()?;
    let holder = posed.holders[side.min(1)]?;
    Some(InWorld { holder, line: G::line(side).and_then(|line| line.in_world(&holder)), posed: posed.at })
}

#[cfg(test)]
mod tests;
