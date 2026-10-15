//! The hand rig as a ready-made. A game says what its skeleton is (a [`Skeleton`]), when the
//! pose is final, and what the hands go on ([`Moment`]); this does everything between: the palms
//! or the targets to the arms ([`Method`]), the game's own work on the solved frames
//! ([`HandRig::adjust`]), the free hands' tracked fingers ([`crate::finger_rig`]), and one write
//! back of the bones the rig placed or changed ([`Frames::flush`]).
//!
//! A game whose pose is final on another thread than its inputs (a worker's animation job) splits
//! the moment: [`capture`] on the game thread (the controllers' fingers, what the hands hold, the
//! hand trail), [`pose`] wherever the bones are final. Every other game calls [`frame`], both at
//! once.

use crate::finger_rig::{self, ChainSource};
use crate::hand_trail::HandTrails;
use crate::held::{Item, Kind};
use crate::skeleton::{Arm, find_arm, subtree};
use crate::{BoneAxes, Frames, Placed, Pose, Skeleton, Snapshot};
use monaka_core::camera::{self, Mat34, compose, position, rigid_inverse};
use monaka_core::feature::Feature;
use monaka_core::gesture::reach_share;
use monaka_core::math::{Vec3, add, distance, scale, sub};
use monaka_core::protocol::{FingerPose, HandPose, HeadPose, LEFT_HAND, RIGHT_HAND};
use monaka_core::reach::{elbow, turn_about};
use monaka_core::sync::Locked;
use monaka_producer::game::{Component, Game, Lack, On, Start};
use monaka_producer::log;
use std::sync::Mutex;

mod debug;

/// A game's hand rig: facts of its skeleton and how the arms go on.
#[derive(Clone, Copy, Debug)]
pub struct Rig {
    pub method: Method,
    pub holds: Holds,
    pub climbing: Climbing,
    /// Where its fingers are, for finger tracking (none: the fingers stay as animated).
    pub fingers: Option<ChainSource>,
}

/// How the arms go on.
#[derive(Clone, Copy, Debug)]
pub enum Method {
    /// The wrists into the palms, the arms reaching them from shoulders carried under the head as
    /// animated ([`crate::follow`]): a skeleton named as [`crate::follow`] expects, in the moment's
    /// space. `bones`: its bone axes against Dying Light 1's; `mirror`: how its left bones mirror
    /// the right ones, where that is a known fact ([`crate::declare_mirror`]; none: measured);
    /// `melee_holder`: a melee weapon locked to the aim controller ([`Pose::melee_holder`]).
    Carried { bones: BoneAxes, mirror: Option<[f32; 9]>, melee_holder: Option<Mat34> },
    /// A skeleton hung from the hands (each hand's attach joint carries the wrist, the fingers and
    /// a chain back up the arm to the shoulder), found by its shape ([`crate::skeleton::find_arm`]):
    /// each attach joint and all under it goes onto its target ([`Hands::Attached`], the
    /// skeleton's own space and units), then each arm bends at the elbow (toward `poles`) so its
    /// shoulder stays at `shoulders`. A free left hand takes `left_grip` (the wrist in its attach
    /// joint's frame) on its target; a left hand holding the other gun of two reaches across to
    /// it when the game moves it there.
    /// `right_melee_grip`: the right wrist in the right attach joint's frame as the game holds its
    /// melee weapon the way that sits right on a controller (measured once): the game turns the
    /// weapon in the hand between grips (a knife forward, then reversed), and the hand keeps this
    /// one, the weapon hanging in it as the game holds it. None: the hand follows the game's hold.
    HandRooted { shoulders: [Vec3; 2], poles: [Vec3; 2], left_grip: Mat34, right_melee_grip: Option<Mat34> },
}

/// Whether the game holds anything with both hands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Holds {
    /// The off hand rides a weapon the game holds two-handed (a rifle's front hand).
    TwoHanded,
    /// A weapon or power per hand: the off hand always follows its own controller
    /// ([`crate::declare_one_handed`]).
    OneHanded,
}

