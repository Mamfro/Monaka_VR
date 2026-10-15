//! The arms on the controllers for a skeleton of named elements ([`follow`], the hand rig's
//! [`crate::hand_rig::Method::Carried`]): each upper arm and forearm reaching its hand from a
//! shoulder under the player's real head, the aim wrist (or what it holds) in its controller's
//! palm, the off hand on the weapon or on its own controller, the rest of the body under the head.
//! Its parts: the arms found on the skeleton and the hands' calibrations ([`arms`]), whether the
//! off hand rides a weapon held two-handed ([`hold`]), and the checks that keep a pose that cannot
//! be the player's from being written ([`checks`]).

mod arms;
mod checks;
mod hold;
#[cfg(test)]
mod tests;

pub use arms::{BoneAxes, MELEE_BLADE, MELEE_GRIP, declare_mirror};
pub use hold::{declare_one_handed, hands_apart, off_hand_on_weapon};

use crate::skeleton::{Frames, Skeleton, Snapshot};
use crate::{hand_rig, melee, traverse};
use arms::{ARMS, Arms, FLIP, FLIP_AGREEING, FLIP_VOTE, GUN_HOLDER_IN_PALM, Part, mirror_flip, orthonormal, rotation_of, turned, wrist_in_palm};
use checks::{
    SWAY_SHIFT, SWAY_TURN, log_first_missing_arm, near, note_implausible, note_mostly_ours, note_reanimation_missed, note_scaled, note_swayed, plausible_hand, plausible_head,
    plausible_placement, written_apart,
};
use hold::{Hold, note_hold, two_handed};
use monaka_core::camera::{self as cam, Mat34, apply_head, compose, position, rigid_inverse, translation};
use monaka_core::math::{Vec3, unit, yaw, yaw_rotation};
use monaka_contract::protocol::{HandPose, HeadPose, LEFT_HAND, RIGHT_HAND};
use monaka_core::reach::{apply, arm_moves, spin_about, twist_between};
use monaka_producer::log;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering::*};

/// How much of the elbow's bend direction comes from the hand's pose ([`monaka_core::reach::hand_pole`]);
/// the rest is the default, down and out.
const ELBOW_FROM_HAND: f32 = 0.75;

/// What one update puts the arms on.
pub struct Pose {
    /// The player camera as the game set it (camera-to-world).
    pub game: Mat34,
    /// Where the headset's tracking space sits in the world: the same origin the view is drawn
    /// from, so a hand lands where its controller is seen.
    pub origin: Mat34,
    pub head: HeadPose,
    /// Each controller's palm in tracking space (`HandPose::palm`), by side, when tracked.
    pub palms: [Option<HandPose>; 2],
    /// The controller that aims (`LEFT_HAND` or `RIGHT_HAND`).
    pub aim_side: usize,
    /// The wrist moved this far in the palm's space (metres; x right, y up, z back).
    pub hold: [f32; 3],
    /// A gun is drawn (it sits in the aim palm by its grip).
    pub gun: bool,
    /// A melee weapon locked to the aim controller: its holder (`r_handholder`) in the palm's frame
    /// (the game's own bone axes). The hand goes around it as the game animates the grip, and the
    /// game's guards no longer turn the weapon in the hand (Kingdom Come II re-grips the sword at
    /// every guard change). None: the wrist is pinned and the game holds the weapon.
    pub melee_holder: Option<Mat34>,
    /// The game's bone axes against DL1's.
    pub bones: BoneAxes,
}

/// What [`follow`] placed, for a producer's diagnostics.
#[derive(Clone, Copy)]
pub struct Placed {
    /// The aim palm in the world.
    pub palm: Mat34,
}

/// Puts the arms, as animated (`before`), on the controllers, each upper arm and forearm reaching
/// its hand from a shoulder under the real head. The aim hand: with a gun, the gun in the palm
/// pointing where the controller aims; otherwise the wrist in the palm. The off hand: on the weapon
/// as the game holds it when it holds it two-handed; otherwise its wrist in the off controller's
/// palm; carried with the body when that controller is not tracked. Every bone is written: setting
/// one marks its children for recomputation, and the ones the game set itself must lose its own
/// adjustments (Dying Light 1 squashes the whole rig toward the camera for the flat screen; left to
/// the game, the body stays flattened front to back). Err (with why) when there is no aim
/// controller or no arm to move. The hand rig's [`hand_rig::Method::Carried`]: a game on the
/// ready-made ([`hand_rig`]) gets this with its fingers in the same write.
pub fn follow(skeleton: &dyn Skeleton, before: &Snapshot, pose: &Pose) -> Result<Placed, &'static str> {
    let mut frames = Frames::new(before);
    let placed = carry(skeleton, before, pose, &mut frames)?;
    let written = frames.flush(skeleton);
    remember(before.skeleton, &frames, &written, placed.palm);
    Ok(placed)
}

