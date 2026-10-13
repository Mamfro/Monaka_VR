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
    HandRooted { shoulders: [Vec3; 2], poles: [Vec3; 2], left_grip: Mat34 },
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
    /// controller, else it rides the right hand's weapon as the game holds it).
    Attached { right: usize, targets: [Option<Mat34>; 2] },
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
pub fn pose<G: HandRig>(skeleton: &dyn Skeleton, animated: &Snapshot, inputs: &Inputs) -> Result<Placed, &'static str> {
    if animated.skeleton != skeleton.id() || animated.worlds.len() != skeleton.count().max(0) as usize {
        return Err("skeleton changed");
    }
    let mut frames = Frames::new(animated);
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
        (Method::HandRooted { shoulders, poles, left_grip }, Hands::Attached { right, targets }) => {
            let rooted = HandRooted { shoulders, poles, left_grip, held: inputs.held };
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
    let written = frames.flush(skeleton);
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
    held: [Kind; 2],
}

/// The arms found under attach joints: by skeleton and attach joint.
static ARMS: Mutex<Vec<(usize, usize, Option<Arm>)>> = Mutex::new(Vec::new());

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
        for side in [LEFT_HAND, RIGHT_HAND] {
            if let Some(arm) = &arms[side] {
                bend_arm(joints, arm, self.shoulders[side], self.poles[side]).ok_or("an arm could not reach its hand")?;
            }
        }
        Ok(Placed { palm: right_target })
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
mod tests {
    use super::*;
    use crate::finger_rig::FingerNames;
    use monaka_core::camera::{IDENTITY_MATRIX, translation};
    use monaka_producer::game::Adapter;
    use std::cell::RefCell;

    /// A skeleton of plain matrices.
    struct Plain {
        id: usize,
        names: Vec<String>,
        parents: Vec<i32>,
        rest: Vec<Option<Mat34>>,
        worlds: RefCell<Vec<Mat34>>,
        writes: RefCell<Vec<usize>>,
        /// A helper's frame against its parent: it follows its bone, as an engine moves it.
        helpers: Vec<Option<Mat34>>,
    }

    impl Plain {
        fn new(id: usize) -> Self {
            Plain { id, names: Vec::new(), parents: Vec::new(), rest: Vec::new(), worlds: RefCell::new(Vec::new()), writes: RefCell::new(Vec::new()), helpers: Vec::new() }
        }

        /// Element `name` made a helper of its parent, kept where it is against it.
        fn make_helper(&mut self, name: &str) -> usize {
            let at = self.names.iter().position(|n| n == name).unwrap();
            let worlds = self.worlds.borrow();
            let local = compose(&camera::inverse(&worlds[self.parents[at] as usize]).unwrap(), &worlds[at]);
            drop(worlds);
            self.helpers.resize(self.names.len(), None);
            self.helpers[at] = Some(local);
            at
        }

        /// A bone at `world` (its rest the same against its parent, when `kept`).
        fn bone(&mut self, name: &str, parent: i32, world: Mat34, kept: bool) -> usize {
            let rest = if parent < 0 { world } else { compose(&camera::inverse(&self.worlds.borrow()[parent as usize]).unwrap(), &world) };
            self.names.push(name.to_owned());
            self.parents.push(parent);
            self.rest.push(kept.then_some(rest));
            self.worlds.borrow_mut().push(world);
            self.names.len() - 1
        }
    }

    impl Skeleton for Plain {
        fn id(&self) -> usize {
            self.id
        }
        fn count(&self) -> i32 {
            self.names.len() as i32
        }
        fn name(&self, element: i32) -> String {
            self.names[element as usize].clone()
        }
        fn parent(&self, element: i32) -> i32 {
            self.parents[element as usize]
        }
        fn rest(&self, element: i32) -> Option<Mat34> {
            self.rest[element as usize]
        }
        fn world(&self, element: i32) -> Option<Mat34> {
            match self.helpers.get(element as usize).copied().flatten() {
                Some(local) => Some(compose(&self.world(self.parents[element as usize])?, &local)),
                None => self.worlds.borrow().get(element as usize).copied(),
            }
        }
        fn set_world(&self, element: i32, matrix: &Mat34) {
            self.worlds.borrow_mut()[element as usize] = *matrix;
            self.writes.borrow_mut().push(element as usize);
        }
        fn is_bone(&self, element: i32) -> bool {
            self.helpers.get(element as usize).is_none_or(Option::is_none)
        }
    }

