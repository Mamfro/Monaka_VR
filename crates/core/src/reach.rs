//! Two-bone reach (an arm's shoulder, elbow and wrist): where the elbow goes so the wrist reaches a
//! target with the bone lengths kept, and the rigid turns that carry an animated arm there. For a
//! game's first-person arm put on a tracked controller while the body stays the game's.

use crate::camera::{Mat34, compose, quaternion, rigid_inverse, translation};
use crate::math::{Vec3, add, cross, dot, length, scale, sub, unit};

/// Where the elbow goes for a shoulder at `shoulder`, bones `upper` and `lower` long, the wrist
/// at `target`, bending toward `pole` (a direction, e.g. down and out for a right arm). A target
/// out of reach straightens the arm toward it; one closer than the bones can fold is pushed out.
pub fn elbow(shoulder: Vec3, target: Vec3, upper: f32, lower: f32, pole: Vec3) -> Option<Vec3> {
    // Bones too short to bend (a collapsed animated pose) have no elbow; this also keeps the
    // clamp below ordered.
    if !(upper.is_finite() && lower.is_finite() && upper.min(lower) > 1e-3) {
        return None;
    }
    let toward = sub(target, shoulder);
    let dir = unit(toward)?;
    let reach = length(toward).clamp((upper - lower).abs() + 1e-4, upper + lower - 1e-4);
    // Law of cosines: the elbow's distance along the shoulder-target line.
    let along = (upper * upper + reach * reach - lower * lower) / (2.0 * reach);
    let out = (upper * upper - along * along).max(0.0).sqrt();
    // The pole, square to the line; any square direction if the pole lies along it.
    let side = unit(sub(pole, scale(dir, dot(pole, dir))))
        .or_else(|| unit(cross(dir, [0.0, 1.0, 0.0])))
        .or_else(|| unit(cross(dir, [1.0, 0.0, 0.0])))?;
    Some(add(add(shoulder, scale(dir, along)), scale(side, out)))
}

/// How far below a controller's pointing direction the forearm runs back from the wrist: an aim
/// ray points about this much above the grip's axis, and a straight wrist lines the grip up with
/// the forearm (radians, 35 degrees).
const FOREARM_BELOW_AIM: f32 = 0.61;

/// Where a hand held as `palm` (its frame in the world: columns right, up, back; it points along
/// -back, as a controller's aim pose) puts its elbow, as a pole for [`elbow`]: the forearm runs
/// back from the wrist, [`FOREARM_BELOW_AIM`] under the pointing line, `forearm` long; the pole
/// points from the middle of the shoulder-wrist line to that elbow. A hand turned across the face
/// to read a watch swings it out and up; one pointing ahead keeps it down.
pub fn hand_pole(shoulder: Vec3, wrist: Vec3, palm: &Mat34, forearm: f32) -> Option<Vec3> {
    let (up, back) = ([palm[1], palm[5], palm[9]], [palm[2], palm[6], palm[10]]);
    let (s, c) = FOREARM_BELOW_AIM.sin_cos();
    let along = unit(sub(scale(back, c), scale(up, s)))?;
    let guess = add(wrist, scale(along, forearm));
    unit(sub(guess, scale(add(shoulder, wrist), 0.5)))
}

/// Wrist-derived pole with only its inward bend component limited. The bend
/// plane is perpendicular to shoulder->wrist; `right` is torso right. Wrist
/// pitch/roll can still raise, lower and swing the elbow within the outer half
/// of that plane. A small outward margin avoids a degenerate medial reversal.
pub fn bounded_hand_pole(shoulder: Vec3, wrist: Vec3, palm: &Mat34, forearm: f32, right: Vec3, left: bool) -> Option<Vec3> {
    if !right.iter().all(|v| v.is_finite()) { return None; }
    let hand = hand_pole(shoulder, wrist, palm, forearm)?;
    let along = unit(sub(wrist, shoulder))?;
    let bend = unit(sub(hand, scale(along, dot(hand, along))))?;
    let right = unit([right[0], 0.0, right[2]])?;
    let outward = scale(right, if left { -1.0 } else { 1.0 });
    let Some(outward) = unit(sub(outward, scale(along, dot(outward, along)))) else {
        // A straight lateral reach has no distinct inner/outer bend half-plane.
        return Some(bend);
    };
    unit(add(bend, scale(outward, (0.05 - dot(bend, outward)).max(0.0))))
}