/// How the arms are told to go back to the game's animation while it climbs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Climbing {
    /// Measured from how the game animates the hands ([`crate::traverse`]).
    Animated,
    /// Said by the game each update ([`crate::traverse::report_climbing`]).
    Reported,
    /// The game never climbs with the arms ([`crate::traverse::declare_no_climbing`]).
    None,
}

/// A game with the hand rig.
pub trait HandRig: Game {
    const RIG: Rig;
    /// The kind of what hand `side` holds now (the held-item provider's kind; the rest of the item:
    /// [`crate::held::HeldItems`]): a hand that grips it keeps the game's grip ([`Kind::grips`]), and
    /// a gun in the aim hand is held by its grip ([`Pose::gun`]).
    fn held(_side: usize) -> Kind {
        Kind::Nothing
    }
    /// What hand `side` holds, when it holds something the game describes (none: only its kind is
    /// known).
    fn item(_side: usize) -> Option<Item> {
        None
    }
    /// The element of the skeleton hand `side` holds things by (a weapon holder, an attach joint),
    /// where the game has one: what the hand holds hangs from it ([`crate::held::Line`] is in its
    /// frame), and the rig reads it back once it has posed the arms ([`posed`]).
    fn holder(_side: usize) -> Option<i32> {
        None
    }
    /// A weapon hand `side` holds that the rig lines up with its controller once the arms are
    /// solved (a gun the game holds pointing elsewhere, in the off hand; [`crate::gun_hold`]),
    /// asked at each posing with the palms; none leaves the hand as solved.
    fn lined_up(_side: usize) -> Option<crate::gun_hold::LinedUp> {
        None
    }
    /// The game's own work on the solved frames, before the fingers (a weapon turned in the hand).
    fn adjust(_frames: &mut Frames, _placed: &Placed) {}
    /// The controllers' palms in the world, noted each moment, for what the hands do outside the
    /// arms' update ([`crate::hand_trail`]: a throw's swing, a bow's draw, a reload's touch).
    fn trail() -> Option<&'static HandTrails> {
        None
    }
    /// Whether the rig notes [`HandRig::trail`] at each moment ([`capture`]); false: the game
    /// notes it itself.
    const NOTES_TRAIL: bool = true;
    /// Where the game's hands are after the rig and its own animation, where the game reads them
    /// back and notes them (each a trail of places): an arm the game animates through a throw lags
    /// its controller. None: where the rig put them ([`posed_hand`]).
    fn posed_hands() -> Option<&'static HandTrails> {
        None
    }
    /// Where the right hand holds a bow's string, in the world. By default the string point on
    /// the right palm ([`crate::hands::Point::StringPoint`]); a game whose hand bones say better
    /// gives its own.
    fn string_point() -> Option<Vec3> {
        crate::hands::pose::<Self>(RIGHT_HAND, crate::hands::Point::StringPoint, crate::hands::At::Latest, crate::hands::In::World).map(|p| p.position)
    }
}

/// Where hand `side` of game `G` is now in the world after the rig and the game's animation
/// ([`HandRig::posed_hands`]), else where the rig put it: its palm noted on the trail.
pub fn posed_hand<G: HandRig>(side: usize) -> Option<Vec3> {
    match G::posed_hands() {
        Some(hands) => hands.position(side),
        None => crate::hands::pose::<G>(side, crate::hands::Point::Palm, crate::hands::At::Latest, crate::hands::In::World).map(|p| p.position),
    }
}

/// The controllers' palms for [`Method::Carried`].
#[derive(Clone, Copy, Debug)]
pub struct Palms {
    /// The player camera as the game set it (camera-to-world).
    pub game: Mat34,
    /// Where the headset's tracking space sits in the world, as the view is drawn from it.
    pub origin: Mat34,
    pub head: HeadPose,
    /// Each controller's palm in tracking space, by side, when tracked.
    pub palms: [Option<HandPose>; 2],
    /// The controller that aims.
    pub aim_side: usize,
    /// The wrist moved this far in the palm's space ([`Pose::hold`]).
    pub hold: Vec3,
    /// Bone axes for this run instead of the rig's (a calibration being measured).
    pub bones: Option<BoneAxes>,
}

