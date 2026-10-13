//! A hand's fingers found on a game skeleton from its reference pose, for [`crate::fingers`]: the
//! game says which bones are which finger ([`Chains`]: by name, by the skeleton's shape, or as a
//! table), and each knuckle's curl axis comes from the reference pose's own geometry, not from a
//! game's bone-axis convention (rigs differ per game, per side and even per joint). Fingers curl
//! about the line across the knuckles, toward the palm; the thumb's two tracked bends fold it
//! across the palm instead, in their own plane.
//!
//! Every frame is row-major 3x4 in one right-handed space (a left-handed engine mirrors its frames
//! before and after).
//!
//! On a [`Skeleton`] (the hand rig's provider) a game says once where its fingers are
//! ([`ChainSource`]), and [`place`] puts a hand's tracked fingers on an update's frames: each hand
//! is found once per skeleton as a small skeleton of its own (the wrist and everything under it),
//! its reference the engine's rest poses where it keeps them and the pose first seen elsewhere,
//! and posed in the wrist's own frame (a scale in the world matrices, as one engine squashes its
//! arms toward the camera, cancels there). A game with fingers and no hand rig says where they are
//! by [`FingerRig`] and calls [`frame`].

use crate::fingers::{self, Finger, FingerJoints, Hand};
use crate::held::Kind;
use crate::skeleton::finger_chains;
use crate::{Frames, Skeleton, Snapshot};
use monaka_core::camera::{self, IDENTITY_MATRIX, Mat34, position};
use monaka_core::math::{Vec3, cross, distance, dot, sub, unit};
use monaka_core::protocol::{FingerPose, LEFT_HAND, RIGHT_HAND};
use monaka_core::sync::Locked;
use monaka_producer::game::Game;
use monaka_producer::log;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex};

/// Which bones are a skeleton's hands and fingers, by index.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Chains {
    /// The hand bones, left then right.
    pub wrists: [usize; 2],
    /// Per side, the curled bones of each finger (thumb, index, middle, ring, little) from the
    /// hand out: two or three each, each the child of the one before, or none for a finger the
    /// hand lacks (the index, middle and little fingers set the palm, so they must be there; a
    /// side with no fingers at all is no hand). A finger's first bone may hang from a bone between
    /// it and the hand (a metacarpal), which stays. Bones below the last (a tip, helpers) ride it.
    pub fingers: [[Vec<usize>; 5]; 2],
}

