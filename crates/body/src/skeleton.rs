//! A game's skeleton, the hand rig's provider ([`Skeleton`]), one moment of it ([`Snapshot`]) and
//! its frames through one update ([`Frames`]); and finding the arms and fingers on a skeleton
//! whose names differ from what [`crate::follow`] expects (`l_upperarm`, `r_hand`, ...) or that has
//! none: a rename table onto the solver's names ([`Renames`]), or the skeleton's shape alone
//! ([`find_arm`], [`finger_chains`], [`attach_joint`]).
//!
//! A skeleton's shape is its parent table: one entry per joint, the parent's index or -1 for a
//! root, parents before their children.

use monaka_core::camera::Mat34;
use std::sync::Mutex;

#[cfg(test)]
mod tests;

/// A game's skeleton, the hand rig's provider: its elements (bones, helpers and, in some games,
/// mesh parts) by index, each with a world matrix (row-major 3x4, camera-to-world convention:
/// columns right, up, back, position). Every matrix is in the space of the moment it is read at:
/// one right-handed space, Y up, in metres, shared with what the hands are put on
/// ([`hand_rig::Moment`]); an impl converts the engine's own form both ways. A skeleton the hand
/// rig places by its own targets ([`hand_rig::Method::HandRooted`]) may keep the space and units
/// its targets are given in.
pub trait Skeleton {
    /// Identifies this skeleton (its address): a different one is looked up again.
    fn id(&self) -> usize;
    fn count(&self) -> i32;
    /// "" where the engine names no elements.
    fn name(&self, element: i32) -> String;
    /// The parent element, or a negative number for a root; parents come before their children.
    fn parent(&self, element: i32) -> i32;
    /// The element's rest pose (the skeleton's bind or reference pose) against its parent, where
    /// the engine keeps one. None: the pose first seen stands for it.
    fn rest(&self, _element: i32) -> Option<Mat34> {
        None
    }
    /// The rest poses of `elements`, all under `root`, at once (an engine that reads them by
    /// resetting `root`'s subtree to its reference frame does that once).
    fn rests_under(&self, _root: i32, elements: &[i32]) -> Vec<Option<Mat34>> {
        elements.iter().map(|&e| self.rest(e)).collect()
    }
    fn world(&self, element: i32) -> Option<Mat34>;
    /// The rig's pose for one element: set in place, or kept for the game's own write-back after
    /// the rig's call returns (a queue, a buffer swapped in at an upload).
    fn set_world(&self, element: i32, matrix: &Mat34);
    /// Whether the element is a bone (helpers and mesh parts follow their bones and are never
    /// written: moving them leaves the arms where the game put them).
    fn is_bone(&self, element: i32) -> bool;
}

/// Every element's world matrix of a skeleton at one moment. An element the skeleton cannot give
/// as finite numbers is kept as NaN and never written: Dying Light 2 can leave an outfit's glove
/// bone so for good after a cutscene, and refusing the whole skeleton for it would leave the arms
/// to the game.
pub struct Snapshot {
    pub skeleton: usize,
    pub worlds: Vec<Mat34>,
}

static SPARE: Mutex<Vec<Mat34>> = Mutex::new(Vec::new());

impl Snapshot {
    pub fn take(skeleton: &dyn Skeleton) -> Option<Self> {
        let mut worlds = SPARE.lock().ok().map(|mut spare| std::mem::take(&mut *spare)).unwrap_or_default();
        worlds.clear();
        for element in 0..skeleton.count() {
            worlds.push(skeleton.world(element).filter(|w| w.iter().all(|v| v.is_finite())).unwrap_or([f32::NAN; 12]));
        }
        worlds.iter().any(|w| w[0].is_finite()).then_some(Self { skeleton: skeleton.id(), worlds })
    }

    /// The pose of skeleton `skeleton` as the game read it elsewhere (a queue's stored set, a
    /// worker's pose), not-finite matrices kept as NaN; None when nothing is readable.
    pub fn of(skeleton: usize, mut worlds: Vec<Mat34>) -> Option<Self> {
        for world in &mut worlds {
            if !world.iter().all(|v| v.is_finite()) {
                *world = [f32::NAN; 12];
            }
        }
        worlds.iter().any(|w| w[0].is_finite()).then_some(Self { skeleton, worlds })
    }
}

