//! Two-bone reach (an arm's shoulder, elbow and wrist): where the elbow goes so the wrist reaches a
//! target with the bone lengths kept, and the rigid turns that carry an animated arm there. For a
//! game's first-person arm put on a tracked controller while the body stays the game's.

use crate::camera::{Mat34, compose, translation};
use crate::math::Vec3;

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn scale(a: Vec3, s: f32) -> Vec3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn length(a: Vec3) -> f32 {
    dot(a, a).sqrt()
}

fn normalized(a: Vec3) -> Option<Vec3> {
    let l = length(a);
    (l.is_finite() && l > 1e-6).then(|| scale(a, 1.0 / l))
}

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
    let dir = normalized(toward)?;
    let reach = length(toward).clamp((upper - lower).abs() + 1e-4, upper + lower - 1e-4);
    // Law of cosines: the elbow's distance along the shoulder-target line.
    let along = (upper * upper + reach * reach - lower * lower) / (2.0 * reach);
    let out = (upper * upper - along * along).max(0.0).sqrt();
    // The pole, square to the line; any square direction if the pole lies along it.
    let side = normalized(sub(pole, scale(dir, dot(pole, dir))))
        .or_else(|| normalized(cross(dir, [0.0, 1.0, 0.0])))
        .or_else(|| normalized(cross(dir, [1.0, 0.0, 0.0])))?;
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
    let along = normalized(sub(scale(back, c), scale(up, s)))?;
    let guess = add(wrist, scale(along, forearm));
    normalized(sub(guess, scale(add(shoulder, wrist), 0.5)))
}

/// Wrist-derived pole with only its inward bend component limited. The bend
/// plane is perpendicular to shoulder->wrist; `right` is torso right. Wrist
/// pitch/roll can still raise, lower and swing the elbow within the outer half
/// of that plane. A small outward margin avoids a degenerate medial reversal.
pub fn bounded_hand_pole(shoulder: Vec3, wrist: Vec3, palm: &Mat34, forearm: f32, right: Vec3, left: bool) -> Option<Vec3> {
    if !right.iter().all(|v| v.is_finite()) { return None; }
    let hand = hand_pole(shoulder, wrist, palm, forearm)?;
    let along = normalized(sub(wrist, shoulder))?;
    let bend = normalized(sub(hand, scale(along, dot(hand, along))))?;
    let right = normalized([right[0], 0.0, right[2]])?;
    let outward = scale(right, if left { -1.0 } else { 1.0 });
    let Some(outward) = normalized(sub(outward, scale(along, dot(outward, along)))) else {
        // A straight lateral reach has no distinct inner/outer bend half-plane.
        return Some(bend);
    };
    normalized(add(bend, scale(outward, (0.05 - dot(bend, outward)).max(0.0))))
}

