//! Finding the arms and fingers on a game's skeleton, for games that name them differently from
//! what [`crate::follow`] expects (`l_upperarm`, `r_hand`, ...) or not at all: a rename table onto
//! the solver's names ([`Renames`]), or the skeleton's shape alone ([`find_arm`],
//! [`finger_chains`], [`attach_joint`]) for a skeleton without names.
//!
//! A skeleton's shape is its parent table: one entry per joint, the parent's index or -1 for a
//! root, parents before their children.

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rename_table_maps_a_games_names() {
        const PREFIXED: Renames = Renames { sides: [("Left", "l_"), ("Right", "r_")], suffix: "", names: &[("ForeArm", "forearm"), ("Hand", "hand")] };
        assert_eq!(PREFIXED.solver_name(1, "RightForeArm").as_deref(), Some("r_forearm"));
        assert_eq!(PREFIXED.solver_name(0, "LeftHand").as_deref(), Some("l_hand"));
        assert_eq!(PREFIXED.solver_name(0, "RightHand"), None, "the other side's prefix");
        assert_eq!(PREFIXED.solver_name(0, "LeftHandIndex1"), None, "not in the table");
        const SUFFIXED: Renames = Renames { sides: [("l_", "l_"), ("r_", "r_")], suffix: "_jnt", names: &[("lowerArm", "forearm")] };
        assert_eq!(SUFFIXED.solver_name(0, "l_lowerArm_jnt").as_deref(), Some("l_forearm"));
        assert_eq!(SUFFIXED.solver_name(1, "r_lowerArm").as_deref(), Some("r_forearm"), "the suffix may be missing");
    }

    /// A right arm hung from its attach joint: joint 0 a root, 1 the attach joint, 2 the wrist with
    /// three fingers (3, 4, 5), 6 and 7 forearm twists, 8 the elbow, 9 the shoulder.
    const ARM: [i32; 10] = [-1, 0, 1, 2, 2, 2, 2, 6, 7, 8];

    #[test]
    fn an_arm_is_found_by_its_shape() {
        let arm = find_arm(&ARM, 1).unwrap();
        assert_eq!((arm.wrist, arm.elbow, arm.shoulder), (2, 8, 9));
        assert_eq!(arm.forearm, vec![6, 7, 8, 9]);
        assert_eq!(arm.subtree.len(), 9);
        assert_eq!(subtree(&ARM, 6), vec![6, 7, 8, 9]);
        assert_eq!(children(&ARM, 2), vec![3, 4, 5, 6]);
        // Fingers: the three one-joint chains beside the forearm.
        let fingers = finger_chains(&ARM, 2, 6);
        assert_eq!(fingers, vec![(2, vec![3]), (2, vec![4]), (2, vec![5])]);
        // Both arms under one root: that root is no arm, and not an attach joint.
        let mut both = ARM.to_vec();
        for &parent in &ARM[1..] {
            both.push(if parent == 0 { 0 } else { parent + 9 });
        }
        assert!(find_arm(&both, 0).is_none());
        let mut distances = vec![9.0; both.len()];
        distances[0] = 0.0;
        distances[1] = 0.0;
        assert_eq!(attach_joint(&both, &distances), Some(1));
    }
}
