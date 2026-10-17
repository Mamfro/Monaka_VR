//! What leaves the hands: the fire hook ([`Fires`]: a shot the player's bow or gun fires), the
//! bow's draw ([`Draws`]: how far it is drawn as it looses) and the throw release ([`Throws`]: an
//! item the player throws), three providers, and the ready-mades' answers to them.
//!
//! Each turns the game's own launch into Monaka's. A game hooks where its player's projectiles are
//! fired and asks [`shot`] for each one: the bow or gun ready-made, having said once how it aims
//! ([`aim_shots`]), answers where it leaves from, which way and how fast ([`Shot`]). A game hooks
//! its throw likewise: when the player lets go it asks [`let_go`] for the throw the hands made, and
//! as the item leaves [`start`] and [`velocity`] for where it starts and how it flies, answered by
//! the throw ready-made ([`throw_by`]). Without an answer the game's own launch stands. Every
//! answer is asked on the thread the game launches on.

use monaka_core::math::{Vec3, length};
use monaka_core::sync::Locked;
use monaka_producer::game::{Game, Lack, Start};
use std::sync::Mutex;
use std::time::Duration;

/// Something leaving, as the game launches it: where from, and its velocity (world, m/s).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Flight {
    pub from: Vec3,
    pub velocity: Vec3,
}

// ---- The fire hook ----

/// What fired a shot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Weapon {
    Bow,
    Gun,
}

/// One projectile of the player's shot, as the game fires it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fired {
    pub weapon: Weapon,
    pub game: Flight,
    /// The aim the game made the shot along, where it says (unit): a pellet's spread is its turn
    /// away from this.
    pub aim: Option<Vec3>,
    /// The hand whose weapon fired it, where the game tells (a gun in each hand); none: the aim
    /// hand.
    pub side: Option<usize>,
}

/// Where a shot leaves from and which way (unit), and how fast where that changes (m/s; none keeps
/// the game's speed).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shot {
    pub from: Vec3,
    pub direction: Vec3,
    pub speed: Option<f32>,
}

impl Shot {
    /// The projectile the game fires as `game`, fired as this shot says (the game's speed where the
    /// shot keeps it); none for a speed not finite.
    pub fn flight(&self, game: &Flight) -> Option<Flight> {
        let speed = self.speed.unwrap_or_else(|| length(game.velocity));
        speed.is_finite().then(|| Flight { from: self.from, velocity: self.direction.map(|c| c * speed) })
    }
}

/// The bow or gun ready-made's answer for each of the player's projectiles: none leaves it as the
/// game fires it.
pub type Aim = fn(&Fired) -> Option<Shot>;

/// Each weapon's answer: the bow's, the gun's.
static AIM: Mutex<[Option<Aim>; 2]> = Mutex::new([None; 2]);

fn weapon_index(weapon: Weapon) -> usize {
    match weapon {
        Weapon::Bow => 0,
        Weapon::Gun => 1,
    }
}

/// The ready-made's answer for `weapon`'s shots from now on (its install); none hands them back to
/// the game (its release).
pub fn aim_shots(weapon: Weapon, aim: Option<Aim>) {
    AIM.locked()[weapon_index(weapon)] = aim;
}

/// Where the player's projectile `fired` goes, by its weapon's ready-made's answer (the game's fire
/// hook asks it for each one); none: as the game fires it.
pub fn shot(fired: &Fired) -> Option<Shot> {
    let aim = AIM.locked()[weapon_index(fired.weapon)]?;
    aim(fired)
}

/// A game's fire hook: where its player's bow and guns fire their projectiles, each asked of
/// [`shot`]. A ready-made that places shots bounds its game on this, and calls [`Fires::install`]
/// from its own install.
pub trait Fires: Game {
    /// Whether the game takes a shot's speed ([`Shot::speed`]); one that does not keeps its own.
    const SPEED: bool;
    /// Hooks the firing (a second install does nothing).
    fn install(start: &mut Start<'_, Self>) -> Result<(), Lack>;
}

// ---- The draw ----

/// The bow ready-made's answer for how far the bow is drawn (0 to 1); none: the game's own draw.
pub type Draw = fn() -> Option<f32>;

static DRAW: Mutex<Option<Draw>> = Mutex::new(None);

/// The ready-made's answer for the draw from now on (its install); none hands it back to the game
/// (its release).
pub fn draw_by(draw: Option<Draw>) {
    *DRAW.locked() = draw;
}

/// How far the player's bow is drawn by the hands now (0 to 1; the game's draw hook asks where it
/// takes the draw); none: as the game draws it.
pub fn draw() -> Option<f32> {
    let draw = (*DRAW.locked())?;
    draw()
}

/// A game's bow draw: where it takes how far the player's bow is drawn (as the arrow leaves, or
/// each update as its power), asking [`draw`] there, and whether the bow is being drawn. A
/// ready-made that draws the bow by hand bounds its game on this, and calls [`Draws::install`] from
/// its own install.
pub trait Draws: Game {
    /// Hooks the draw (a second install does nothing).
    fn install(start: &mut Start<'_, Self>) -> Result<(), Lack>;
    /// How long ago the player's bow was last seen drawing, as the game tells it; none where it has
    /// not been since (or was seen not drawing since, where the game says so).
    fn drawn_ago() -> Option<Duration>;
}

// ---- The throw release ----

/// What leaves the hand in a throw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Thrown {
    /// A throwable: a knife, a bottle, a grenade.
    Throwable,
    /// The melee weapon in the hand.
    Weapon,
}

/// How the game sends a thrown item off, which says what of the answer it takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Way {
    /// Given a start and a velocity.
    Given,
    /// Launched by its own object with the velocity given (a zero one is the game's own lob).
    Launched,
    /// Flown at a point at the game's own speed: the game turns the velocity into the point, along
    /// it and as far as it is fast.
    AtPoint,
}

