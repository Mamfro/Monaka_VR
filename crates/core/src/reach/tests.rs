use super::*;

#[test]
fn a_bounded_hand_pole_keeps_the_elbow_outside_and_still_follows_the_wrist() {
    // The palm rolled all the way round: the elbow never crosses inside its shoulder, yet it
    // still rises and falls with the roll, the pole always square to the arm.
    let mut saw_up = false;
    let mut saw_down = false;
    for left in [true, false] {
        let side = if left { -1.0 } else { 1.0 };
        let shoulder = [side * 0.2, 0.0, 0.0];
        let wrist = [side * 0.2, -0.2, -0.3];
        for degrees in (-180..=180).step_by(10) {
            let (s, c) = ((degrees as f32).to_radians() * 0.5).sin_cos();
            let palm = crate::camera::apply_head(&translation(wrist), [0.0, 0.0, s, c], [0.0; 3]);
            let pole = bounded_hand_pole(shoulder, wrist, &palm, 0.3, [1.0, 0.0, 0.0], left).unwrap();
            let elbow = elbow(shoulder, wrist, 0.3, 0.3, pole).unwrap();
            assert!(side * (elbow[0] - shoulder[0]) > 0.0, "{degrees}: the elbow crossed inside");
            saw_up |= elbow[1] > -0.1;
            saw_down |= elbow[1] < -0.1;
            let along = unit(sub(wrist, shoulder)).unwrap();
            assert!(dot(pole, along).abs() < 1e-5);
        }
    }
    assert!(saw_up && saw_down, "the elbow follows the wrist's roll");
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