/// What the hands go on.
// One moment's value, made and read once an update: its size does not matter.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Copy, Debug)]
pub enum Hands {
    /// [`Method::Carried`].
    Palms(Palms),
    /// [`Method::HandRooted`]: the right hand's attach joint (the left one is its sibling) and
    /// where each goes (the skeleton's space; the left one only while it follows its own
    /// controller, else it rides the right hand's weapon as the game holds it). `space`: where the
    /// skeleton's space sits in tracking space (skeleton-to-tracking, scale included) as the game
    /// placed the targets this moment, so the debug views draw the bones where the eyes see them.
    Attached { right: usize, targets: [Option<Mat34>; 2], space: Mat34 },
}

/// Whether any hand of a moment is tracked (a palm, or an attached hand's target).
fn tracked(hands: &Hands) -> bool {
    match hands {
        Hands::Palms(palms) => palms.palms.iter().any(Option::is_some),
        Hands::Attached { targets, .. } => targets.iter().any(Option::is_some),
    }
}

/// Whether any of the rig's debug views is on: a game makes its moment then even with no
/// controller tracked, so the views show its arms as it animated them.
pub fn views_on() -> bool {
    debug::views_on()
}

/// One moment of the hand rig.
#[derive(Clone, Copy, Debug)]
pub struct Moment {
    pub hands: Hands,
    /// The free hands' fingers follow finger tracking now.
    pub fingers: bool,
}

/// A moment as captured on the game thread ([`capture`]), for [`pose`].
#[derive(Clone, Copy, Debug)]
pub struct Inputs {
    hands: Hands,
    held: [Kind; 2],
    /// Each hand's holder ([`HandRig::holder`]), read back after the write.
    holders: [Option<i32>; 2],
    /// Each free hand's tracked fingers.
    fingers: [Option<FingerPose>; 2],
}

/// The game thread's part of a moment of game `G`: what the hands hold and by which elements, the
/// free hands' tracked fingers, the palms noted on the game's hand trail. What the hands hold is
/// noted for code without the game type ([`crate::held::latest`]).
pub fn capture<G: HandRig>(moment: Moment) -> Inputs {
    let held = [G::held(LEFT_HAND), G::held(RIGHT_HAND)];
    crate::held::note_latest::<G>(held);
    let holders = [G::holder(LEFT_HAND), G::holder(RIGHT_HAND)];
    let fingers = std::array::from_fn(|side| if moment.fingers && !held[side].grips() { crate::controllers::fingers(side) } else { None });
    if let (true, Hands::Palms(palms), Some(trail)) = (G::NOTES_TRAIL, &moment.hands, G::trail()) {
        trail.note(&palms.origin, &palms.palms);
    }
    Inputs { hands: moment.hands, held, holders, fingers }
}

/// Each hand's holder ([`HandRig::holder`]) as the rig left it at its latest posing, in the
/// skeleton's space (the game world for [`Method::Carried`]): where what the hand holds is now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Posed {
    /// By side; none where the game names no holder or it could not be read.
    pub holders: [Option<Mat34>; 2],
    /// When the rig posed them (`monaka_channel::tick`). A pose the rig could not make leaves the
    /// last one, older.
    pub at: u64,
}

static POSED: Mutex<Option<Posed>> = Mutex::new(None);

/// Each hand's holder as the rig left it at its latest posing ([`pose`]); none before the first.
pub fn posed() -> Option<Posed> {
    *POSED.locked()
}

/// Notes each holder as the rig left it: what the rig wrote, where it wrote the holder itself (an
/// engine may keep a write for later), else read back from `skeleton` (a helper the engine moves
/// with its bone).
fn note_posed(skeleton: &dyn Skeleton, frames: &Frames, written: &[usize], holders: [Option<i32>; 2]) {
    let read = |holder: Option<i32>| {
        let element = holder?;
        let index = usize::try_from(element).ok()?;
        let world = if written.contains(&index) { frames.now.get(index).copied() } else { skeleton.world(element) };
        world.filter(|w| w.iter().all(|v| v.is_finite()))
    };
    *POSED.locked() = Some(Posed { holders: holders.map(read), at: monaka_channel::tick() });
}