/// The rotation about `pivot` that turns direction `from` onto `to` (the shortest turn), as a
/// matrix that applies to world points and frames.
pub fn turn_about(pivot: Vec3, from: Vec3, to: Vec3) -> Option<Mat34> {
    let (a, b) = (normalized(from)?, normalized(to)?);
    let c = dot(a, b).clamp(-1.0, 1.0);
    let axis = cross(a, b);
    let r: [f32; 9] = match normalized(axis) {
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
            let k = normalized(cross(a, [0.0, 1.0, 0.0])).or_else(|| normalized(cross(a, [1.0, 0.0, 0.0])))?;
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
    let k = normalized(axis)?;
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
    let k = normalized(axis)?;
    // The turn from one to the other: to * from^T (rotations only).
    let m = |i: usize, j: usize| (0..3).map(|n| to[i * 4 + n] * from[j * 4 + n]).sum::<f32>();
    // Its quaternion (Shepperd: from the largest of the diagonal terms, stable at any angle).
    let trace = m(0, 0) + m(1, 1) + m(2, 2);
    let (w, v) = if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        (s / 4.0, [(m(2, 1) - m(1, 2)) / s, (m(0, 2) - m(2, 0)) / s, (m(1, 0) - m(0, 1)) / s])
    } else if m(0, 0) > m(1, 1) && m(0, 0) > m(2, 2) {
        let s = (1.0 + m(0, 0) - m(1, 1) - m(2, 2)).sqrt() * 2.0;
        ((m(2, 1) - m(1, 2)) / s, [s / 4.0, (m(0, 1) + m(1, 0)) / s, (m(0, 2) + m(2, 0)) / s])
    } else if m(1, 1) > m(2, 2) {
        let s = (1.0 + m(1, 1) - m(0, 0) - m(2, 2)).sqrt() * 2.0;
        ((m(0, 2) - m(2, 0)) / s, [(m(0, 1) + m(1, 0)) / s, s / 4.0, (m(1, 2) + m(2, 1)) / s])
    } else {
        let s = (1.0 + m(2, 2) - m(0, 0) - m(1, 1)).sqrt() * 2.0;
        ((m(1, 0) - m(0, 1)) / s, [(m(0, 2) + m(2, 0)) / s, (m(1, 2) + m(2, 1)) / s, s / 4.0])
    };
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
mod tests {
    use super::*;

    #[test]
    fn bounded_hand_poles_keep_wrist_response_and_limit_inward_bending() {
        let mut saw_up = false;
        let mut saw_down = false;
        for left in [true, false] {
            let side = if left { -1.0 } else { 1.0 };
            let shoulder = [side * 0.2, 0.0, 0.0];
            let wrist = [side * 0.2, -0.2, -0.3];
            for degrees in (-180..=180).step_by(10) {
                let (s,c) = ((degrees as f32).to_radians()*0.5).sin_cos();
                let palm = crate::camera::apply_head(&translation(wrist), [0.0,0.0,s,c], [0.0;3]);
                let pole = bounded_hand_pole(shoulder,wrist,&palm,0.3,[1.0,0.0,0.0],left).unwrap();
                let elbow = elbow(shoulder,wrist,0.3,0.3,pole).unwrap();
                assert!(side * (elbow[0]-shoulder[0]) > 0.0);
                saw_up |= elbow[1] > -0.1;
                saw_down |= elbow[1] < -0.1;
                let along = normalized(sub(wrist,shoulder)).unwrap();
                assert!(dot(pole,along).abs()<1e-5);
            }
        }
        assert!(saw_up && saw_down);
    }

    fn close(a: Vec3, b: Vec3) -> bool {
        length(sub(a, b)) < 1e-4
    }

    #[test]
    fn a_spin_keeps_its_line_and_its_twist_reads_back() {
        let (pivot, axis) = ([0.1, -0.3, -0.2], [0.3, -0.8, -0.5]);
        for angle in [-3.0f32, -1.2, -0.3, 0.0, 0.4, 1.5, 3.0] {
            let spin = spin_about(pivot, axis, angle).unwrap();
            assert!(close(apply(&spin, pivot), pivot));
            assert!(close(apply(&spin, add(pivot, axis)), add(pivot, axis)), "points on the line stay");
            let twist = twist_between(&translation([0.0; 3]), &spin, axis).unwrap();
            assert!((twist - angle).abs() < 1e-3, "{angle} read back as {twist}");
            // Against a base move, and about the reversed axis the other way.
            let base = spin_about([0.0; 3], [1.0, 0.2, 0.0], 0.7).unwrap();
            let moved = compose(&spin, &base);
            assert!((twist_between(&base, &moved, axis).unwrap() - angle).abs() < 1e-3);
            assert!((twist_between(&base, &moved, scale(axis, -1.0)).unwrap() + angle).abs() < 1e-3);
        }
    }

    #[test]
    fn the_hand_steers_the_elbow() {
        let (shoulder, forearm) = ([-0.2, -0.25, 0.0], 0.28);
        // A left hand reaching ahead, pointing ahead (identity: -Z ahead, +Y up): elbow down.
        let ahead = translation([0.0; 3]);
        let pole = hand_pole(shoulder, [-0.15, -0.3, -0.45], &ahead, forearm).unwrap();
        assert!(pole[1] < -0.5, "{pole:?}");
        // The same hand at the face, pointing right across it (a watch read): elbow out to the
        // left, and up from where a hand pointing ahead put it.
        let across = [0.0, 0.0, -1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0];
        let watch = hand_pole(shoulder, [-0.05, 0.0, -0.35], &across, forearm).unwrap();
        assert!(watch[0] < -0.5, "{watch:?}");
        let e = elbow(shoulder, [-0.05, 0.0, -0.35], 0.3, forearm, watch).unwrap();
        let low = elbow(shoulder, [-0.05, 0.0, -0.35], 0.3, forearm, [-0.6, -1.0, 0.0]).unwrap();
        assert!(e[0] < low[0] && e[1] > low[1], "out and up: {e:?} against {low:?}");
    }

    #[test]
    fn a_bend_across_the_axis_is_no_twist() {
        let bend = spin_about([0.0; 3], [1.0, 0.0, 0.0], 0.6).unwrap();
        let twist = twist_between(&translation([0.0; 3]), &bend, [0.0, 0.0, -1.0]).unwrap();
        assert!(twist.abs() < 1e-4, "{twist}");
    }

    #[test]
    fn elbow_keeps_the_bones_and_bends_toward_the_pole() {
        let (shoulder, target) = ([0.2, -0.25, 0.0], [0.15, -0.2, -0.45]);
        let e = elbow(shoulder, target, 0.3, 0.28, [0.0, -1.0, 0.0]).unwrap();
        assert!((length(sub(e, shoulder)) - 0.3).abs() < 1e-4);
        assert!((length(sub(target, e)) - 0.28).abs() < 1e-4);
        assert!(e[1] < -0.25, "bends down: {e:?}");
        // Out of reach: straight toward the target.
        let far = elbow(shoulder, [0.2, -0.25, -2.0], 0.3, 0.28, [0.0, -1.0, 0.0]).unwrap();
        assert!(close(far, [0.2, -0.25, -0.3]) || length(sub(far, [0.2, -0.25, -0.3])) < 0.01, "{far:?}");
        // Collapsed bones: no elbow, no panic.
        assert!(elbow(shoulder, target, 0.0, 0.28, [0.0, -1.0, 0.0]).is_none());
        assert!(elbow(shoulder, target, 0.3, f32::NAN, [0.0, -1.0, 0.0]).is_none());
        assert!(arm_moves([[0.0; 3]; 3], shoulder, target, [0.0, -1.0, 0.0]).is_none());
    }

    #[test]
    fn arm_moves_put_the_wrist_on_the_target() {
        let now = [[0.0, 0.0, 0.0], [0.0, -0.3, 0.0], [0.0, -0.3, -0.28]];
        let (shoulder, target) = ([0.1, 0.05, 0.02], [0.2, -0.1, -0.4]);
        let [upper, lower] = arm_moves(now, shoulder, target, [0.0, -1.0, 0.0]).unwrap();
        assert!(close(apply(&upper, now[0]), shoulder));
        let elbow_at = apply(&upper, now[1]);
        assert!(close(apply(&lower, now[1]), elbow_at), "forearm starts at the moved elbow");
        assert!(close(apply(&lower, now[2]), target), "wrist on target: {:?}", apply(&lower, now[2]));
        assert!((length(sub(elbow_at, shoulder)) - 0.3).abs() < 1e-4);
    }

    #[test]
    fn turn_about_turns_and_keeps_the_pivot() {
        let m = turn_about([1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [1.0, 0.0, 0.0]).unwrap();
        assert!(close(apply(&m, [1.0, 0.0, 0.0]), [1.0, 0.0, 0.0]));
        assert!(close(apply(&m, [1.0, 0.0, -1.0]), [2.0, 0.0, 0.0]));
        let half = turn_about([0.0; 3], [0.0, 0.0, -1.0], [0.0, 0.0, 1.0]).unwrap();
        assert!(close(apply(&half, [0.0, 0.0, -1.0]), [0.0, 0.0, 1.0]));
        assert!(turn_about([0.0; 3], [0.0; 3], [0.0, 0.0, 1.0]).is_none());
    }
}