/// The rotation about `pivot` that turns direction `from` onto `to` (the shortest turn), as a
/// matrix that applies to world points and frames.
pub fn turn_about(pivot: Vec3, from: Vec3, to: Vec3) -> Option<Mat34> {
    let (a, b) = (unit(from)?, unit(to)?);
    let c = dot(a, b).clamp(-1.0, 1.0);
    let axis = cross(a, b);
    let r: [f32; 9] = match unit(axis) {
        Some(k) => {
            // Rodrigues: R = cI + s[k]x + (1-c)kk^T.
            let s = length(axis);
            let t = 1.0 - c;
            [
                c + t * k[0] * k[0],
                t * k[0] * k[1] - s * k[2],
                t * k[0] * k[2] + s * k[1],
                t * k[1] * k[0] + s * k[2],
                c + t * k[1] * k[1],
                t * k[1] * k[2] - s * k[0],
                t * k[2] * k[0] - s * k[1],
                t * k[2] * k[1] + s * k[0],
                c + t * k[2] * k[2],
            ]
        }
        // Already lined up.
        None if c > 0.0 => [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        // A half turn: about any axis square to the direction.
        None => {
            let k = unit(cross(a, [0.0, 1.0, 0.0])).or_else(|| unit(cross(a, [1.0, 0.0, 0.0])))?;
            // R = 2kk^T - I.
            [
                2.0 * k[0] * k[0] - 1.0,
                2.0 * k[0] * k[1],
                2.0 * k[0] * k[2],
                2.0 * k[1] * k[0],
                2.0 * k[1] * k[1] - 1.0,
                2.0 * k[1] * k[2],
                2.0 * k[2] * k[0],
                2.0 * k[2] * k[1],
                2.0 * k[2] * k[2] - 1.0,
            ]
        }
    };
    let rotation = [r[0], r[1], r[2], 0.0, r[3], r[4], r[5], 0.0, r[6], r[7], r[8], 0.0];
    Some(compose(&compose(&translation(pivot), &rotation), &translation(scale(pivot, -1.0))))
}

/// The turn by `angle` radians (right-handed) about the line through `pivot` along `axis`, as a
/// matrix that applies to world points and frames.
pub fn spin_about(pivot: Vec3, axis: Vec3, angle: f32) -> Option<Mat34> {
    let k = unit(axis)?;
    let (s, c) = angle.sin_cos();
    let t = 1.0 - c;
    let rotation = [
        c + t * k[0] * k[0],
        t * k[0] * k[1] - s * k[2],
        t * k[0] * k[2] + s * k[1],
        0.0,
        t * k[1] * k[0] + s * k[2],
        c + t * k[1] * k[1],
        t * k[1] * k[2] - s * k[0],
        0.0,
        t * k[2] * k[0] - s * k[1],
        t * k[2] * k[1] + s * k[0],
        c + t * k[2] * k[2],
        0.0,
    ];
    Some(compose(&compose(&translation(pivot), &rotation), &translation(scale(pivot, -1.0))))
}

/// How far the move `to` turns things about `axis` beyond the move `from` (radians,
/// right-handed, within a half turn): the twist part of the turn between their rotations, any bend
/// across the axis left out. What a forearm's twist bones take of a hand turned against it.
pub fn twist_between(from: &Mat34, to: &Mat34, axis: Vec3) -> Option<f32> {
    let k = unit(axis)?;
    // The turn from one to the other: to * from^T (rotations only).
    let q = quaternion(&compose(to, &rigid_inverse(from)));
    let (w, v) = (q[3], [q[0], q[1], q[2]]);
    // The twist about the axis: the quaternion's part along it.
    let angle = 2.0 * dot(v, k).atan2(w);
    let wrapped = crate::math::wrap_radians(angle);
    wrapped.is_finite().then_some(wrapped)
}

/// A point moved by a 3x4 matrix.
pub fn apply(m: &Mat34, p: Vec3) -> Vec3 {
    [0, 1, 2].map(|r| m[r * 4] * p[0] + m[r * 4 + 1] * p[1] + m[r * 4 + 2] * p[2] + m[r * 4 + 3])
}

/// The moves for an arm: given where its shoulder, elbow and wrist are now (`shoulder`, `elbow`,
/// `wrist`), the new shoulder and the wrist target, the moves for the upper arm and for the
/// forearm (each applies to that bone's world frame and to the frames that turn with it).
pub fn arm_moves(now: [Vec3; 3], shoulder: Vec3, target: Vec3, pole: Vec3) -> Option<[Mat34; 2]> {
    let [s0, e0, w0] = now;
    let (upper, lower) = (length(sub(e0, s0)), length(sub(w0, e0)));
    let elbow_at = elbow(shoulder, target, upper, lower, pole)?;
    // Upper arm: carried to the new shoulder, then turned so its elbow lands.
    let carry = translation(sub(shoulder, s0));
    let upper_move = compose(&turn_about(shoulder, sub(e0, s0), sub(elbow_at, shoulder))?, &carry);
    // Forearm: rides along with the upper arm, then turned about the elbow onto the target.
    let wrist_now = apply(&upper_move, w0);
    let lower_move = compose(&turn_about(elbow_at, sub(wrist_now, elbow_at), sub(target, elbow_at))?, &upper_move);
    let moves = [upper_move, lower_move];
    moves.iter().all(|m| m.iter().all(|v| v.is_finite())).then_some(moves)
}

#[cfg(test)]
mod tests;