/// Puts game `G`'s arms on `inputs` (any thread, once the pose is final): `animated` is the
/// skeleton as the game animated it. The arms by the rig's method, the game's adjustment, the
/// free hands' fingers, then the bones placed or changed written to `skeleton`, and each hand's
/// holder read back ([`posed`]). Err (with why, nothing written) when the arms cannot go on.
/// Timed for the Stats panel while it shows.
pub fn pose<G: HandRig>(skeleton: &dyn Skeleton, animated: &Snapshot, inputs: &Inputs) -> Result<Placed, &'static str> {
    let started = monaka_producer::debug::timing().then(std::time::Instant::now);
    let posed = posing::<G>(skeleton, animated, inputs);
    if let Some(at) = started {
        monaka_producer::debug::cost(monaka_core::debug_draw::Cost::HandRig, at.elapsed());
    }
    posed
}

fn posing<G: HandRig>(skeleton: &dyn Skeleton, animated: &Snapshot, inputs: &Inputs) -> Result<Placed, &'static str> {
    if animated.skeleton != skeleton.id() || animated.worlds.len() != skeleton.count().max(0) as usize {
        return Err("skeleton changed");
    }
    let mut frames = Frames::new(animated);
    if !tracked(&inputs.hands) {
        // No hand to put the arms on: nothing is moved, and the views show the game's own arms.
        debug::draw::<G>(skeleton, &frames, &[], inputs, &Placed { palm: [f32::NAN; 12] }, true);
        return Err("no controller tracked");
    }
    let (placed, carried) = match (G::RIG.method, inputs.hands) {
        (Method::Carried { bones, melee_holder, .. }, Hands::Palms(p)) => {
            let pose = Pose {
                game: p.game,
                origin: p.origin,
                head: p.head,
                palms: p.palms,
                aim_side: p.aim_side,
                hold: p.hold,
                gun: inputs.held[p.aim_side.min(1)] == Kind::Gun,
                melee_holder,
                bones: p.bones.unwrap_or(bones),
            };
            let placed = crate::carry(skeleton, animated, &pose, &mut frames)?;
            for side in [LEFT_HAND, RIGHT_HAND] {
                if let Some(lined) = G::lined_up(side) {
                    crate::gun_hold::line_up(&mut frames, &lined, &p.origin, p.palms[side].as_ref());
                }
            }
            (placed, Some(p.aim_side))
        }
        (Method::HandRooted { shoulders, poles, left_grip, right_melee_grip }, Hands::Attached { right, targets, .. }) => {
            let rooted = HandRooted { shoulders, poles, left_grip, right_melee_grip, held: inputs.held };
            (rooted.place(skeleton, &mut frames, right, targets)?, None)
        }
        _ => return Err("the moment does not fit the hand rig's method"),
    };
    G::adjust(&mut frames, &placed);
    if let Some(source) = &G::RIG.fingers {
        for side in [LEFT_HAND, RIGHT_HAND] {
            // A carried off hand riding a weapon held two-handed keeps the game's grip.
            let riding = carried.is_some_and(|aim| side != aim && crate::off_hand_on_weapon());
            if let Some(tracked) = inputs.fingers[side].filter(|_| !riding) {
                finger_rig::place(skeleton, &mut frames, source, side, &tracked);
            }
        }
    }
    // The debug view of the game's own arms: everything worked out, nothing written.
    let left_to_game = monaka_producer::debug::on(monaka_core::debug_draw::Category::SkipHandRig);
    let written = if left_to_game { frames.changed(skeleton) } else { frames.flush(skeleton) };
    debug::draw::<G>(skeleton, &frames, &written, inputs, &placed, left_to_game);
    if left_to_game {
        return Ok(placed);
    }
    note_posed(skeleton, &frames, &written, inputs.holders);
    if carried.is_some() {
        crate::remember(animated.skeleton, &frames, &written, placed.palm);
    }
    Ok(placed)
}