    /// The tests that pose, one at a time: each hand's posed holder is one for the run.
    static POSING: Mutex<()> = Mutex::new(());

    /// A left hand under a squashing root (as one engine squashes its arms toward the camera):
    /// metacarpals and three knuckles a finger, the thumb three bones, the fingers down -z.
    fn squashed_hand(id: usize, kept: bool) -> Plain {
        let mut arms = Plain::new(id);
        let squash: Mat34 = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.6, 0.0];
        let root = arms.bone("root", -1, squash, true) as i32;
        let hand = arms.bone("l_hand", root, compose(&squash, &translation([0.0, 0.0, -0.3])), true) as i32;
        for (finger, x) in [(0, 0.04), (1, 0.02), (2, 0.0), (3, -0.02), (4, -0.04)] {
            let mut parent = hand;
            let mut at = [x, 0.0, -0.3];
            if finger != 0 {
                at[2] -= 0.03;
                parent = arms.bone(&format!("l_finger{finger}0"), hand, compose(&squash, &translation(at)), kept) as i32;
            }
            for n in 1..=3 {
                at = if finger == 0 && n == 1 { [x, -0.01, at[2] - 0.02] } else { [at[0], at[1], at[2] - 0.03] };
                parent = arms.bone(&format!("l_finger{finger}{n}"), parent, compose(&squash, &translation(at)), kept) as i32;
            }
        }
        arms
    }

    const CHROME: FingerNames = FingerNames { wrists: ["l_hand", "r_hand"], sides: ["l_", "r_"], fingers: ["finger0", "finger1", "finger2", "finger3", "finger4"], bone: "{side}{finger}{n}" };

    #[test]
    fn a_squashed_hand_curls_its_tracked_finger_alone() {
        for kept in [true, false] {
            // A distinct skeleton per run: the hands are found once per skeleton.
            let arms = squashed_hand(10 + kept as usize, kept);
            let at = |name: &str| arms.names.iter().position(|n| n == name).unwrap();
            let snapshot = Snapshot::take(&arms).unwrap();
            let mut frames = Frames::new(&snapshot);
            let mut pose = FingerPose { valid: 1, ..FingerPose::default() };
            pose.fingers[1] = [1.0, 1.0, 1.0, 0.0];
            assert!(finger_rig::place(&arms, &mut frames, &ChainSource::Named(CHROME), LEFT_HAND, &pose));
            let written = frames.flush(&arms);
            let moved = |name: &str| written.contains(&at(name));
            assert!(moved("l_finger12") && moved("l_finger13"), "the index finger curls");
            assert!(!moved("l_hand") && !moved("l_finger10") && !moved("l_finger22"), "the hand, its metacarpal and the other fingers stay");
            // The squash stays in what is written.
            let tip = arms.worlds.borrow()[at("l_finger13")];
            let length = |c: usize| (0..3).map(|r| tip[r * 4 + c] * tip[r * 4 + c]).sum::<f32>().sqrt();
            assert!((0..3).map(length).any(|l| (l - 0.6).abs() < 0.05), "{tip:?}");
        }
    }

    /// The right arm of a skeleton hung from the hands (a rifle hold): 0 the hands root, 1 the
    /// attach joint, 2 the wrist with three fingers (3, 4, 5), 6 and 7 forearm twists, 8 the
    /// elbow, 9 the shoulder; the left arm mirrored (10 to 18).
    fn hand_rooted() -> Plain {
        let mut arms = Plain::new(20);
        let positions = [
            [0.0, 0.0, 0.0],
            [16.5, -6.1, -11.9],
            [13.2, -9.0, -11.0],
            [18.3, -8.9, -9.7],
            [18.4, -8.9, -11.1],
            [17.9, -8.7, -12.4],
            [11.3, -9.7, -11.6],
            [6.2, -11.5, -13.2],
            [1.2, -13.3, -14.8],
            [-10.9, -10.0, -10.8],
        ];
        let parents = [-1, 0, 1, 2, 2, 2, 2, 6, 7, 8];
        for (&p, &parent) in positions.iter().zip(&parents) {
            arms.bone("", parent, translation(p), false);
        }
        for j in 1..10 {
            let p = positions[j];
            arms.bone("", if parents[j] == 0 { 0 } else { parents[j] + 9 }, translation([p[0], -p[1], p[2]]), false);
        }
        arms
    }

    struct Wolfish;
    impl Game for Wolfish {
        const MANIFESTS: &'static [monaka_core::manifest::Manifest] = &[];
        const ADAPTER: Adapter<Self> = Adapter { components: &[] };
        type Build = ();
        type Frame = ();
        fn prepare(_: &'static monaka_producer::Session, _: &'static monaka_core::manifest::Manifest, _: &monaka_core::options::Options) -> Result<(), monaka_producer::Rejection> {
            Ok(())
        }
    }
    impl HandRig for Wolfish {
        const RIG: Rig = Rig {
            method: Method::HandRooted { shoulders: [[-10.9, 10.0, -10.8], [-10.9, -10.0, -10.8]], poles: [[0.0, 0.6, -1.0], [0.0, -0.6, -1.0]], left_grip: IDENTITY_MATRIX },
            holds: Holds::TwoHanded,
            climbing: Climbing::None,
            fingers: None,
        };
        fn held(_: usize) -> Kind {
            Kind::Gun
        }
        /// Each gun hangs from its hand's attach joint.
        fn holder(side: usize) -> Option<i32> {
            Some(if side == RIGHT_HAND { 1 } else { 10 })
        }
    }

    #[test]
    fn a_hand_rooted_arm_reaches_its_target_and_the_shoulder_stays() {
        let _serial = POSING.locked();
        let arms = hand_rooted();
        let animated = Snapshot::take(&arms).unwrap();
        let lengths = |j: &[Mat34]| (distance(position(&j[8]), position(&j[2])), distance(position(&j[9]), position(&j[8])));
        let before = lengths(&animated.worlds);
        // The hand 4 units further in, 4 out and 4 up, turned 30 degrees about up.
        let (s, c) = 30f32.to_radians().sin_cos();
        let target = [c, -s, 0.0, 12.0, s, c, 0.0, -10.0, 0.0, 0.0, 1.0, -8.0];
        let moment = Moment { hands: Hands::Attached { right: 1, targets: [None, Some(target)] }, fingers: false };
        frame::<Wolfish>(&arms, &animated, moment).unwrap();
        let after = arms.worlds.borrow().clone();
        assert!(camera::distance(&after[1], &target) < 1e-5, "the attach joint is the target");
        assert!(distance(position(&after[9]), [-10.9, -10.0, -10.8]) < 1e-3, "the shoulder stays: {:?}", position(&after[9]));
        let now = lengths(&after);
        assert!((now.0 - before.0).abs() < 1e-3 && (now.1 - before.1).abs() < 1e-3, "bone lengths kept: {before:?} {now:?}");
        assert!(position(&after[8])[2] < position(&after[2])[2].max(position(&after[9])[2]), "the elbow bends down");
        // The left hand rides the right one's weapon: it moved, its shoulder stays.
        assert!(camera::distance(&after[10], &animated.worlds[10]) > 1e-3);
        assert!(distance(position(&after[18]), [-10.9, 10.0, -10.8]) < 1e-3);
    }

    #[test]
    fn a_moment_for_another_method_is_refused() {
        let arms = hand_rooted();
        let animated = Snapshot::take(&arms).unwrap();
        let palms = Palms { game: IDENTITY_MATRIX, origin: IDENTITY_MATRIX, head: HeadPose::default(), palms: [None; 2], aim_side: RIGHT_HAND, hold: [0.0; 3], bones: None };
        assert!(frame::<Wolfish>(&arms, &animated, Moment { hands: Hands::Palms(palms), fingers: false }).is_err());
        assert!(arms.writes.borrow().is_empty());
    }

    /// Named arms as [`crate::follow`] expects them, standing under a level camera at 1.6 m.
    fn named_arms(id: usize) -> Plain {
        let mut arms = Plain::new(id);
        let at = |p: Vec3| translation(p);
        let body = arms.bone("hspine", -1, at([0.0, 1.35, 0.05]), true) as i32;
        for (side, x) in [("l_", -1.0f32), ("r_", 1.0)] {
            let clavicle = arms.bone(&format!("{side}clavicle"), body, at([0.08 * x, 1.45, 0.0]), true) as i32;
            let upper = arms.bone(&format!("{side}upperarm"), clavicle, at([0.18 * x, 1.42, 0.0]), true) as i32;
            let fore = arms.bone(&format!("{side}forearm"), upper, at([0.22 * x, 1.18, -0.12]), true) as i32;
            let hand = arms.bone(&format!("{side}hand"), fore, at([0.2 * x, 1.12, -0.38]), true) as i32;
            arms.bone(&format!("{side}handholder"), hand, at([0.2 * x, 1.1, -0.46]), true);
        }
        for side in ["l_", "r_"] {
            arms.bone(&format!("{side}thigh"), body, at([0.0, 0.9, 0.0]), true);
            arms.bone(&format!("{side}calf"), body, at([0.0, 0.5, 0.0]), true);
        }
        arms
    }

    struct Chromish;
    impl Game for Chromish {
        const MANIFESTS: &'static [monaka_core::manifest::Manifest] = &[];
        const ADAPTER: Adapter<Self> = Adapter { components: &[] };
        type Build = ();
        type Frame = ();
        fn prepare(_: &'static monaka_producer::Session, _: &'static monaka_core::manifest::Manifest, _: &monaka_core::options::Options) -> Result<(), monaka_producer::Rejection> {
            Ok(())
        }
    }
    const MIRROR: [f32; 9] = [1.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 1.0];
    impl HandRig for Chromish {
        const RIG: Rig = Rig {
            method: Method::Carried { bones: BoneAxes::DYING_LIGHT_1, mirror: Some(MIRROR), melee_holder: None },
            holds: Holds::TwoHanded,
            climbing: Climbing::Animated,
            fingers: None,
        };
        /// `l_handholder` and `r_handholder` in [`named_arms`].
        fn holder(side: usize) -> Option<i32> {
            Some(if side == RIGHT_HAND { 10 } else { 5 })
        }
    }

    #[test]
    fn the_carried_method_writes_what_follow_writes() {
        let _serial = POSING.locked();
        crate::declare_mirror(MIRROR);
        let game = translation([0.0, 1.6, 0.0]);
        let head = HeadPose { orientation: [0.0, 0.0, 0.0, 1.0], position: [0.0, 1.6, 0.0], valid: 1, ..HeadPose::default() };
        let palm = |x: f32| HandPose { orientation: [0.0, 0.0, 0.0, 1.0], position: [0.25 * x, 1.25, -0.35], valid: 1, received_tick: 0 };
        let palms = Palms { game, origin: IDENTITY_MATRIX, head, palms: [Some(palm(-1.0)), Some(palm(1.0))], aim_side: RIGHT_HAND, hold: [0.0; 3], bones: None };
        let pose = Pose { game, origin: IDENTITY_MATRIX, head, palms: palms.palms, aim_side: RIGHT_HAND, hold: [0.0; 3], gun: false, melee_holder: None, bones: BoneAxes::DYING_LIGHT_1 };
        let (old, new) = (named_arms(30), named_arms(31));
        let animated = new.worlds.borrow().clone();
        crate::follow(&old, &Snapshot::take(&old).unwrap(), &pose).unwrap();
        frame::<Chromish>(&new, &Snapshot::take(&new).unwrap(), Moment { hands: Hands::Palms(palms), fingers: false }).unwrap();
        assert_eq!(*old.worlds.borrow(), *new.worlds.borrow(), "the same bones, the same matrices");
        assert_eq!(*old.writes.borrow(), *new.writes.borrow(), "written alike");
        let hand = new.names.iter().position(|n| n == "r_hand").unwrap();
        assert!(camera::distance(&new.worlds.borrow()[hand], &animated[hand]) > 1e-3, "the hand moved");
        // Handed the same pose back (not animated again), both put back what they wrote.
        old.writes.borrow_mut().clear();
        new.writes.borrow_mut().clear();
        let written = new.worlds.borrow().clone();
        frame::<Chromish>(&new, &Snapshot::take(&new).unwrap(), Moment { hands: Hands::Palms(palms), fingers: false }).unwrap();
        assert_eq!(*new.worlds.borrow(), written);
        assert_eq!(new.writes.borrow().len(), new.names.len(), "every bone put back as written");
    }

    /// The holders as the rig left them are what a feature read back from the skeleton right after
    /// the rig before (the bow's holder at the arms callback): a written joint as written, a helper
    /// where the engine moved it with its bone.
    #[test]
    fn each_holder_is_read_back_as_the_rig_left_it() {
        let _serial = POSING.locked();
        // Hand-rooted: each attach joint is written.
        let arms = hand_rooted();
        let animated = Snapshot::take(&arms).unwrap();
        let target = [1.0, 0.0, 0.0, 12.0, 0.0, 1.0, 0.0, -10.0, 0.0, 0.0, 1.0, -8.0];
        frame::<Wolfish>(&arms, &animated, Moment { hands: Hands::Attached { right: 1, targets: [None, Some(target)] }, fingers: false }).unwrap();
        let rooted = posed().unwrap();
        assert_eq!(rooted.holders, [arms.world(10), arms.world(1)]);
        assert!(camera::distance(&rooted.holders[RIGHT_HAND].unwrap(), &target) < 1e-5);
        assert!(arms.writes.borrow().contains(&1) && arms.writes.borrow().contains(&10), "both written");
        // Carried: the holders are helpers, never written, moved with their hands.
        crate::declare_mirror(MIRROR);
        let mut arms = named_arms(32);
        let helpers = [arms.make_helper("l_handholder"), arms.make_helper("r_handholder")];
        let game = translation([0.0, 1.6, 0.0]);
        let head = HeadPose { orientation: [0.0, 0.0, 0.0, 1.0], position: [0.0, 1.6, 0.0], valid: 1, ..HeadPose::default() };
        let palm = |x: f32| HandPose { orientation: [0.0, 0.0, 0.0, 1.0], position: [0.25 * x, 1.25, -0.35], valid: 1, received_tick: 0 };
        let palms = Palms { game, origin: IDENTITY_MATRIX, head, palms: [Some(palm(-1.0)), Some(palm(1.0))], aim_side: RIGHT_HAND, hold: [0.0; 3], bones: None };
        let animated = Snapshot::take(&arms).unwrap();
        let before = animated.worlds[helpers[RIGHT_HAND]];
        frame::<Chromish>(&arms, &animated, Moment { hands: Hands::Palms(palms), fingers: false }).unwrap();
        assert!(!arms.writes.borrow().iter().any(|w| helpers.contains(w)), "a helper is never written");
        let carried = posed().unwrap();
        assert_eq!(carried.holders, [arms.world(5), arms.world(10)]);
        assert!(camera::distance(&carried.holders[RIGHT_HAND].unwrap(), &before) > 1e-3, "it moved with the hand");
        // A pose the rig cannot make leaves the last one.
        let refused = Moment { hands: Hands::Attached { right: 1, targets: [None, None] }, fingers: false };
        assert!(frame::<Chromish>(&arms, &Snapshot::take(&arms).unwrap(), refused).is_err());
        assert_eq!(posed(), Some(carried));
    }

    #[test]
    fn a_hand_reloading_the_other_gun_goes_to_where_that_gun_is() {
        let arms = hand_rooted();
        let parents = arms.parents.clone();
        let mut game = arms.worlds.borrow().clone();
        let (right, left) = (find_arm(&parents, 1).unwrap(), find_arm(&parents, 10).unwrap());
        // Holding their own guns, neither hand reaches.
        assert_eq!((reach(&game, &right, &left), reach(&game, &left, &right)), (0.0, 0.0));
        // The game brings the left wrist over to the right gun.
        game[11] = translation([15.0, -5.0, -12.0]);
        assert_eq!(reach(&game, &left, &right), 1.0);
        // The right gun is 10 units further right and turned 90 degrees about up.
        let right_move = [0.0, -1.0, 0.0, 0.0, 1.0, 0.0, 0.0, -10.0, 0.0, 0.0, 1.0, 0.0];
        let mut joints = game.clone();
        move_rigid(&mut joints, &right.subtree, &right_move);
        reach_across(&mut joints, &game, &parents, &left, &right, &right_move);
        let moved = |j: usize| position(&compose(&right_move, &game[j]));
        assert!(distance(position(&joints[11]), moved(11)) < 1e-4, "{:?} vs {:?}", position(&joints[11]), moved(11));
        // Its fingers come along; its own gun stays.
        assert!(distance(position(&joints[12]), moved(12)) < 1e-4);
        assert_eq!(joints[10], game[10]);
    }

    #[test]
    fn a_free_hand_takes_its_tracked_fingers_by_shape() {
        // A left hand palm down, fingers along +x: wrist 0 (forearm 1), a three-joint thumb (2-4)
        // on the right (-y), four-joint fingers index 5-8, middle 9-12, ring 13-16, little 17-20.
        let mut hand = Plain::new(40);
        hand.bone("", -1, IDENTITY_MATRIX, false);
        hand.bone("", 0, translation([-5.0, 0.0, 0.0]), false);
        for (len, y) in [(3, -3.0), (4, -1.5), (4, -0.5), (4, 0.5), (4, 1.5)] {
            for k in 0..len {
                let parent = if k == 0 { 0 } else { hand.names.len() as i32 - 1 };
                hand.bone("", parent, translation([2.0 + 2.0 * k as f32, y, 0.0]), false);
            }
        }
        let animated = Snapshot::take(&hand).unwrap();
        let mut frames = Frames::new(&animated);
        let tracked = FingerPose { valid: 1, fingers: [[0.8; 4]; 5], ..FingerPose::default() };
        assert!(!finger_rig::place(&hand, &mut frames, &ChainSource::Shape, LEFT_HAND, &tracked), "no wrist found yet");
        frames.wrists[LEFT_HAND] = Some((0, 1));
        assert!(finger_rig::place(&hand, &mut frames, &ChainSource::Shape, LEFT_HAND, &tracked));
        assert_eq!(frames.now[0], animated.worlds[0], "the wrist stays");
        assert_eq!(frames.now[5], animated.worlds[5], "the index's metacarpal stays");
        // Palm down: a curled index tip goes down, under the hand.
        assert!(frames.now[8][11] < -1.0, "{:?}", position(&frames.now[8]));
    }

    #[test]
    fn blends_run_from_one_frame_to_the_other() {
        let a = IDENTITY_MATRIX;
        let b = [0.0, -1.0, 0.0, 2.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0];
        assert!(camera::distance(&blend(&a, &b, 0.0), &a) < 1e-5 && camera::distance(&blend(&a, &b, 1.0), &b) < 1e-5);
        let (s, c) = 45f32.to_radians().sin_cos();
        assert!(camera::distance(&blend(&a, &b, 0.5), &[c, -s, 0.0, 1.0, s, c, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0]) < 1e-5);
    }
}
