//! The hand rig's debug views (`monaka_contract::debug_draw`), drawn for the frame the game is drawing
//! (`monaka_producer::debug::drawing`):
//!
//! - Skeleton: every bone to its parent as the rig left it, the bones it wrote orange, the rest
//!   white; where it moved a bone, the game's own animated bone in blue.
//! - Bone axes: each bone's axes as the rig left it.
//! - Fingers: the bones from each wrist down, with small axes.
//! - Hand targets: the controllers' palms (axes) and where the rig put the aim palm (a point).
//!
//! A skeleton is drawn in tracking space through its moment: the palms' origin for
//! [`super::Method::Carried`] skeletons (the game world), the attached hands' space otherwise. A
//! moment with no hand tracked is drawn too, as the game animated the arms (nothing moved): the
//! views need no controllers ([`views_on`] tells a game to make such moments while one is on).

use super::{HandRig, Hands, Inputs};
use crate::skeleton::subtree;
use crate::{Frames, Placed, Skeleton};
use monaka_core::camera::{Mat34, compose, position, rigid_inverse};
use monaka_contract::debug_draw::{self, Category, GREEN, ORANGE, Primitive, WHITE, YELLOW, axes, rgba};
use monaka_contract::protocol::{LEFT_HAND, RIGHT_HAND};
use monaka_producer::debug;

const BONE_AXES: f32 = 0.025;
const FINGER_AXES: f32 = 0.012;
const PALM_AXES: f32 = 0.06;
const CATEGORIES: [Category; 4] = [Category::Skeleton, Category::BoneAxes, Category::Fingers, Category::HandTargets];

/// Whether any of the rig's views is on.
pub fn views_on() -> bool {
    CATEGORIES.into_iter().any(debug::on)
}

/// Says the rig can draw its views and leave the arms to the game.
pub(super) fn offer() {
    for category in CATEGORIES {
        debug::offer(category);
    }
    debug::offer(Category::SkipHandRig);
}

/// Draws the rig's views of one posing: `frames` as the rig left them, `written` the bones it
/// changed (left with the game when `left_to_game`).
pub(super) fn draw<G: HandRig>(skeleton: &dyn Skeleton, frames: &Frames, written: &[usize], inputs: &Inputs, placed: &Placed, left_to_game: bool) {
    let wanted = CATEGORIES.map(debug::on);
    if !wanted.contains(&true) {
        return;
    }
    let key = debug::drawing();
    let space = space(&inputs.hands);
    let [skeleton_on, axes_on, fingers_on, targets_on] = wanted;
    let to_tracking = |m: &Mat34| compose(&space, m);
    let finite = |m: &Mat34| m.iter().all(|v| v.is_finite());
    let count = (skeleton.count().max(0) as usize).min(frames.now.len());
    let parents: Vec<i32> = (0..count as i32).map(|e| skeleton.parent(e)).collect();
    let shown = if left_to_game { frames.animated } else { &frames.now[..] };
    let mut out: Vec<Primitive> = Vec::new();

    if skeleton_on {
        for (element, &parent) in parents.iter().enumerate() {
            let Ok(parent) = usize::try_from(parent) else { continue };
            if !skeleton.is_bone(element as i32) || parent >= count {
                continue;
            }
            let moved = written.contains(&element);
            let (a, b) = (&shown[parent], &shown[element]);
            if finite(a) && finite(b) {
                let colour = if moved && !left_to_game { ORANGE } else { WHITE };
                out.push(Primitive::segment(Category::Skeleton, position(&to_tracking(a)), position(&to_tracking(b)), colour));
            }
            let (a, b) = (&frames.animated[parent], &frames.animated[element]);
            if moved && !left_to_game && finite(a) && finite(b) {
                out.push(Primitive::segment(Category::Skeleton, position(&to_tracking(a)), position(&to_tracking(b)), rgba(0.3, 0.5, 1.0, 0.6)));
            }
        }
    }
    if axes_on {
        for element in (0..count).filter(|&e| skeleton.is_bone(e as i32) && finite(&shown[e])) {
            out.extend(axes(Category::BoneAxes, &to_tracking(&shown[element]), BONE_AXES));
        }
    }
    if fingers_on {
        // The fingers where the finger rig finds them (by name, table or shape), else everything
        // under the wrists the rig found by shape.
        let hands: Vec<Vec<usize>> = [LEFT_HAND, RIGHT_HAND]
            .into_iter()
            .filter_map(|side| match &G::RIG.fingers {
                Some(source) => crate::finger_rig::hand_elements(skeleton, frames, source, side),
                None => frames.wrists[side].map(|(wrist, _)| subtree(&parents, wrist)),
            })
            .collect();
        for hand in &hands {
            for &element in hand.iter().skip(1).filter(|&&e| e < count && finite(&shown[e])) {
                let m = to_tracking(&shown[element]);
                out.extend(axes(Category::Fingers, &m, FINGER_AXES));
                if let Some(parent) = usize::try_from(parents[element]).ok().filter(|p| finite(&shown[*p])) {
                    out.push(Primitive::segment(Category::Fingers, position(&to_tracking(&shown[parent])), position(&m), debug_draw::CYAN));
                }
            }
        }
    }
    if targets_on {
        if let Hands::Palms(palms) = &inputs.hands {
            for palm in palms.palms.iter().flatten() {
                out.extend(axes(Category::HandTargets, &pose_matrix(palm.orientation, palm.position), PALM_AXES));
            }
            if finite(&placed.palm) {
                out.push(Primitive::point(Category::HandTargets, position(&to_tracking(&placed.palm)), YELLOW));
            }
        }
        if let Hands::Attached { targets, .. } = &inputs.hands {
            for target in targets.iter().flatten() {
                let m = to_tracking(target);
                out.push(Primitive::point(Category::HandTargets, position(&m), GREEN));
                out.extend(axes(Category::HandTargets, &m, PALM_AXES));
            }
        }
    }
    if !out.is_empty() {
        debug::add(key, &out);
    }
}

/// Skeleton-to-tracking for this moment.
fn space(hands: &Hands) -> Mat34 {
    match hands {
        Hands::Palms(palms) => rigid_inverse(&palms.origin),
        Hands::Attached { space, .. } => *space,
    }
}

/// A pose (tracking space) as a [`Mat34`].
fn pose_matrix(orientation: [f32; 4], at: [f32; 3]) -> Mat34 {
    let [x, y, z] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]].map(|axis| monaka_core::math::rotate(orientation, axis));
    [x[0], y[0], z[0], at[0], x[1], y[1], z[1], at[1], x[2], y[2], z[2], at[2]]
}