/// A moment of game `G` captured and posed at once ([`capture`], [`pose`]).
pub fn frame<G: HandRig>(skeleton: &dyn Skeleton, animated: &Snapshot, moment: Moment) -> Result<Placed, &'static str> {
    pose::<G>(skeleton, animated, &capture::<G>(moment))
}

/// The hand rig's part of a game's arms component install: the rig's facts declared, the
/// controllers opened. The game's install calls it, then hooks its moment.
pub fn install<G: HandRig>(start: &mut Start<'_, G>) -> Result<(), Lack> {
    let rig = G::RIG;
    if let Method::Carried { mirror: Some(mirror), .. } = rig.method {
        crate::declare_mirror(mirror);
    }
    if rig.holds == Holds::OneHanded {
        crate::declare_one_handed();
    }
    if rig.climbing == Climbing::None {
        crate::traverse::declare_no_climbing();
    }
    crate::controllers::open(start.channel()?);
    debug::offer();
    Ok(())
}

/// The hand rig component for game `G`, on with motion controls: a game gives its own install
/// (which calls [`install`] and hooks its moment) and what it works on top of, by struct update.
pub const fn hand_rig<G: HandRig>() -> Component<G> {
    Component {
        name: "hand rig",
        features: &[Feature::HandRig],
        needs_channel: true,
        on: |run| On::when(run.shared.aim.rig, "aim is not motion controls"),
        install: install::<G>,
        release: crate::controllers::close,
        ..Component::NONE
    }
}

/// End-of-run lines.
pub fn report() {
    finger_rig::report();
}

// ---- Hand-rooted ----

/// [`Method::HandRooted`] for one moment.
struct HandRooted {
    shoulders: [Vec3; 2],
    poles: [Vec3; 2],
    left_grip: Mat34,
    right_melee_grip: Option<Mat34>,
    held: [Kind; 2],
}

/// The arms found under attach joints: by skeleton and attach joint.
static ARMS: Mutex<Vec<(usize, usize, Option<Arm>)>> = Mutex::new(Vec::new());

/// The game's melee grip of the right hand last logged (the wrist in the attach joint's frame), and
/// how many changes of it were logged: each change is the game turning the weapon in the hand.
static MELEE_GRIP_LOGGED: Mutex<(Option<Mat34>, u32)> = Mutex::new((None, 0));
const MELEE_GRIP_LOGS: u32 = 12;
/// Whether the game's right-hand gun grip was logged (once a run: a game's grips are measured from it).
static GUN_GRIP_LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn side_name(side: usize) -> &'static str {
    if side == RIGHT_HAND { "right" } else { "left" }
}

/// The arm of `side` under attach joint `root`, found once per skeleton.
fn arm(skeleton: usize, parents: &[i32], side: usize, root: usize) -> Option<Arm> {
    let mut arms = ARMS.locked();
    if let Some((_, _, arm)) = arms.iter().find(|(s, r, _)| *s == skeleton && *r == root) {
        return arm.clone();
    }
    let arm = find_arm(parents, root);
    match &arm {
        Some(a) => log!("hand rig: {} arm from joint {}: wrist {}, elbow {}, shoulder {}, {} joints, {} in the forearm", side_name(side), a.root, a.wrist, a.elbow, a.shoulder, a.subtree.len(), a.forearm.len()),
        None => log!("hand rig: no {} arm under joint {root}", side_name(side)),
    }
    if arms.len() >= 16 {
        arms.clear();
    }
    arms.push((skeleton, root, arm.clone()));
    arm
}