/// A skeleton's finger bones by name: `{side}`, `{finger}` and `{n}` in [`FingerNames::bone`]
/// stand for a side's part ([`FingerNames::sides`]), a finger's ([`FingerNames::fingers`]) and the
/// bone's number from the hand out (1 to 3).
#[derive(Clone, Copy, Debug)]
pub struct FingerNames {
    /// The hand bones, left then right.
    pub wrists: [&'static str; 2],
    pub sides: [&'static str; 2],
    /// Thumb, index, middle, ring, little.
    pub fingers: [&'static str; 5],
    pub bone: &'static str,
}

impl FingerNames {
    /// The name of `finger`'s bone `n` (1 to 3) on `side`.
    pub fn name(&self, side: usize, finger: usize, n: usize) -> String {
        self.bone.replace("{side}", self.sides[side]).replace("{finger}", self.fingers[finger]).replace("{n}", &n.to_string())
    }

    /// Side `side`'s wrist and fingers (three bones each), looked up by `find`.
    pub fn side(&self, side: usize, find: impl Fn(&str) -> Option<usize>) -> Result<(usize, [Vec<usize>; 5]), String> {
        let wrist = find(self.wrists[side]).ok_or_else(|| format!("no bone {}", self.wrists[side]))?;
        let mut fingers: [Vec<usize>; 5] = Default::default();
        for (finger, bones) in fingers.iter_mut().enumerate() {
            for n in 1..=3 {
                let name = self.name(side, finger, n);
                bones.push(find(&name).ok_or_else(|| format!("no bone {name}"))?);
            }
        }
        Ok((wrist, fingers))
    }
}

impl Chains {
    /// The chains of a skeleton whose bones `find` looks up by name: three bones a finger.
    pub fn named(names: &FingerNames, find: impl Fn(&str) -> Option<usize>) -> Result<Chains, String> {
        let mut chains = Chains::default();
        for side in 0..2 {
            (chains.wrists[side], chains.fingers[side]) = names.side(side, &find)?;
        }
        Ok(chains)
    }

    /// The chains of a skeleton that names no bone, from its shape: the chains hanging from each
    /// wrist besides its `forearms` child ([`finger_chains`]), the thumb the shortest, the others
    /// by how far their knuckles sit from the thumb's (index to little) in the reference pose
    /// (`model`, each bone in the skeleton's space). A finger of more than three bones starts
    /// with a kept metacarpal; a thumb bends its last two of three.
    pub fn by_shape(parents: &[i32], model: &[Mat34], wrists: [usize; 2], forearms: [usize; 2]) -> Result<Chains, String> {
        let mut chains = Chains { wrists, ..Chains::default() };
        for side in 0..2 {
            let found: Vec<Vec<usize>> = finger_chains(parents, wrists[side], forearms[side]).into_iter().map(|(_, chain)| chain).collect();
            if found.len() != 5 {
                return Err(format!("{} fingers on wrist {}", found.len(), wrists[side]));
            }
            let thumb = (0..5).min_by_key(|&i| found[i].len()).unwrap_or(0);
            // A finger's knuckle (its second joint): the first ones (metacarpals) can all start at
            // the wrist.
            let knuckle = |i: usize| position(&model[*found[i].get(1).unwrap_or(&found[i][0])]);
            let from_thumb = |i: usize| distance(knuckle(i), knuckle(thumb));
            let mut others: Vec<usize> = (0..5).filter(|&i| i != thumb).collect();
            others.sort_by(|&a, &b| from_thumb(a).total_cmp(&from_thumb(b)));
            for (finger, i) in std::iter::once(thumb).chain(others).enumerate() {
                let chain = &found[i];
                let curled = match (finger, chain.len()) {
                    (_, 0 | 1) => return Err(format!("finger {finger} on wrist {} has {} bones", wrists[side], chain.len())),
                    (0, n) => &chain[..n.min(3)],
                    (_, 2 | 3) => &chain[..],
                    _ => &chain[1..4],
                };
                chains.fingers[side][finger] = curled.to_vec();
            }
        }
        Ok(chains)
    }
}

/// The fingers of a skeleton, ready to take tracked bends.
pub struct Rig {
    rest: Vec<Mat34>,
    parents: Vec<i32>,
    /// Per side: (finger, which tracked bend, a one-knuckle hand with that knuckle's own axes).
    hands: [Vec<(usize, usize, Hand)>; 2],
    /// Per side, the bones the fingers are rebuilt from and on (each knuckle and the bone it hangs
    /// from): the frames that must be rigid. Bones elsewhere (a scaled prop joint) do not matter.
    used: [Vec<usize>; 2],
}

fn direction(m: &Mat34, v: Vec3) -> Vec3 {
    std::array::from_fn(|r| (0..3).map(|c| m[r * 4 + c] * v[c]).sum())
}

/// Anatomical palm axes: back from the middle knuckle toward the wrist, up toward the index (thumb)
/// side of the knuckles, right their cross product; at the wrist. Finger flexion does not set
/// these axes.
pub fn palm_frame(wrist: Mat34, middle: Mat34, index: Mat34, pinky: Mat34) -> Option<Mat34> {
    let normalize = |v: Vec3| {
        let n = dot(v, v).sqrt();
        (n.is_finite() && n > 0.001).then(|| v.map(|x| x / n))
    };
    let at = position(&wrist);
    let back = normalize(sub(at, position(&middle)))?;
    let across = sub(position(&index), position(&pinky));
    let up = normalize(std::array::from_fn(|i| across[i] - back[i] * dot(across, back)))?;
    let right = cross(up, back);
    let frame = [right[0], up[0], back[0], at[0], right[1], up[1], back[1], at[1], right[2], up[2], back[2], at[2]];
    camera::is_rigid(&frame, 0.002).then_some(frame)
}

/// A knuckle's curl axis (world). A finger turns about the line across the knuckles (the palm's
/// up axis), the way that takes an open finger toward the palm (`inward`, the palm's normal),
/// made square to the bone (`along`): an open finger gets the axis of the bone times the normal,
/// and one curled toward the palm in the reference pose gets the same. The thumb turns across the
/// palm, away from its own edge.
fn curl_axis(palm: &Mat34, inward: Vec3, along: Vec3, thumb: bool) -> Option<Vec3> {
    let up = [palm[1], palm[5], palm[9]];
    if thumb {
        return unit(cross(along, up.map(|v| -v)));
    }
    let ahead = [-palm[2], -palm[6], -palm[10]];
    let sign = if dot(cross(ahead, inward), up) >= 0.0 { 1.0 } else { -1.0 };
    let axis = up.map(|v| v * sign);
    unit(std::array::from_fn(|i| axis[i] - along[i] * dot(axis, along)))
}

impl Rig {
    /// The rig of a skeleton: `parents` (-1 for a root, else a lower index), `rest` each bone's
    /// reference pose against its parent, and the hand and finger `chains`. Why not, when the
    /// skeleton does not hold the chains as given or a hand's palm cannot be told.
    pub fn discover(parents: &[i32], rest: Vec<Mat34>, chains: &Chains) -> Result<Self, String> {
        if rest.len() != parents.len() || rest.is_empty() {
            return Err(format!("{} reference poses for {} bones", rest.len(), parents.len()));
        }
        let mut world: Vec<Mat34> = Vec::with_capacity(rest.len());
        for (i, &parent) in parents.iter().enumerate() {
            if parent < -1 || parent >= i as i32 {
                return Err(format!("bone {i}'s parent {parent} does not come before it"));
            }
            world.push(if parent < 0 { rest[i] } else { camera::compose(&world[parent as usize], &rest[i]) });
        }
        let mut hands: [Vec<(usize, usize, Hand)>; 2] = std::array::from_fn(|_| Vec::new());
        let mut used: [Vec<usize>; 2] = std::array::from_fn(|_| Vec::new());
        for (side, hand) in hands.iter_mut().enumerate() {
            let chains_of = &chains.fingers[side];
            if chains_of.iter().all(Vec::is_empty) {
                continue;
            }
            let first = |finger: usize| chains_of[finger].first().and_then(|&b| world.get(b)).copied();
            let wrist = *world.get(chains.wrists[side]).ok_or_else(|| format!("no wrist bone {}", chains.wrists[side]))?;
            let (Some(middle), Some(index), Some(pinky)) = (first(2), first(1), first(4)) else {
                return Err(format!("side {side} lacks a middle, index or little finger"));
            };
            let palm = palm_frame(wrist, middle, index, pinky).ok_or_else(|| format!("side {side}'s palm cannot be told"))?;
            // The palm's normal: its right axis on the left hand, mirrored on the right.
            let sign = if side == 0 { 1.0 } else { -1.0 };
            let inward = [palm[0] * sign, palm[4] * sign, palm[8] * sign];
            for (finger, chain) in chains_of.iter().enumerate() {
                if chain.is_empty() {
                    continue;
                }
                if !(2..=3).contains(&chain.len()) || chain.iter().any(|&b| b >= parents.len()) || chain.windows(2).any(|w| parents[w[1]] != w[0] as i32) {
                    return Err(format!("side {side} finger {finger}: {chain:?} is not a chain of two or three bones"));
                }
                let base = usize::try_from(parents[chain[0]]).map_err(|_| format!("bone {} hangs from nothing", chain[0]))?;
                for &bone in std::iter::once(&base).chain(chain) {
                    if !camera::is_rigid(&world[bone], 0.002) {
                        return Err(format!("bone {bone}'s reference pose is not rigid"));
                    }
                    used[side].push(bone);
                }
                // The tracked thumb bends cover its last two joints (a three-bone thumb's first
                // keeps its rest pose); a finger's cover its joints from the hand out.
                let start = if finger == 0 { chain.len() - 2 } else { 0 };
                for k in start..chain.len() {
                    let bone = chain[k];
                    let child = chain[(k + 1).min(chain.len() - 1)];
                    let along = unit(position(&rest[child])).ok_or_else(|| format!("bone {child} sits on bone {bone}"))?;
                    let along_world = direction(&world[bone], along);
                    let curl_world = curl_axis(&palm, inward, along_world, finger == 0).ok_or_else(|| format!("bone {bone} runs along its curl axis"))?;
                    let curl = direction(&camera::rigid_inverse(&world[bone]), curl_world);
                    let mut fingers: [Option<Finger>; 5] = Default::default();
                    fingers[finger] = Some(Finger { base: parents[bone], knuckles: vec![bone as i32], followers: Vec::new() });
                    hand.push((finger, k - start, Hand { fingers, along, curl }));
                }
            }
        }
        Ok(Self { rest, parents: parents.to_vec(), hands, used })
    }

    /// Puts one hand's tracked fingers on `frames` (every bone, in the rig's space): each knuckle
    /// and everything below it moves, the hand does not. False (and `frames` untouched) for an
    /// invalid pose or frames that are not the rig's.
    pub fn apply(&self, frames: &mut [Mat34], side: usize, pose: FingerPose) -> bool {
        let Some(hand) = self.hands.get(side) else { return false };
        if pose.valid == 0 || frames.len() != self.parents.len() || !pose.fingers.iter().flatten().all(|v| v.is_finite()) || !self.used[side].iter().all(|&b| camera::is_rigid(&frames[b], 0.002)) {
            return false;
        }
        let mut joints = Joints { frames, parents: &self.parents, rest: &self.rest };
        for (finger, bend, mapping) in hand {
            let mut sample = pose;
            sample.fingers[*finger][0] = pose.fingers[*finger][*bend];
            fingers::apply(&mut joints, mapping, &sample);
        }
        true
    }
}

struct Joints<'a> {
    frames: &'a mut [Mat34],
    parents: &'a [i32],
    rest: &'a [Mat34],
}

impl FingerJoints for Joints<'_> {
    fn world(&self, joint: i32) -> Option<Mat34> {
        self.frames.get(usize::try_from(joint).ok()?).copied()
    }

    fn rest_between(&self, from: i32, to: i32) -> Option<Mat34> {
        let to = usize::try_from(to).ok()?;
        (self.parents.get(to) == Some(&from)).then(|| self.rest[to])
    }

    /// Moves the bone and carries every bone below it along.
    fn set_world(&mut self, joint: i32, matrix: &Mat34) {
        let bone = joint as usize;
        let delta = camera::compose(matrix, &camera::rigid_inverse(&self.frames[bone]));
        self.frames[bone] = *matrix;
        for child in bone + 1..self.frames.len() {
            let mut parent = self.parents[child];
            while parent > bone as i32 {
                parent = self.parents[parent as usize];
            }
            if parent == bone as i32 {
                self.frames[child] = camera::compose(&delta, &self.frames[child]);
            }
        }
    }
}

// ---- On a game's skeleton ----

/// Where a skeleton's finger bones are, as a game says once.
#[derive(Clone, Copy, Debug)]
pub enum ChainSource {
    /// By name ([`FingerNames`]).
    Named(FingerNames),
    /// By index ([`ChainTable`]).
    Table(&'static ChainTable),
    /// By the skeleton's shape ([`Chains::by_shape`]), under the wrists the hand rig found
    /// ([`Frames::wrists`]): for a skeleton that names nothing.
    Shape,
}

/// A skeleton's hands and finger bones by element, left then right: each finger's curled bones
/// from the hand out ([`Chains::fingers`]); a side with no fingers listed has none.
#[derive(Clone, Copy, Debug)]
pub struct ChainTable {
    pub wrists: [i32; 2],
    pub fingers: [[&'static [i32]; 5]; 2],
}

/// A wrist and its fingers' curled bones, by element.
type SideChains = (usize, [Vec<usize>; 5]);

impl ChainSource {
    /// Side `side`'s wrist and its fingers' curled bones (elements), or why not. `None`: not yet
    /// (the shape's wrists are not found this update).
    fn side(&self, skeleton: &dyn Skeleton, frames: &Frames, parents: &[i32], side: usize) -> Option<Result<SideChains, String>> {
        Some(match self {
            ChainSource::Named(names) => {
                let all: Vec<String> = (0..skeleton.count()).map(|e| skeleton.name(e)).collect();
                names.side(side, |wanted| all.iter().position(|n| n == wanted))
            }
            ChainSource::Table(table) => table.side(side),
            ChainSource::Shape => {
                let (wrist, forearm) = frames.wrists[side]?;
                Chains::by_shape(parents, frames.animated, [wrist; 2], [forearm; 2]).map(|chains| (wrist, chains.fingers[side].clone()))
            }
        })
    }
}

impl ChainTable {
    fn side(&self, side: usize) -> Result<SideChains, String> {
        let index = |e: i32| usize::try_from(e).map_err(|_| format!("element {e} in the finger table"));
        let mut fingers: [Vec<usize>; 5] = Default::default();
        for (finger, bones) in fingers.iter_mut().enumerate() {
            *bones = self.fingers[side][finger].iter().map(|&b| index(b)).collect::<Result<_, _>>()?;
        }
        Ok((index(self.wrists[side])?, fingers))
    }
}

/// `root` and everything under it, parents first (whatever order the skeleton keeps them in).
fn under(parents: &[i32], root: usize) -> Vec<usize> {
    let mut out = vec![root];
    let mut k = 0;
    while k < out.len() && out.len() <= parents.len() {
        let parent = out[k];
        out.extend((0..parents.len()).filter(|&e| e != parent && parents[e] == parent as i32));
        k += 1;
    }
    out
}

/// One hand as a skeleton of its own: its elements (the wrist first, parents before children) and
/// its finger rig.
struct HandBones {
    elements: Vec<usize>,
    rig: Rig,
}

/// Hand `side` of `skeleton`, its fingers as `source` says. `None`: not yet.
fn hand_bones(skeleton: &dyn Skeleton, frames: &Frames, source: &ChainSource, side: usize) -> Option<Result<HandBones, String>> {
    let parents: Vec<i32> = (0..skeleton.count()).map(|e| skeleton.parent(e)).collect();
    Some(source.side(skeleton, frames, &parents, side)?.and_then(|(wrist, fingers)| hand_from(skeleton, frames, &parents, side, wrist, &fingers)))
}

fn hand_from(skeleton: &dyn Skeleton, frames: &Frames, parents: &[i32], side: usize, wrist: usize, fingers: &[Vec<usize>; 5]) -> Result<HandBones, String> {
    if fingers.iter().all(Vec::is_empty) {
        return Err("no fingers listed".to_owned());
    }
    if wrist >= frames.animated.len() {
        return Err(format!("no wrist element {wrist}"));
    }
    let elements = under(parents, wrist);
    let local = |e: usize| elements.iter().position(|&x| x == e);
    let local_parents: Vec<i32> = elements
        .iter()
        .enumerate()
        .map(|(i, &e)| if i == 0 { -1 } else { usize::try_from(parents[e]).ok().and_then(local).map_or(-1, |p| p as i32) })
        .collect();
    let asked: Vec<i32> = elements[1..].iter().map(|&e| e as i32).collect();
    let kept = skeleton.rests_under(wrist as i32, &asked);
    let mut seen = 0;
    let mut rest = vec![IDENTITY_MATRIX];
    for (i, &e) in elements.iter().enumerate().skip(1) {
        let matrix = match kept.get(i - 1).copied().flatten() {
            Some(matrix) => matrix,
            None => {
                seen += 1;
                let parent = elements[local_parents[i] as usize];
                let back = camera::inverse(&frames.animated[parent]).ok_or_else(|| format!("element {parent} as first seen has no inverse"))?;
                camera::compose(&back, &frames.animated[e])
            }
        };
        if !matrix.iter().all(|v| v.is_finite()) {
            return Err(format!("element {e} has no rest pose"));
        }
        rest.push(matrix);
    }
    let mut chains = Chains::default();
    chains.fingers[side] = fingers.each_ref().map(|bones| bones.iter().filter_map(|&b| local(b)).collect());
    if chains.fingers[side].iter().zip(fingers).any(|(found, listed)| found.len() != listed.len()) {
        return Err(format!("a finger bone is not under the wrist {wrist}"));
    }
    let rig = Rig::discover(&local_parents, rest, &chains)?;
    log!(
        "fingers: the {} hand's fingers found on wrist {wrist} ({} elements; rest poses: {} the game's, {seen} as first seen)",
        side_name(side),
        elements.len(),
        elements.len() - 1 - seen
    );
    Ok(HandBones { elements, rig })
}

fn side_name(side: usize) -> &'static str {
    if side == LEFT_HAND { "left" } else { "right" }
}

/// A skeleton's hand found (or found missing), by skeleton, element count and side.
struct Known {
    skeleton: usize,
    count: i32,
    side: usize,
    hand: Option<Arc<HandBones>>,
}

static KNOWN: Mutex<Vec<Known>> = Mutex::new(Vec::new());
/// Updates each hand's fingers were put on tracking.
static PLACED: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];

/// Hand `side` of `skeleton`, found once.
fn known(skeleton: &dyn Skeleton, frames: &Frames, source: &ChainSource, side: usize) -> Option<Arc<HandBones>> {
    let (id, count) = (skeleton.id(), skeleton.count());
    if let Some(k) = KNOWN.locked().iter().find(|k| k.skeleton == id && k.count == count && k.side == side) {
        return k.hand.clone();
    }
    let hand = match hand_bones(skeleton, frames, source, side)? {
        Ok(hand) => Some(Arc::new(hand)),
        Err(why) => {
            log!("fingers: the {} hand stays as the game animates it: {why}", side_name(side));
            None
        }
    };
    let mut known = KNOWN.locked();
    if known.len() >= 16 {
        known.clear();
    }
    known.push(Known { skeleton: id, count, side, hand: hand.clone() });
    hand
}

/// Puts hand `side`'s fingers on `tracked` in `frames` (the rig's frames of `skeleton`, the hand
/// where the rig put it): each knuckle and what hangs from it moves, the hand does not. False (and
/// `frames` untouched) when the hand has no fingers found or the pose is not usable.
pub fn place(skeleton: &dyn Skeleton, frames: &mut Frames, source: &ChainSource, side: usize, tracked: &FingerPose) -> bool {
    let Some(hand) = known(skeleton, frames, source, side) else { return false };
    if hand.elements.iter().any(|&e| e >= frames.now.len()) {
        return false;
    }
    let world = frames.now[hand.elements[0]];
    let Some(back) = camera::inverse(&world) else { return false };
    let mut local: Vec<Mat34> = hand.elements.iter().map(|&e| camera::compose(&back, &frames.now[e])).collect();
    local[0] = IDENTITY_MATRIX;
    let before = local.clone();
    if !hand.rig.apply(&mut local, side, *tracked) {
        return false;
    }
    for (i, &e) in hand.elements.iter().enumerate().skip(1) {
        if local[i] != before[i] {
            frames.now[e] = camera::compose(&world, &local[i]);
        }
    }
    if PLACED[side].fetch_add(1, Relaxed) == 0 {
        log!("fingers: the {} hand's fingers follow finger tracking", side_name(side));
    }
    true
}

/// How often each hand's fingers were put on tracking.
pub fn summary() -> String {
    format!("fingers: placed from tracking {} times (left), {} (right)", PLACED[LEFT_HAND].load(Relaxed), PLACED[RIGHT_HAND].load(Relaxed))
}

/// End-of-run line, when fingers were placed at all.
pub fn report() {
    if PLACED.iter().any(|p| p.load(Relaxed) > 0) {
        log!("{}", summary());
    }
}

/// A game with tracked fingers and no hand rig: where its fingers are.
pub trait FingerRig: Game {
    const CHAINS: ChainSource;
    /// The kind of what hand `side` holds now: a hand that grips it keeps the game's grip
    /// ([`Kind::grips`]).
    fn held(_side: usize) -> Kind {
        Kind::Nothing
    }
}

/// A finger update of game `G` on `skeleton` (no hand rig): the skeleton read, each free hand's
/// tracked fingers put on, the bones they changed written. False when nothing was written.
pub fn frame<G: FingerRig>(skeleton: &dyn Skeleton) -> bool {
    let Some(animated) = Snapshot::take(skeleton) else { return false };
    let mut frames = Frames::new(&animated);
    let mut placed = false;
    for side in [LEFT_HAND, RIGHT_HAND] {
        if !G::held(side).grips()
            && let Some(tracked) = crate::controllers::fingers(side)
        {
            placed |= place(skeleton, &mut frames, &G::CHAINS, side, &tracked);
        }
    }
    placed && !frames.flush(skeleton).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each bone's pose against its parent, from its pose in the skeleton's space (`model`; a
    /// root's stays as given).
    fn rests_against_parents(parents: &[i32], model: &[Mat34]) -> Vec<Mat34> {
        (0..model.len())
            .map(|i| match usize::try_from(parents[i]).ok().and_then(|p| model.get(p)) {
                Some(parent) => camera::compose(&camera::rigid_inverse(parent), &model[i]),
                None => model[i],
            })
            .collect()
    }

    /// Two hands of five fingers, `joints` curled bones each (an optional metacarpal before them),
    /// with a relaxed rest curl, a watch on the left wrist and a nub on each index tip.
    struct Fixture {
        parents: Vec<i32>,
        rest: Vec<Mat34>,
        world: Vec<Mat34>,
        chains: Chains,
        tip: [usize; 2],
        watch: usize,
    }

    fn push(f: &mut Fixture, parent: usize, rest: Mat34) -> usize {
        f.parents.push(parent as i32);
        f.rest.push(rest);
        f.parents.len() - 1
    }

    fn fixture(joints: usize, metacarpals: bool) -> Fixture {
        curled_fixture(joints, metacarpals, -20.0)
    }

    /// As [`fixture`], each knuckle curled `degrees` in the reference pose.
    fn curled_fixture(joints: usize, metacarpals: bool, degrees: f32) -> Fixture {
        let mut f = Fixture { parents: vec![-1], rest: vec![camera::translation([0.0; 3])], world: Vec::new(), chains: Chains::default(), tip: [0; 2], watch: 0 };
        for side in 0..2 {
            let wrist = push(&mut f, 0, camera::translation([if side == 0 { -0.2 } else { 0.2 }, 0.0, 0.0]));
            f.chains.wrists[side] = wrist;
            for finger in 0..5 {
                let x = [0.06, 0.04, 0.0, -0.02, -0.04][finger] * if side == 0 { 1.0 } else { -1.0 };
                let mut parent = wrist;
                if metacarpals {
                    parent = push(&mut f, wrist, camera::translation([x * 0.2, 0.0, -0.005]));
                }
                for k in 0..joints {
                    let offset = if k == 0 { [if metacarpals { x * 0.8 } else { x }, 0.0, -0.08] } else { [0.0, 0.0, -0.04] };
                    // A relaxed reference curl; a tracked zero takes it out.
                    let a = degrees.to_radians();
                    let rest = camera::apply_head(&camera::translation(offset), [(a / 2.0).sin(), 0.0, 0.0, (a / 2.0).cos()], [0.0; 3]);
                    let bone = push(&mut f, parent, rest);
                    f.chains.fingers[side][finger].push(bone);
                    parent = bone;
                }
                if finger == 1 {
                    f.tip[side] = parent;
                    push(&mut f, parent, camera::translation([0.0, 0.0, -0.02]));
                }
            }
        }
        let left_wrist = f.chains.wrists[0];
        f.watch = push(&mut f, left_wrist, camera::translation([0.0, 0.02, 0.0]));
        for i in 0..f.rest.len() {
            let world = if f.parents[i] < 0 { f.rest[i] } else { camera::compose(&f.world[f.parents[i] as usize], &f.rest[i]) };
            f.world.push(world);
        }
        f
    }

    fn open() -> FingerPose {
        FingerPose { valid: 1, ..FingerPose::default() }
    }

    #[test]
    fn mirrored_fingers_curl_toward_palms_without_moving_wrists_watch_or_other_hand() {
        for (joints, metacarpals) in [(3, false), (3, true), (2, false)] {
            let f = fixture(joints, metacarpals);
            let rig = Rig::discover(&f.parents, f.rest.clone(), &f.chains).unwrap();
            for side in 0..2 {
                let mut opened = f.world.clone();
                assert!(rig.apply(&mut opened, side, open()));
                let mut closed = f.world.clone();
                assert!(rig.apply(&mut closed, side, FingerPose { fingers: [[0.7, 0.8, 0.6, 0.0]; 5], ..open() }));
                let tip = f.tip[side];
                assert!(position(&closed[tip])[1] < position(&opened[tip])[1] - 0.02, "{joints} joints, metacarpals {metacarpals}");
                // The nub rides its knuckle.
                let (nub, knuckle) = (tip + 1, tip);
                let riding = camera::compose(&camera::rigid_inverse(&closed[knuckle]), &closed[nub]);
                assert!(camera::distance(&riding, &f.rest[nub]) < 1e-4);
                let wrist = f.chains.wrists[side];
                assert_eq!(opened[wrist], f.world[wrist]);
                assert_eq!(closed[f.watch], f.world[f.watch]);
                let other = f.chains.wrists[1 - side];
                assert_eq!(closed[other], f.world[other]);
                // Segments keep their reference lengths.
                let chain = &f.chains.fingers[side][1];
                for w in chain.windows(2) {
                    let (a, b) = (position(&closed[w[1]]), position(&closed[w[0]]));
                    let length = (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f32>().sqrt();
                    assert!((length - 0.04).abs() < 1e-5);
                }
            }
        }
    }

    #[test]
    fn invalid_tracking_and_wrong_hierarchies_preserve_animation() {
        let mut f = fixture(3, false);
        let rig = Rig::discover(&f.parents, f.rest.clone(), &f.chains).unwrap();
        let mut frames = f.world.clone();
        assert!(!rig.apply(&mut frames, 0, FingerPose::default()));
        let mut bad = open();
        bad.fingers[1][1] = f32::NAN;
        assert!(!rig.apply(&mut frames, 0, bad));
        assert_eq!(frames, f.world);
        let bone = f.chains.fingers[0][1][1];
        f.parents[bone] = 0;
        assert!(Rig::discover(&f.parents, f.rest.clone(), &f.chains).is_err());
        f.parents[bone] = bone as i32;
        assert!(Rig::discover(&f.parents, f.rest.clone(), &f.chains).is_err());
        let mut short = fixture(3, false);
        short.chains.fingers[0][2].truncate(1);
        assert!(Rig::discover(&short.parents, short.rest.clone(), &short.chains).is_err());
        // A hand without a thumb or ring finger still has a palm; one without a middle finger not.
        let mut fewer = fixture(3, false);
        fewer.chains.fingers[0][0].clear();
        fewer.chains.fingers[1][3].clear();
        assert!(Rig::discover(&fewer.parents, fewer.rest.clone(), &fewer.chains).is_ok());
        fewer.chains.fingers[0][2].clear();
        assert!(Rig::discover(&fewer.parents, fewer.rest.clone(), &fewer.chains).is_err());
    }

    #[test]
    fn individual_fingers_and_two_thumb_bends_are_independent_and_bounded() {
        for joints in [3, 2] {
            let f = fixture(joints, false);
            let rig = Rig::discover(&f.parents, f.rest.clone(), &f.chains).unwrap();
            let mut opened = f.world.clone();
            assert!(rig.apply(&mut opened, 0, open()));
            let mut pose = open();
            pose.fingers[1] = [0.8, 0.4, 0.3, 0.0];
            let mut index = f.world.clone();
            assert!(rig.apply(&mut index, 0, pose));
            for finger in [0, 2, 3, 4] {
                for &bone in &f.chains.fingers[0][finger] {
                    assert_eq!(index[bone], opened[bone]);
                }
            }
            let thumb = &f.chains.fingers[0][0];
            let mut pose = open();
            pose.fingers[0] = [0.4, 0.9, 0.0, 0.0];
            let mut tracked = f.world.clone();
            assert!(rig.apply(&mut tracked, 0, pose));
            // A three-bone thumb keeps its first bone; its last two (and a two-bone thumb's both)
            // take the bends.
            if joints == 3 {
                assert_eq!(tracked[thumb[0]], opened[thumb[0]]);
            }
            assert_ne!(tracked[thumb[joints - 2]], opened[thumb[joints - 2]]);
            assert_ne!(tracked[thumb[joints - 1]], opened[thumb[joints - 1]]);
            let mut huge = open();
            huge.fingers[1] = [100.0; 4];
            let mut bounded = open();
            bounded.fingers[1] = [1.9; 4];
            let (mut a, mut b) = (f.world.clone(), f.world.clone());
            assert!(rig.apply(&mut a, 0, huge));
            assert!(rig.apply(&mut b, 0, bounded));
            assert_eq!(a, b);
        }
    }

    #[test]
    fn applying_twice_from_the_same_hand_gives_the_same_fingers() {
        let f = fixture(3, true);
        let rig = Rig::discover(&f.parents, f.rest.clone(), &f.chains).unwrap();
        let pose = FingerPose { fingers: [[0.5, 0.6, 0.4, 0.0]; 5], ..open() };
        let mut once = f.world.clone();
        assert!(rig.apply(&mut once, 1, pose));
        let mut twice = once.clone();
        assert!(rig.apply(&mut twice, 1, pose));
        for (a, b) in once.iter().zip(&twice) {
            assert!(camera::distance(a, b) < 1e-5);
        }
    }

    #[test]
    fn a_curled_reference_pose_gives_an_open_ones_axes() {
        let f = fixture(3, true);
        let open = Rig::discover(&f.parents, f.rest.clone(), &f.chains).unwrap();
        // Each knuckle 80 degrees in: the fingers point into the palm, where the bone times the
        // palm's normal says nothing.
        let f = curled_fixture(3, true, -80.0);
        let curled = Rig::discover(&f.parents, f.rest.clone(), &f.chains).unwrap();
        for side in 0..2 {
            for ((_, _, a), (_, _, b)) in open.hands[side].iter().zip(&curled.hands[side]) {
                assert!(distance(a.curl, b.curl) < 1e-4, "side {side}: {:?} against {:?}", a.curl, b.curl);
            }
        }
    }

    #[test]
    fn chains_by_name() {
        let f = fixture(3, false);
        const NAMES: FingerNames = FingerNames { wrists: ["l_hand", "r_hand"], sides: ["Left", "Right"], fingers: ["Thumb", "Index", "Middle", "Ring", "Pinky"], bone: "{side}Hand{finger}{n}" };
        let name = |bone: usize| -> Option<String> {
            for side in 0..2 {
                if f.chains.wrists[side] == bone {
                    return Some(NAMES.wrists[side].to_owned());
                }
                for finger in 0..5 {
                    if let Some(n) = f.chains.fingers[side][finger].iter().position(|&b| b == bone) {
                        return Some(NAMES.name(side, finger, n + 1));
                    }
                }
            }
            None
        };
        let named: Vec<Option<String>> = (0..f.parents.len()).map(name).collect();
        let chains = Chains::named(&NAMES, |wanted| named.iter().position(|n| n.as_deref() == Some(wanted))).unwrap();
        assert_eq!(chains, f.chains);
        assert_eq!(NAMES.name(1, 3, 2), "RightHandRing2");
        assert!(Chains::named(&NAMES, |_| None).unwrap_err().contains("l_hand"));
    }

    #[test]
    fn chains_by_shape() {
        // A right hand: wrist 0 (forearm 1), a three-bone thumb (2-4) on the index side (+y),
        // four-bone fingers index 5-8, middle 9-12, ring 13-16, little 17-20.
        let mut parents = vec![-1, 0];
        let mut model = vec![camera::translation([0.0; 3]), camera::translation([-5.0, 0.0, 0.0])];
        for (len, y) in [(3, 3.0), (4, 1.5), (4, 0.5), (4, -0.5), (4, -1.5)] {
            for k in 0..len {
                parents.push(if k == 0 { 0 } else { parents.len() as i32 - 1 });
                model.push(camera::translation([2.0 + 2.0 * k as f32, y, 0.0]));
            }
        }
        let chains = Chains::by_shape(&parents, &model, [0, 0], [1, 1]).unwrap();
        assert_eq!(chains.fingers[1], [vec![2, 3, 4], vec![6, 7, 8], vec![10, 11, 12], vec![14, 15, 16], vec![18, 19, 20]]);
        // The forearm taken for a finger: six.
        assert!(Chains::by_shape(&parents, &model, [0, 0], [usize::MAX; 2]).unwrap_err().contains("6 fingers"));
    }

    #[test]
    fn one_hand_and_model_space_rests() {
        let f = fixture(3, true);
        let rest = rests_against_parents(&f.parents, &f.world);
        assert!(rest.iter().zip(&f.rest).all(|(a, b)| camera::distance(a, b) < 1e-5));
        let mut chains = f.chains.clone();
        chains.fingers[1] = Default::default();
        let rig = Rig::discover(&f.parents, rest, &chains).unwrap();
        let mut frames = f.world.clone();
        assert!(rig.apply(&mut frames, 1, FingerPose { fingers: [[0.7; 4]; 5], ..open() }));
        assert_eq!(frames, f.world, "no right hand: nothing moves");
        assert!(rig.apply(&mut frames, 0, FingerPose { fingers: [[0.7; 4]; 5], ..open() }));
        assert_ne!(frames, f.world);
        // A scaled bone the hands do not use (the watch) is no reason to refuse.
        let mut scaled = f.rest.clone();
        scaled[f.watch][0] = 2.0;
        let rig = Rig::discover(&f.parents, scaled, &f.chains).unwrap();
        let mut frames = f.world.clone();
        frames[f.watch][0] = 2.0;
        assert!(rig.apply(&mut frames, 0, FingerPose { fingers: [[0.7; 4]; 5], ..open() }));
    }
}