/// What [`follow`] puts the arms at, into `frames` (every bone placed), without writing them.
pub(crate) fn carry(skeleton: &dyn Skeleton, before: &Snapshot, pose: &Pose, frames: &mut Frames) -> Result<Placed, &'static str> {
    // Poses that are not a tracked hand or head (not finite, no rotation, kilometres out) are
    // taken as untracked: placed on them, the bones would go wherever they said.
    let palms = pose.palms.map(|p| p.filter(plausible_hand));
    let Some(hand) = palms.get(pose.aim_side).copied().flatten() else { return Err("no controller") };
    let (aim_side, off_side) = (pose.aim_side.min(1), 1 - pose.aim_side.min(1));
    if before.skeleton != skeleton.id() || skeleton.count() as usize != before.worlds.len() {
        return Err("skeleton changed");
    }
    // The arms as the game animated them. A game that did not animate them again since the last
    // move (paused, or calling twice in one frame) hands back what was written: moved again, the
    // move would pile up every call until the wrists were far out and the arms flickered. Then the
    // bones are put back exactly as written, nothing solved: solving on through a pause from the
    // last animation ends in an arm that cannot reach its hand, and Dying Light 1 then reads a null
    // in a nearest-of-a-list search that finds nothing for a position not finite or far out.
    let Ok(mut memory) = MEMORY.lock() else { return Err("lock") };
    let Memory { skeleton: remembered, animated, written, palm: last_palm } = &mut *memory;
    let written: &[Mat34] = written;
    // Within a sway of it counts too: holding a rifle, the game's own small motion (its sway) goes
    // on in the pause menu, on top of the bones as written; taken for an animation, the arms would
    // be moved again every call and fly off along the look.
    let same = *remembered == before.skeleton && written.len() == before.worlds.len() && written.iter().any(|w| w[0].is_finite());
    let apart = same.then(|| written_apart(written, &before.worlds));
    let ours = apart.is_some_and(|(turn, shift)| turn <= SWAY_TURN && shift <= SWAY_SHIFT);
    if let Some((turn, shift)) = apart.filter(|&(turn, shift)| ours && (turn > 0.0 || shift > 0.0)) {
        note_swayed(turn, shift);
    }
    if !ours && same {
        note_mostly_ours(skeleton, written, &before.worlds);
    }
    if ours {
        note_reanimation_missed();
        for (element, placed) in written.iter().enumerate().filter(|(_, w)| w[0].is_finite()) {
            frames.place(element, *placed);
        }
        return Ok(Placed { palm: *last_palm });
    }
    // Arms handed back nowhere near the camera are not the player's arms in play (a cutscene, a
    // load): left to the game.
    if !near(&before.worlds, &pose.game) {
        return Err("arms far from the camera");
    }
    *remembered = before.skeleton;
    animated.clear();
    animated.extend_from_slice(&before.worlds);
    let animated: &[Mat34] = animated;
    let Ok(mut arms) = ARMS.lock() else { return Err("lock") };
    if !arms.as_ref().is_some_and(|a| a.skeleton == before.skeleton && a.parts.len() == before.worlds.len()) {
        *arms = Arms::find(skeleton);
        if arms.is_none() {
            log_first_missing_arm();
        }
    }
    let Some(arms) = arms.as_mut() else { return Err("no arm") };
    let world = |element: i32| &animated[element as usize];
    let game = pose.game;
    let view = rigid_inverse(&game);
    let flip = {
        let Ok(mut known) = FLIP.lock() else { return Err("lock") };
        if known.is_none() {
            let readings: Vec<Option<[f32; 9]>> = arms.pairs.iter().map(|&[l, r]| mirror_flip(&rotation_of(&compose(&view, world(r))), &rotation_of(&compose(&view, world(l))))).collect();
            // Pairs not posed alike (legs mid-stride) read nothing; at least two must read, alike.
            let read: Vec<[f32; 9]> = readings.iter().flatten().copied().collect();
            let agreed = read.first().copied().filter(|first| read.len() >= 2 && read.iter().all(|r| r == first));
            if let Ok(mut vote) = FLIP_VOTE.lock() {
                *vote = match agreed {
                    Some(flip) if vote.0 == flip => (flip, vote.1 + 1),
                    Some(flip) => (flip, 1),
                    None => ([0.0; 9], 0),
                };
                if vote.1 >= FLIP_AGREEING {
                    log!("hand rig: left bones mirror the right ones with {:?} ({} of {} pairs agreeing, {FLIP_AGREEING} updates running)", vote.0, read.len(), readings.len());
                    *known = Some(vote.0);
                }
            }
        }
        *known
    };
    // Everything under a wrist moves with what is pinned.
    let origin = pose.origin;
    let palm = compose(&apply_head(&origin, hand.orientation, hand.position), &translation(pose.hold));
    let aim = arms.sides[aim_side];
    melee::note_animated_wrist(position(&compose(&view, world(aim.joints[2]))));
    // The rig's share of the arms: none while the game's animation climbs or hangs with them.
    let rig = traverse::update([LEFT_HAND, RIGHT_HAND].map(|side| position(&compose(&view, world(arms.sides[side].joints[2])))), &game);
    let holder = aim.holder.filter(|_| aim_side == RIGHT_HAND);
    let held = match (holder, pose.gun, pose.melee_holder) {
        (Some(holder), true, _) => Some((holder, turned(&GUN_HOLDER_IN_PALM, &pose.bones.holder), Hold::Gun)),
        (Some(holder), false, Some(in_palm)) => Some((holder, in_palm, Hold::Melee)),
        _ => None,
    };
    note_hold(held.map_or(Hold::Wrist, |h| h.2));
    let (pinned, in_palm) = match held {
        Some((holder, in_palm, _)) => (world(holder), in_palm),
        None => (world(aim.joints[2]), wrist_in_palm(aim_side, flip.as_ref(), &pose.bones.wrist).ok_or("no wrist calibration")?),
    };
    // From the rigid part of what the game animated: a scaled wrist inverted as if rigid would make
    // the move scale whatever it moved (the hands several times their size).
    note_scaled("aim wrist", pinned);
    let aim_move = compose(&compose(&palm, &in_palm), &rigid_inverse(&orthonormal(pinned)));
    if !aim_move.iter().all(|v| v.is_finite()) {
        static LOGGED: AtomicU64 = AtomicU64::new(0);
        if LOGGED.fetch_add(1, Relaxed) < 3 {
            log!("hand rig: not finite: palm {palm:?} pinned {pinned:?} game {game:?} hand {hand:?}");
        }
        return Err("not finite");
    }
    // The off hand: on the weapon when the game holds it two-handed, else on its own controller.
    // Only a weapon is held two-handed: with the aim hand reported empty, an animation bringing
    // the hands together never hands the off arm the aim controller's move.
    let off = arms.sides[off_side];
    let anchor = position(world(aim.holder.unwrap_or(aim.joints[2])));
    let off_wrist = position(world(off.joints[2]));
    let holds_weapon = crate::held::latest(aim_side).is_some_and(|held| matches!(held.kind, crate::held::Kind::Gun | crate::held::Kind::Melee | crate::held::Kind::Blade));
    let both_hands = two_handed(if holds_weapon { monaka_core::math::distance(anchor, off_wrist) } else { f32::INFINITY });
    let off_move = if both_hands {
        Some(aim_move)
    } else {
        palms[off_side].and_then(|off_hand| {
            let off_palm = compose(&apply_head(&origin, off_hand.orientation, off_hand.position), &translation(pose.hold));
            let in_palm = wrist_in_palm(off_side, flip.as_ref(), &pose.bones.wrist)?;
            note_scaled("off wrist", world(off.joints[2]));
            Some(compose(&compose(&off_palm, &in_palm), &rigid_inverse(&orthonormal(world(off.joints[2])))))
        })
    }
    .filter(|m| m.iter().all(|v| v.is_finite()));
    // The shoulders where the game animates them against its levelled camera, carried under the
    // real head (yaw only: nodding does not swing the shoulders).
    let Some(flat_game) = cam::level(&game) else { return Err("camera not levelled") };
    let head = pose.head;
    let body = if head.valid != 0 && plausible_head(&head) { apply_head(&origin, yaw_rotation(yaw(head.orientation)), head.position) } else { flat_game };
    let carry = compose(&body, &rigid_inverse(&flat_game));
    let mut moves: [Option<[Mat34; 3]>; 2] = [None, None];
    // Each forearm's elbow, its axis toward the wrist and how far the hand rolls about it.
    let mut twists: [Option<(Vec3, Vec3, f32)>; 2] = [None, None];
    for (side, hand_move) in [(aim_side, Some(aim_move)), (off_side, off_move)] {
        let Some(hand_move) = hand_move else { continue };
        let arm = arms.sides[side];
        let joint = |i: usize| position(world(arm.joints[i]));
        let target = apply(&hand_move, joint(2));
        let shoulder = apply(&carry, joint(0));
        // The elbow bends down and out to the arm's side; a hand on a two-handed weapon's front
        // keeps its elbow down under it.
        let out = match side {
            _ if side == off_side && both_hands => 0.0,
            LEFT_HAND => -0.6,
            _ => 0.6,
        };
        let base = [body[0] * out, body[4] * out - 1.0, body[8] * out];
        // Mostly toward where the hand's own pose puts the forearm (a hand turned to read a watch
        // lifts the elbow out and up), with the default keeping it steady; not for a hand on a
        // two-handed weapon's front, whose controller is elsewhere.
        let forearm = monaka_core::math::distance(joint(1), joint(2));
        let hand_pole = palms[side]
            .filter(|_| side == aim_side || !both_hands)
            .and_then(|h| monaka_core::reach::hand_pole(shoulder, target, &apply_head(&origin, h.orientation, h.position), forearm));
        let pole = match (hand_pole, unit(base)) {
            (Some(hand), Some(base)) => [0, 1, 2].map(|i| hand[i] * ELBOW_FROM_HAND + base[i] * (1.0 - ELBOW_FROM_HAND)),
            _ => base,
        };
        // An arm the reach cannot solve this update (a degenerate animated pose) keeps its last
        // solution for a moment, then goes rigidly with the hand: skipping the move would draw the
        // arm where the game put it for that frame, a flicker.
        let [upper, lower] = match arm_moves([joint(0), joint(1), joint(2)], shoulder, target, pole) {
            Some(solved) => {
                if let Ok(mut last) = LAST_REACH.lock() {
                    last[side] = Some((solved, monaka_channel::tick()));
                }
                solved
            }
            None => {
                note_reach_failure();
                let recent = LAST_REACH.lock().ok().and_then(|last| last[side]).filter(|(_, at)| monaka_channel::tick().saturating_sub(*at) < 250);
                recent.map_or([hand_move; 2], |(solved, _)| solved)
            }
        };
        moves[side] = Some([upper, lower, hand_move]);
        // The hand's roll about the forearm beyond the animation's, for the twist bones.
        let elbow_at = apply(&lower, joint(1));
        let axis = [target[0] - elbow_at[0], target[1] - elbow_at[1], target[2] - elbow_at[2]];
        twists[side] = twist_between(&lower, &hand_move, axis).map(|angle| (elbow_at, axis, angle));
    }
    // Every bone from the snapshot. Elements that are not bones stay: the helpers (`r_handholder`,
    // which the weapon hangs on, ...) follow their moved bones, and moving the helper `hspine1`
    // (the root Dying Light 1 squashes) or the mesh parts (`player_6_handr_fpp`, ...) leaves the
    // arms where the game put them, out of view, with only the weapon at the controller.
    // Climbing, every bone goes where the game animated it in the world, blended: the hands stay
    // on the ledge. (Carried under the head, the arms would turn with the player's sight while the
    // game, left alone on a ledge, does not turn the character: the hands would come off it.)
    let in_place = translation([0.0; 3]);
    let pivot = position(&game);
    let body_move = blend(&in_place, &carry, rig, pivot);
    let arm = |side: usize, part: usize| moves[side].map_or(body_move, |m| blend(&in_place, &m[part], rig, pivot));
    let mut placements = Vec::with_capacity(animated.len());
    for (element, world) in (0..skeleton.count()).zip(animated) {
        let mover = match arms.parts[element as usize] {
            Part::Body | Part::Clavicle(_) => body_move,
            Part::UpperArm(side) => arm(side, 0),
            Part::Forearm(side) => arm(side, 1),
            Part::Twist(side, share) => match (moves[side], twists[side]) {
                (Some(m), Some((elbow, axis, angle))) => {
                    let twisted = spin_about(elbow, axis, angle * share).map_or(m[1], |spin| compose(&spin, &m[1]));
                    blend(&in_place, &twisted, rig, pivot)
                }
                _ => arm(side, 1),
            },
            Part::Hand(side) => arm(side, 2),
        };
        // A bone the game left not finite stays as it is (a Dying Light 2 glove after a cutscene).
        if world.iter().all(|v| v.is_finite()) && skeleton.is_bone(element) {
            placements.push((element, compose(&mover, world), world));
        }
    }
    // All or nothing: a skeleton half moved is torn, and one bone far out is enough for the game
    // to look up nothing near it.
    if let Some(&(element, placed, _)) = placements.iter().find(|(_, placed, world)| !plausible_placement(placed, world, &game)) {
        note_implausible(&skeleton.name(element), &placed, &game);
        return Err("implausible placement");
    }
    for (element, placed, _) in placements {
        frames.place(element as usize, placed);
    }
    Ok(Placed { palm })
}