/// A skeleton's frames through one update: as the game animated them, as the rig has them now, and
/// which the rig placed. Written back in one go ([`Frames::flush`]): every element placed, and
/// every other one the rig changed.
pub struct Frames<'a> {
    pub animated: &'a [Mat34],
    pub now: Vec<Mat34>,
    placed: Vec<bool>,
    /// Per side, the wrist and its child toward the elbow, where the arms were found by the
    /// skeleton's shape (a skeleton hung from the hands): where the fingers are looked for by shape.
    pub wrists: [Option<(usize, usize)>; 2],
}

impl<'a> Frames<'a> {
    pub fn new(animated: &'a Snapshot) -> Self {
        Self { animated: &animated.worlds, now: animated.worlds.clone(), placed: vec![false; animated.worlds.len()], wrists: [None; 2] }
    }

    /// Element `element` placed at `matrix`: written back even if that is where it was.
    pub fn place(&mut self, element: usize, matrix: Mat34) {
        if let (Some(now), Some(placed)) = (self.now.get_mut(element), self.placed.get_mut(element)) {
            *now = matrix;
            *placed = true;
        }
    }

    /// Whether element `element` goes back to the game: placed, or changed from its animation.
    fn written(&self, element: usize) -> bool {
        self.now[element].iter().all(|v| v.is_finite()) && (self.placed[element] || self.now[element] != self.animated[element])
    }

    /// The bones placed or changed, in index order: what [`Frames::flush`] writes.
    pub fn changed(&self, skeleton: &dyn Skeleton) -> Vec<usize> {
        let count = (skeleton.count().max(0) as usize).min(self.now.len());
        (0..count).filter(|&e| self.written(e) && skeleton.is_bone(e as i32)).collect()
    }

    /// Writes the elements placed or changed into `skeleton` (bones only, in index order) and
    /// returns them.
    pub fn flush(&self, skeleton: &dyn Skeleton) -> Vec<usize> {
        let written = self.changed(skeleton);
        for &element in &written {
            skeleton.set_world(element as i32, &self.now[element]);
        }
        written
    }
}

impl Drop for Snapshot {
    fn drop(&mut self) {
        // Keep the allocation for the next call.
        if let Ok(mut spare) = SPARE.lock() {
            *spare = std::mem::take(&mut self.worlds);
        }
    }
}

/// A game's arm joint names onto the solver's: each side's prefix (the game's, the solver's), a
/// suffix the game puts after every name, and the names in between (the game's, the solver's).
#[derive(Clone, Copy, Debug)]
pub struct Renames {
    /// By side (left, right): the game's prefix and the solver's (`l_` or `r_`).
    pub sides: [(&'static str, &'static str); 2],
    /// After every game name (none: "").
    pub suffix: &'static str,
    pub names: &'static [(&'static str, &'static str)],
}

impl Renames {
    /// The solver's name for the game's joint `own` on side `side` (0 left, 1 right), if the table
    /// names it.
    pub fn solver_name(&self, side: usize, own: &str) -> Option<String> {
        let (game, solver) = self.sides[side.min(1)];
        let rest = own.strip_prefix(game)?;
        let bare = rest.strip_suffix(self.suffix).unwrap_or(rest);
        self.names.iter().find(|(k, _)| *k == bare).map(|(_, ours)| format!("{solver}{ours}"))
    }
}

/// Joint `j`'s children.
pub fn children(parents: &[i32], j: usize) -> Vec<usize> {
    (j + 1..parents.len()).filter(|&k| parents[k] == j as i32).collect()
}

/// Joint `j`'s descendants, `j` first.
pub fn subtree(parents: &[i32], j: usize) -> Vec<usize> {
    let mut inside = vec![false; parents.len()];
    inside[j] = true;
    let mut out = vec![j];
    for k in j + 1..parents.len() {
        let p = parents[k];
        if p >= 0 && inside[p as usize] {
            inside[k] = true;
            out.push(k);
        }
    }
    out
}