/// `w` of the way from rigid `a` to rigid `b` (the rotation along the shorter way round).
fn blend(a: &Mat34, b: &Mat34, w: f32) -> Mat34 {
    let (qa, mut qb) = (camera::quaternion(a), camera::quaternion(b));
    if (0..4).map(|i| qa[i] * qb[i]).sum::<f32>() < 0.0 {
        qb = qb.map(|v| -v);
    }
    let q: [f32; 4] = std::array::from_fn(|i| qa[i] + (qb[i] - qa[i]) * w);
    let n = (0..4).map(|i| q[i] * q[i]).sum::<f32>().sqrt().max(1e-6);
    let r = camera::rotation(q.map(|v| v / n));
    let t = add(position(a), scale(sub(position(b), position(a)), w));
    [r[0], r[1], r[2], t[0], r[3], r[4], r[5], t[1], r[6], r[7], r[8], t[2]]
}

/// The move taking rigid `from` to `to`.
fn move_between(from: &Mat34, to: &Mat34) -> Mat34 {
    compose(to, &rigid_inverse(from))
}

/// Moves `which` by `d` (applied after each one's own transform).
fn move_rigid(joints: &mut [Mat34], which: &[usize], d: &Mat34) {
    for &j in which {
        joints[j] = compose(d, &joints[j]);
    }
}

impl HandRooted {
    /// The arms onto their targets in `frames` (nothing placed: only what moves is written).
    fn place(&self, skeleton: &dyn Skeleton, frames: &mut Frames, right: usize, targets: [Option<Mat34>; 2]) -> Result<Placed, &'static str> {
        let count = frames.now.len();
        let parents: Vec<i32> = (0..count as i32).map(|e| skeleton.parent(e)).collect();
        let right_target = targets[RIGHT_HAND].ok_or("no target for the right hand")?;
        let hands_root = parents.get(right).and_then(|&p| usize::try_from(p).ok()).ok_or("the right attach joint hangs from nothing")?;
        // The left attach joint: the right one's sibling.
        let left_root = (0..count).find(|&j| j != right && parents[j] == hands_root as i32);
        let arms = [left_root.and_then(|root| arm(skeleton.id(), &parents, LEFT_HAND, root)), arm(skeleton.id(), &parents, RIGHT_HAND, right)];
        let right_arm = arms[RIGHT_HAND].as_ref().ok_or("no right arm")?;
        frames.wrists = arms.each_ref().map(|a| a.as_ref().and_then(|a| Some((a.wrist, *a.forearm.first()?))));
        let game = frames.animated;
        let game_grip = compose(&rigid_inverse(&game[right_arm.root]), &game[right_arm.wrist]);
        match self.held[RIGHT_HAND] {
            Kind::Melee | Kind::Blade => note_melee_grip(&game_grip),
            Kind::Gun if !GUN_GRIP_LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) => {
                log!("hand rig: the game's gun grip of the right hand: {:?}", game_grip.map(|v| (v * 1000.0).round() / 1000.0));
            }
            _ => {}
        }
        let joints = &mut frames.now;
        let d = move_between(&joints[right_arm.root], &right_target);
        match (targets[LEFT_HAND], &arms[LEFT_HAND]) {
            // A gun in each hand, or a free left hand: each hand onto its own controller.
            (Some(left), Some(left_arm)) => {
                move_rigid(joints, &right_arm.subtree, &d);
                let d_left = move_between(&joints[left_arm.root], &left);
                move_rigid(joints, &left_arm.subtree, &d_left);
                if !self.held[LEFT_HAND].grips() {
                    // A free hand: the game poses it for no grip at all, so it takes a gun's grip
                    // on its controller (fingers as the game has them).
                    let d_wrist = move_between(&joints[left_arm.wrist], &compose(&left, &self.left_grip));
                    move_rigid(joints, &subtree(&parents, left_arm.wrist), &d_wrist);
                } else {
                    reach_across(joints, game, &parents, right_arm, left_arm, &d_left);
                    reach_across(joints, game, &parents, left_arm, right_arm, &d);
                }
            }
            // One weapon: both hands as the game holds it (the front hand keeps its grip).
            _ => move_rigid(joints, &subtree(&parents, hands_root), &d),
        }
        // A melee weapon's grip changes with its animation (a knife held forward, then reversed),
        // its attach joint turning in the hand: on the attach joint the hand would turn with it.
        // The hand keeps the game's gun grip on its controller instead, the weapon hanging in it
        // as the game holds it now.
        if matches!(self.held[RIGHT_HAND], Kind::Melee | Kind::Blade)
            && let Some(grip) = self.right_melee_grip
        {
            let d_hand = move_between(&joints[right_arm.wrist], &compose(&right_target, &grip));
            move_rigid(joints, &right_arm.subtree, &d_hand);
        }
        for side in [LEFT_HAND, RIGHT_HAND] {
            if let Some(arm) = &arms[side] {
                bend_arm(joints, arm, self.shoulders[side], self.poles[side]).ok_or("an arm could not reach its hand")?;
            }
        }
        Ok(Placed { palm: right_target })
    }
}