/// The throw the hands made as the player let go: where the hand was (none where the item starts
/// where the game has it, in the hand) and how it moved.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HandThrow {
    pub from: Option<Vec3>,
    pub velocity: Vec3,
}

/// A thrown item leaving.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Leaving {
    pub thrown: Thrown,
    pub way: Way,
    /// The throw the hands made at the let-go ([`let_go`]).
    pub hand: HandThrow,
    /// The game's own velocity for it, where the game has one before Monaka's replaces it.
    pub game: Option<Vec3>,
}

/// The throw ready-made's answers.
#[derive(Clone, Copy)]
pub struct Thrower {
    /// The player let go: the throw the hands made, read now; none leaves this throw to the game.
    pub let_go: fn(Thrown) -> Option<HandThrow>,
    /// Where the leaving item starts; none: where the game starts it.
    pub start: fn(&Leaving) -> Option<Vec3>,
    /// Its velocity; none: the game's.
    pub velocity: fn(&Leaving) -> Option<Vec3>,
    /// Whether the player's throw is under way now, as the game says (`Throws::throwing`).
    pub throwing: fn() -> bool,
}

static THROWER: Mutex<Option<Thrower>> = Mutex::new(None);

/// The ready-made's answers for the throws from now on (its install); none hands them back to the
/// game (its release).
pub fn throw_by(thrower: Option<Thrower>) {
    *THROWER.locked() = thrower;
}

/// The player let go of a throw of `thrown` (the game's throw hook asks): the throw the hands made,
/// kept by the game for the item leaving.
pub fn let_go(thrown: Thrown) -> Option<HandThrow> {
    let thrower = (*THROWER.locked())?;
    (thrower.let_go)(thrown)
}

/// Where the item `leaving` starts.
pub fn start(leaving: &Leaving) -> Option<Vec3> {
    let thrower = (*THROWER.locked())?;
    (thrower.start)(leaving)
}

/// How fast and which way the item `leaving` flies.
pub fn velocity(leaving: &Leaving) -> Option<Vec3> {
    let thrower = (*THROWER.locked())?;
    (thrower.velocity)(leaving)
}

/// Whether the player's throw by hand is under way now: the throw ready-made answers the throws, and
/// the game says one is under way.
pub fn throwing_by_hand() -> bool {
    let thrower = *THROWER.locked();
    thrower.is_some_and(|thrower| (thrower.throwing)())
}

// ---- What aims itself ----

/// Whether what is in hand aims its shots itself now, so the game's aim (the flashlight with it)
/// stays the head's: a bow drawn by hand ([`crate::bow::held`]), a gun shooting from its barrel (the
/// gun ready-made placing its shots, a gun in the aim hand `aim` as melee takes it), and, while the
/// left controller aims the left hand's tool (`tool`), a throw by hand under way.
pub fn aims_itself(tool: bool, aim: usize) -> bool {
    let gun_from_barrel = AIM.locked()[weapon_index(Weapon::Gun)].is_some();
    aims_itself_with(crate::bow::held(), gun_from_barrel, crate::held::wielded(aim), tool, throwing_by_hand)
}

/// [`aims_itself`] from what is known: a bow held by hand, the gun ready-made placing shots, what
/// the hand holds, the tool aiming, and a throw by hand under way (asked last).
fn aims_itself_with(bow: bool, gun_from_barrel: bool, held: Option<crate::held::Kind>, tool: bool, throwing: impl FnOnce() -> bool) -> bool {
    bow || (gun_from_barrel && held == Some(crate::held::Kind::Gun)) || (tool && throwing())
}

/// A game's throw release: its throw hooked where the player lets go and where the item leaves,
/// each asking the throw ready-made ([`let_go`], [`start`], [`velocity`]). A ready-made that throws
/// by hand bounds its game on this, and calls [`Throws::install`] from its own install.
pub trait Throws: Game {
    /// Hooks the throw (a second install does nothing).
    fn install(start: &mut Start<'_, Self>) -> Result<(), Lack>;
    /// Whether the player's throw is under way now (winding up to the item leaving, and a moment
    /// after, as the game says): the hands stay apart on their controllers then.
    fn throwing() -> bool;
    /// The hooks' counts, for the stop's log (empty: nothing to say).
    fn summary() -> String {
        String::new()
    }
}

#[cfg(test)]
mod tests;