/// Keeps what a carried update wrote to `skeleton` (`written`, by element, from `frames`) and the
/// aim palm it went on: a game that hands the same pose back has not animated the arms again.
pub(crate) fn remember(skeleton: usize, frames: &Frames, written: &[usize], palm: Mat34) {
    let Ok(mut memory) = MEMORY.lock() else { return };
    if memory.skeleton != skeleton {
        return;
    }
    memory.written.clear();
    memory.written.resize(frames.now.len(), [f32::NAN; 12]);
    for &element in written {
        memory.written[element] = frames.now[element];
    }
    memory.palm = palm;
}

/// The arms the game last animated, the bones as [`follow`] last wrote them (NaN for the rest) and
/// the aim palm they were put on.
struct Memory {
    skeleton: usize,
    animated: Vec<Mat34>,
    written: Vec<Mat34>,
    palm: Mat34,
}

static MEMORY: Mutex<Memory> = Mutex::new(Memory { skeleton: 0, animated: Vec::new(), written: Vec::new(), palm: [0.0; 12] });

/// `weight` of the way from one rigid move to another (0 `from`, 1 `to`): the matrices mixed and
/// made rigid again about `pivot`, which goes where the mix takes it. Linear in what it does to a
/// point, so an elbow both arm bones move alike stays joined, but for that last rounding, which
/// grows with the distance from `pivot`: made rigid about the world's origin, hundreds of metres
/// off in a game's map, it would throw the arms tens of metres on every climb's blend.
fn blend(from: &Mat34, to: &Mat34, weight: f32, pivot: [f32; 3]) -> Mat34 {
    if weight >= 1.0 {
        return *to;
    }
    if weight <= 0.0 {
        return *from;
    }
    let mixed: Mat34 = std::array::from_fn(|i| from[i] + (to[i] - from[i]) * weight);
    let mut rigid = orthonormal(&mixed);
    let (target, turned) = (apply(&mixed, pivot), apply(&rigid, pivot));
    for (row, (t, r)) in target.iter().zip(turned).enumerate() {
        rigid[row * 4 + 3] += t - r;
    }
    match rigid.iter().all(|v| v.is_finite()) {
        true => rigid,
        false if weight < 0.5 => *from,
        false => *to,
    }
}

/// An arm's last solved upper-arm and forearm moves and when (`monaka_channel::tick`).
type Reach = Option<([Mat34; 2], u64)>;
/// Each arm's, by side.
static LAST_REACH: Mutex<[Reach; 2]> = Mutex::new([None, None]);
static REACH_FAILURES: AtomicU64 = AtomicU64::new(0);

fn note_reach_failure() {
    let n = REACH_FAILURES.fetch_add(1, Relaxed) + 1;
    if log::seldom(n, 3) {
        log!("hand rig: the arm could not reach the hand {n}x (kept the last reach or moved it with the hand)");
    }
}

/// End-of-run tally (the producer logs its own call counts).
pub fn report() {
    let failures = REACH_FAILURES.load(Relaxed);
    if failures > 0 {
        log!("  arm reach failed {failures}x (covered)");
    }
    traverse::report();
    melee::report();
    hand_rig::report();
}