/// One arm found by shape.
#[derive(Clone, Debug, PartialEq)]
pub struct Arm {
    /// The joint the arm hangs from (a hand's attach joint) and everything under it.
    pub root: usize,
    pub subtree: Vec<usize>,
    pub wrist: usize,
    pub elbow: usize,
    pub shoulder: usize,
    /// The wrist's child toward the shoulder and everything under it (forearm, elbow, shoulder).
    pub forearm: Vec<usize>,
}

/// The arm under joint `root` of a skeleton hung from the hands (the arm runs from the wrist back
/// up to the shoulder): its wrist (the one joint with three or more children, the fingers and the
/// forearm; a joint holding both arms has two), the shoulder (the end of the longest chain under
/// the wrist: the forearm's twists, the elbow and the shoulder outnumber any finger's joints), its
/// elbow (the shoulder's parent). Found from the skeleton alone, so whatever pose the game holds
/// when it is first looked at does not matter.
pub fn find_arm(parents: &[i32], root: usize) -> Option<Arm> {
    let subtree = subtree(parents, root);
    let children = |j: usize| parents.iter().filter(|&&p| p == j as i32).count();
    let mut wrists = subtree.iter().copied().filter(|&j| children(j) >= 3);
    let (Some(wrist), None) = (wrists.next(), wrists.next()) else { return None };
    // Joints below the wrist (the wrist itself 0); parents come first, so one pass fills them.
    let mut below = vec![usize::MAX; parents.len()];
    below[wrist] = 0;
    for &j in &subtree {
        if let Ok(p) = usize::try_from(parents[j])
            && below[p] != usize::MAX
            && j != wrist
        {
            below[j] = below[p] + 1;
        }
    }
    let shoulder = subtree.iter().copied().filter(|&j| below[j] != usize::MAX).max_by_key(|&j| (below[j], usize::MAX - j))?;
    let elbow = usize::try_from(parents[shoulder]).ok()?;
    // The wrist's child on the way to the shoulder.
    let mut at = shoulder;
    while parents[at] != wrist as i32 {
        at = usize::try_from(parents[at]).ok()?;
    }
    let forearm = self::subtree(parents, at);
    (elbow != wrist && forearm.contains(&elbow)).then_some(Arm { root, subtree, wrist, elbow, shoulder, forearm })
}

/// The attach joint for a hand tag: of the joints within half a unit of the tag (`distances`, by
/// joint), the nearest with one arm under it ([`find_arm`]). A root holding both arms can sit on the
/// attach joint in some poses; it is passed over.
pub fn attach_joint(parents: &[i32], distances: &[f32]) -> Option<usize> {
    let mut near: Vec<usize> = (0..distances.len()).filter(|&j| distances[j] < 0.5).collect();
    near.sort_by(|&a, &b| distances[a].total_cmp(&distances[b]));
    near.into_iter().find(|&j| find_arm(parents, j).is_some())
}

/// The fingers under `wrist` (not under `forearm`): from each fingertip (a joint with no children)
/// back up while the joint above has only it, stopping below the wrist or a joint more fingers
/// share (a palm joint). Each chain runs from the hand out, with the joint it hangs from.
pub fn finger_chains(parents: &[i32], wrist: usize, forearm: usize) -> Vec<(usize, Vec<usize>)> {
    let mut inside = vec![false; parents.len()];
    inside[wrist] = true;
    let mut tips = Vec::new();
    for k in wrist + 1..parents.len() {
        let Ok(p) = usize::try_from(parents[k]) else { continue };
        if inside[p] && k != forearm {
            inside[k] = true;
            if children(parents, k).is_empty() {
                tips.push(k);
            }
        }
    }
    tips.into_iter()
        .map(|tip| {
            let mut chain = vec![tip];
            let mut at = tip;
            while let Ok(p) = usize::try_from(parents[at])
                && p != wrist
                && children(parents, p).len() == 1
            {
                chain.push(p);
                at = p;
            }
            chain.reverse();
            (usize::try_from(parents[at]).unwrap_or(wrist), chain)
        })
        .collect()
}

