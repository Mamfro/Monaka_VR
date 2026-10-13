use super::*;

#[test]
fn a_turn_keeps_the_head_where_it_is_in_the_world() {
    // The head 1 m along tracking x from the character; the world turns 90 degrees.
    let (anchor, head) = ([0.0, 0.0], [1.0, 1.7, 0.0]);
    let old = ([1.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
    let new = ([0.0, 0.0, -1.0], [1.0, 0.0, 0.0]);
    let a = turned_anchor(anchor, head, old, new);
    let world = |anchor: [f32; 2], axes: ([f32; 3], [f32; 3])| {
        let o = [head[0] - anchor[0], head[2] - anchor[1]];
        [0, 1, 2].map(|k| axes.0[k] * o[0] + axes.1[k] * o[1])
    };
    let (before, after) = (world(anchor, old), world(a, new));
    assert!((0..3).all(|k| (before[k] - after[k]).abs() < 1e-5), "{before:?} {after:?}");
    assert_eq!(turned_anchor(anchor, head, old, old), anchor, "no turn, no change");
}

#[test]
fn eyes_on_the_moved_back_camera_are_those_on_the_moved_back_origin() {
    use monaka_core::camera::{apply_head, distance, from_back_up, tracking_origin, view_centre};
    // A pitched game camera turned 30 degrees left, 0.2 m walked to the right and 0.1 back.
    let yaw = 30f32.to_radians();
    let game = from_back_up([yaw.sin() * 0.9, 0.43, yaw.cos() * 0.9], [0.0, 1.0, 0.0], [5.0, 1.0, 2.0]).unwrap();
    let (anchor, head, q) = ([0.2, 0.1], [0.25, 1.6, 0.05], [0.0, 0.2588, 0.0, 0.9659]);
    for base in [Base::Game, Base::Level { baked_yaw: 0.3 }] {
        // As the eye sites did it: the tracking origin shifted, the head applied to it.
        let mut origin = match base {
            Base::Level { baked_yaw } => tracking_origin(&game, baked_yaw).unwrap(),
            _ => game,
        };
        origin[3] -= origin[0] * anchor[0] + origin[2] * anchor[1];
        origin[7] -= origin[4] * anchor[0] + origin[6] * anchor[1];
        origin[11] -= origin[8] * anchor[0] + origin[10] * anchor[1];
        let before = apply_head(&origin, q, head);
        let moved = moved_back(&game, &base.origin(&game).unwrap(), anchor);
        let after = view_centre(&moved, base, q, head).unwrap();
        assert!(distance(&before, &after) < 1e-5, "{base:?}: {before:?} {after:?}");
    }
}

#[test]
fn a_step_is_measured_at_the_next_beyond_the_game_s_own_move() {
    let mut last = LastStep::new();
    let mut credited = Vec::new();
    // The first step has nothing before it to measure.
    let first = last.begin(Some([0.0, 0.0, 0.0]), 0.02, || None, |_| [1.0, 0.0], |b, f, dt| credited.push((b, f, dt)));
    assert_eq!(first, [1.0, 0.0]);
    assert!(credited.is_empty());
    last.game([2.0, 0.0, 0.0]);
    // The body went 0.06 m: 0.04 of it the game's own (2 m/s for 0.02 s), 0.02 following's.
    last.begin(Some([0.06, 0.0, 0.0]), 0.02, || Some(Wait::Cutscene), |w| if w.is_some() { [0.0; 2] } else { [1.0, 0.0] }, |b, f, dt| {
        credited.push((b, f, dt))
    });
    assert_eq!(credited.len(), 1);
    let (beyond, follow, dt) = credited[0];
    assert!((beyond[0] - 0.02).abs() < 1e-6 && beyond[1] == 0.0 && beyond[2] == 0.0, "{beyond:?}");
    assert_eq!((follow, dt), ([1.0, 0.0], 0.02));
}

#[test]
fn an_unreadable_body_measures_nothing() {
    let mut last = LastStep::new();
    let mut credited = 0;
    last.begin(Some([0.0; 3]), 0.02, || None, |_| [1.0, 0.0], |_, _, _| credited += 1);
    last.begin(Some([f32::NAN, 0.0, 0.0]), 0.02, || None, |_| [1.0, 0.0], |_, _, _| credited += 1);
    last.begin(Some([0.02, 0.0, 0.0]), 0.02, || None, |_| [1.0, 0.0], |_, _, _| credited += 1);
    assert_eq!(credited, 0, "a step either side of an unreadable one is not measured");
}