/// The angle (radians) between the rotations of rigid `a` and `b`.
fn turned_by(a: &Mat34, b: &Mat34) -> f32 {
    let r = compose(&rigid_inverse(a), b);
    ((r[0] + r[5] + r[10] - 1.0) * 0.5).clamp(-1.0, 1.0).acos()
}

/// Logs the game's melee grip of the right hand (`grip`: the wrist in the attach joint's frame) when
/// it first shows and each time the game turns the weapon in the hand (by a sixth of a turn or
/// more): the values a game's `right_melee_grip` is measured from.
fn note_melee_grip(grip: &Mat34) {
    let mut logged = MELEE_GRIP_LOGGED.locked();
    let turned = logged.0.is_none_or(|last| turned_by(&last, grip) > 60f32.to_radians());
    if turned && logged.1 < MELEE_GRIP_LOGS {
        logged.1 += 1;
        let at = logged.0.map_or(String::new(), |last| format!(" (turned {:.0} degrees in the hand)", turned_by(&last, grip).to_degrees()));
        log!("hand rig: the game's melee grip of the right hand{at}: {:?}", grip.map(|v| (v * 1000.0).round() / 1000.0));
        logged.0 = Some(*grip);
    }
}

/// How far a hand has gone from its own weapon over to the other one in the game's pose `game`: 0
/// holding its own, 1 at the other (with two guns each hand reloads the other's).
fn reach(game: &[Mat34], arm: &Arm, other: &Arm) -> f32 {
    reach_share(position(&game[arm.wrist]), position(&game[arm.root]), position(&game[other.root]), 0.3, 0.7)
}

/// A hand working the other weapon goes where that weapon is now: its wrist (and all below it)
/// blends from riding its own to keeping the game's place relative to the other, which moved by
/// `other_move`.
fn reach_across(joints: &mut [Mat34], game: &[Mat34], parents: &[i32], arm: &Arm, other: &Arm, other_move: &Mat34) {
    let w = reach(game, arm, other);
    if w <= 0.0 {
        return;
    }
    let target = blend(&joints[arm.wrist], &compose(other_move, &game[arm.wrist]), w);
    let d = move_between(&joints[arm.wrist], &target);
    move_rigid(joints, &subtree(parents, arm.wrist), &d);
}

/// Bends `arm` (already moved with its hand) so its shoulder reaches `shoulder_at`, the elbow
/// toward `pole`: the forearm turns about the wrist, the upper arm about the elbow.
fn bend_arm(joints: &mut [Mat34], arm: &Arm, shoulder_at: Vec3, pole: Vec3) -> Option<()> {
    let (w, e, s) = (position(&joints[arm.wrist]), position(&joints[arm.elbow]), position(&joints[arm.shoulder]));
    let elbow_at = elbow(w, shoulder_at, distance(e, w), distance(s, e), pole)?;
    let forearm = turn_about(w, sub(e, w), sub(elbow_at, w))?;
    move_rigid(joints, &arm.forearm, &forearm);
    let (e, s) = (position(&joints[arm.elbow]), position(&joints[arm.shoulder]));
    let upper = turn_about(e, sub(s, e), sub(shoulder_at, e))?;
    move_rigid(joints, &[arm.shoulder], &upper);
    joints.iter().all(|j| j.iter().all(|v| v.is_finite())).then_some(())
}

#[cfg(test)]
mod tests;
