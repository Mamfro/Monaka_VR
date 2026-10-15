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
    const MANIFESTS: &'static [monaka_contract::manifest::Manifest] = &[];
    const ADAPTER: Adapter<Self> = Adapter { components: &[] };
    type Build = ();
    type Frame = ();
    fn prepare(_: &'static monaka_producer::Session, _: &'static monaka_contract::manifest::Manifest, _: &monaka_core::options::Options) -> Result<(), monaka_producer::Rejection> {
        Ok(())
    }
}
impl HandRig for Wolfish {
    const RIG: Rig = Rig {
        method: Method::HandRooted { shoulders: [[-10.9, 10.0, -10.8], [-10.9, -10.0, -10.8]], poles: [[0.0, 0.6, -1.0], [0.0, -0.6, -1.0]], left_grip: IDENTITY_MATRIX, right_melee_grip: || None },
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
fn a_melee_hand_keeps_its_grip_whichever_way_the_game_holds_the_weapon() {
    let _serial = POSING.locked();
    let target = [1.0, 0.0, 0.0, 15.0, 0.0, 1.0, 0.0, -5.0, 0.0, 0.0, 1.0, -10.0];
    // The grip measured where it sits right: the wrist in the attach joint's frame.
    let forward = Snapshot::take(&hand_rooted()).unwrap();
    let grip = compose(&rigid_inverse(&forward.worlds[1]), &forward.worlds[2]);
    let rooted = HandRooted { shoulders: [[-10.9, 10.0, -10.8], [-10.9, -10.0, -10.8]], poles: [[0.0, 0.6, -1.0], [0.0, -0.6, -1.0]], left_grip: IDENTITY_MATRIX, right_melee_grip: Some(grip), held: [Kind::Nothing, Kind::Melee] };
    // The knife the other way round: its attach joint turned half a turn in the hand.
    let flipped = hand_rooted();
    let at = flipped.worlds.borrow()[1];
    flipped.worlds.borrow_mut()[1] = [1.0, 0.0, 0.0, at[3], 0.0, -1.0, 0.0, at[7], 0.0, 0.0, -1.0, at[11]];
    let knife = Snapshot::take(&flipped).unwrap();
    let mut frames = Frames::new(&knife);
    rooted.place(&flipped, &mut frames, 1, [None, Some(target)]).unwrap();
    let wanted = compose(&target, &grip);
    assert!(camera::distance(&frames.now[2], &wanted) < 1e-4, "the hand keeps its grip: {:?} {wanted:?}", frames.now[2]);
    // The knife's joint hangs in the hand as the game holds it: turned against the target.
    assert!(camera::distance(&frames.now[1], &target) > 0.5, "the knife turns with the game's grip");
}

#[test]
fn with_no_controller_tracked_the_arms_stay_as_animated() {
    let _serial = POSING.locked();
    let arms = hand_rooted();
    let animated = Snapshot::take(&arms).unwrap();
    let moment = Moment { hands: Hands::Attached { right: 1, targets: [None, None], space: IDENTITY_MATRIX }, fingers: false };
    assert_eq!(frame::<Wolfish>(&arms, &animated, moment).err(), Some("no controller tracked"));
    assert_eq!(*arms.worlds.borrow(), animated.worlds, "nothing written");
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
    let moment = Moment { hands: Hands::Attached { right: 1, targets: [None, Some(target)], space: IDENTITY_MATRIX }, fingers: false };
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
    const MANIFESTS: &'static [monaka_contract::manifest::Manifest] = &[];
    const ADAPTER: Adapter<Self> = Adapter { components: &[] };
    type Build = ();
    type Frame = ();
    fn prepare(_: &'static monaka_producer::Session, _: &'static monaka_contract::manifest::Manifest, _: &monaka_core::options::Options) -> Result<(), monaka_producer::Rejection> {
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

/// A game whose right hand sometimes holds a sword ([`SWORD`]): the rig locks it to the controller
/// by its holder only then.
struct Swordish;
static SWORD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// The sword's holder in the palm: turned a quarter about the palm's forward.
fn sword_in_palm() -> Mat34 {
    [0.0, -1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0]
}
impl Game for Swordish {
    const MANIFESTS: &'static [monaka_contract::manifest::Manifest] = &[];
    const ADAPTER: Adapter<Self> = Adapter { components: &[] };
    type Build = ();
    type Frame = ();
    fn prepare(_: &'static monaka_producer::Session, _: &'static monaka_contract::manifest::Manifest, _: &monaka_core::options::Options) -> Result<(), monaka_producer::Rejection> {
        Ok(())
    }
}
impl HandRig for Swordish {
    const RIG: Rig = Rig {
        method: Method::Carried { bones: BoneAxes::DYING_LIGHT_1, mirror: Some(MIRROR), melee_holder: Some(sword_in_palm) },
        holds: Holds::TwoHanded,
        climbing: Climbing::Animated,
        fingers: None,
    };
    fn held(side: usize) -> Kind {
        if side == RIGHT_HAND && SWORD.load(std::sync::atomic::Ordering::Relaxed) { Kind::Blade } else { Kind::Nothing }
    }
}

#[test]
fn a_melee_holder_holds_the_weapon_only_while_the_aim_hand_has_one() {
    let _serial = POSING.locked();
    crate::declare_mirror(MIRROR);
    let game = translation([0.0, 1.6, 0.0]);
    let head = HeadPose { orientation: [0.0, 0.0, 0.0, 1.0], position: [0.0, 1.6, 0.0], valid: 1, ..HeadPose::default() };
    let palm = |x: f32| HandPose { orientation: [0.0, 0.0, 0.0, 1.0], position: [0.25 * x, 1.25, -0.35], valid: 1, received_tick: 0 };
    let palms = Palms { game, origin: IDENTITY_MATRIX, head, palms: [Some(palm(-1.0)), Some(palm(1.0))], aim_side: RIGHT_HAND, hold: [0.0; 3], bones: None };
    let followed = |id: usize, melee_holder: Option<Mat34>| {
        let arms = named_arms(id);
        let pose = Pose { game, origin: IDENTITY_MATRIX, head, palms: palms.palms, aim_side: RIGHT_HAND, hold: [0.0; 3], gun: false, melee_holder, bones: BoneAxes::DYING_LIGHT_1 };
        crate::follow(&arms, &Snapshot::take(&arms).unwrap(), &pose).unwrap();
        arms.worlds.into_inner()
    };
    let rigged = |id: usize, sword: bool| {
        SWORD.store(sword, std::sync::atomic::Ordering::Relaxed);
        let arms = named_arms(id);
        frame::<Swordish>(&arms, &Snapshot::take(&arms).unwrap(), Moment { hands: Hands::Palms(palms), fingers: false }).unwrap();
        arms.worlds.into_inner()
    };
    assert_eq!(rigged(40, true), followed(41, Some(sword_in_palm())), "a sword: held by its holder");
    assert_eq!(rigged(42, false), followed(43, None), "an empty hand: by the wrist");
    assert_ne!(followed(44, None), followed(45, Some(sword_in_palm())), "the holder moves the hand");
}

/// The holders as the rig left them are what a feature read back from the skeleton right after
/// the rig before (the bow's holder at the arms callback): a written joint as written, a helper
/// where the engine moved it with its bone.
#[test]
fn the_debug_views_draw_the_bones_for_the_frame_being_drawn() {
    use monaka_channel::debug::{DebugChannel, Source, Wanted};
    use monaka_contract::debug_draw::{Categories, Category, ORANGE};
    let _serial = POSING.locked();
    crate::declare_mirror(MIRROR);
    let name = monaka_channel::ChannelName::new(format!(r"Local\monaka-test-rig-debug-{}", std::process::id())).unwrap();
    let mut viewer = DebugChannel::open(&name).unwrap();
    viewer.want(Wanted { categories: Categories::NONE.with(Category::Skeleton, true), ..Wanted::default() });
    monaka_producer::debug::open(&name);
    monaka_producer::debug::set_drawing(5);
    let game = translation([0.0, 1.6, 0.0]);
    let head = HeadPose { orientation: [0.0, 0.0, 0.0, 1.0], position: [0.0, 1.6, 0.0], valid: 1, ..HeadPose::default() };
    let palm = |x: f32| HandPose { orientation: [0.0, 0.0, 0.0, 1.0], position: [0.25 * x, 1.25, -0.35], valid: 1, received_tick: 0 };
    let palms = Palms { game, origin: IDENTITY_MATRIX, head, palms: [Some(palm(-1.0)), Some(palm(1.0))], aim_side: RIGHT_HAND, hold: [0.0; 3], bones: None };
    let arms = named_arms(31);
    frame::<Chromish>(&arms, &Snapshot::take(&arms).unwrap(), Moment { hands: Hands::Palms(palms), fingers: false }).unwrap();
    monaka_producer::debug::eye_keys(1, [5, 5]);
    monaka_producer::debug::close();
    let mut eyes = [Vec::new(), Vec::new()];
    assert_eq!(viewer.read_pair(1, &mut eyes), [Source::Synced(5); 2]);
    assert!(eyes[0].iter().all(|p| p.category == Category::Skeleton as u16));
    assert!(eyes[0].iter().any(|p| p.colour == ORANGE), "the bones the rig moved: {:?}", eyes[0].len());
}

/// A skeleton hung from the hands is drawn through its moment's space: model units and axes
/// into tracking space, scale included.
#[test]
fn a_hand_rooted_skeleton_is_drawn_through_its_moments_space() {
    use monaka_channel::debug::{DebugChannel, Source, Wanted};
    use monaka_contract::debug_draw::{Categories, Category};
    let _serial = POSING.locked();
    let name = monaka_channel::ChannelName::new(format!(r"Local\monaka-test-rooted-debug-{}", std::process::id())).unwrap();
    let mut viewer = DebugChannel::open(&name).unwrap();
    viewer.want(Wanted { categories: Categories::NONE.with(Category::Skeleton, true), ..Wanted::default() });
    monaka_producer::debug::open(&name);
    monaka_producer::debug::set_drawing(7);
    let arms = hand_rooted();
    let animated = Snapshot::take(&arms).unwrap();
    let target = [1.0, 0.0, 0.0, 12.0, 0.0, 1.0, 0.0, -10.0, 0.0, 0.0, 1.0, -8.0];
    // Model x forward, y left, z up in game units of 2.5 cm, to tracking (x right, y up, z back):
    // the model's origin at (0.1, 1.5, -0.2).
    let u = 0.025;
    let space = [0.0, -u, 0.0, 0.1, 0.0, 0.0, u, 1.5, -u, 0.0, 0.0, -0.2];
    frame::<Wolfish>(&arms, &animated, Moment { hands: Hands::Attached { right: 1, targets: [None, Some(target)], space }, fingers: false }).unwrap();
    monaka_producer::debug::eye_keys(1, [7, 7]);
    monaka_producer::debug::close();
    let mut eyes = [Vec::new(), Vec::new()];
    assert_eq!(viewer.read_pair(1, &mut eyes), [Source::Synced(7); 2]);
    // The right attach joint, at the target, ends a bone: 12 forward, 10 right, 8 down.
    let expected = [0.1 + 10.0 * u, 1.5 - 8.0 * u, -0.2 - 12.0 * u];
    let near = |p: [f32; 3]| distance(p, expected) < 1e-4;
    assert!(eyes[0].iter().any(|p| near(p.a) || near(p.b)), "the attach joint in tracking space: {:?}", eyes[0].iter().map(|p| p.b).collect::<Vec<_>>());
}

#[test]
fn each_holder_is_read_back_as_the_rig_left_it() {
    let _serial = POSING.locked();
    // Hand-rooted: each attach joint is written.
    let arms = hand_rooted();
    let animated = Snapshot::take(&arms).unwrap();
    let target = [1.0, 0.0, 0.0, 12.0, 0.0, 1.0, 0.0, -10.0, 0.0, 0.0, 1.0, -8.0];
    frame::<Wolfish>(&arms, &animated, Moment { hands: Hands::Attached { right: 1, targets: [None, Some(target)], space: IDENTITY_MATRIX }, fingers: false }).unwrap();
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
    let refused = Moment { hands: Hands::Attached { right: 1, targets: [None, None], space: IDENTITY_MATRIX }, fingers: false };
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

#[test]
fn a_melee_hand_keeps_its_resting_fingers_while_the_game_twirls_the_weapon() {
    let _serial = POSING.locked();
    let target = [1.0, 0.0, 0.0, 15.0, 0.0, 1.0, 0.0, -5.0, 0.0, 0.0, 1.0, -10.0];
    let resting = hand_rooted();
    let at_rest = Snapshot::take(&resting).unwrap();
    let grip = compose(&rigid_inverse(&at_rest.worlds[1]), &at_rest.worlds[2]);
    let rooted = HandRooted { shoulders: [[-10.9, 10.0, -10.8], [-10.9, -10.0, -10.8]], poles: [[0.0, 0.6, -1.0], [0.0, -0.6, -1.0]], left_grip: IDENTITY_MATRIX, right_melee_grip: Some(grip), held: [Kind::Nothing, Kind::Melee] };
    // At the resting grip the game's fingers are the hand's.
    let mut frames = Frames::new(&at_rest);
    rooted.place(&resting, &mut frames, 1, [None, Some(target)]).unwrap();
    let rested = frames.now.clone();
    // A twirl: the knife half a turn in the hand, one finger of the game's somewhere else.
    let twirled = hand_rooted();
    {
        let mut worlds = twirled.worlds.borrow_mut();
        let at = worlds[1];
        worlds[1] = [1.0, 0.0, 0.0, at[3], 0.0, -1.0, 0.0, at[7], 0.0, 0.0, -1.0, at[11]];
        worlds[3][3] += 2.0;
    }
    let twirling = Snapshot::take(&twirled).unwrap();
    let mut frames = Frames::new(&twirling);
    rooted.place(&twirled, &mut frames, 1, [None, Some(target)]).unwrap();
    for finger in [3, 4, 5] {
        assert!(camera::distance(&frames.now[finger], &rested[finger]) < 1e-4, "finger {finger} keeps its resting place");
    }
}

#[test]
fn the_grip_held_longest_comes_first() {
    let _serial = POSING.locked();
    MELEE_GRIPS.locked().clear();
    let forward = IDENTITY_MATRIX;
    let reversed = [1.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0];
    let nudged = [1.0, 0.0, 0.0, 0.1, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0];
    for grip in [forward, reversed, nudged, forward] {
        tally_melee_grip(&grip);
    }
    let held = melee_grips_held_longest(2);
    MELEE_GRIPS.locked().clear();
    assert_eq!(held[0], (forward, 0.75), "a nudge counts as the same grip");
    assert_eq!(held[1], (reversed, 0.25));
}
