use super::*;
use crate::Snapshot;

#[test]
fn a_turn_between_nearly_opposite_directions_is_left_alone() {
    assert!(turn([0.0; 3], [0.0, 0.0, -1.0], [0.0, 0.6, -0.8]).is_some());
    assert!(turn([0.0; 3], [0.0, 0.0, -1.0], [0.0, 0.05, 1.0]).is_none());
}

#[test]
fn the_upper_arm_reaches_the_elbow_stretching_no_further_than_allowed() {
    let shoulder = [0.0, 1.5, 0.0];
    // The elbow 0.3 m down, moved to 0.3 m ahead: turned, no stretch.
    let reach = reach_for(shoulder, [0.0, 1.2, 0.0], [0.0, 1.5, -0.3], [0.6, 1.6]).unwrap();
    let elbow = apply_3x3(&reach, [0.0, -0.3, 0.0]);
    assert!((elbow[2] + 0.3).abs() < 1e-5 && elbow[1].abs() < 1e-5, "{elbow:?}");
    // Twice as far: stretched to at most 1.6 times.
    let reach = reach_for(shoulder, [0.0, 1.2, 0.0], [0.0, 0.9, 0.0], [0.6, 1.6]).unwrap();
    let elbow = [0, 1, 2].map(|k| reach[4 * k + 3] + apply_3x3(&reach, [0.0, 1.2, 0.0])[k]);
    assert!((elbow[1] - (1.5 - 0.3 * 1.6)).abs() < 1e-5, "{elbow:?}");
}

#[test]
fn the_hand_turns_at_the_wrist_so_the_barrel_points_along_the_controller() {
    // Three elements: the forearm (elbow), the wrist, the gun's holder on the hand.
    let at = |p: Vec3| [1.0, 0.0, 0.0, p[0], 0.0, 1.0, 0.0, p[1], 0.0, 0.0, 1.0, p[2]];
    let snapshot = Snapshot::of(1, vec![at([0.0, 1.0, 0.0]), at([0.0, 1.0, -0.3]), at([0.0, 1.0, -0.4])]).unwrap();
    let mut frames = Frames::new(&snapshot);
    // The barrel points up as posed; the controller points ahead (-z).
    let lined = LinedUp { barrel: Some([0.0, 1.0, 0.0]), holder: 2, wrist: 1, hand: &[1, 2], forearm: None };
    let palm = HandPose { position: [0.0; 3], orientation: [0.0, 0.0, 0.0, 1.0], valid: 1, received_tick: 0 };
    line_up(&mut frames, &lined, &at([0.0; 3]), Some(&palm));
    let barrel = apply_3x3(&frames.now[2], [0.0, 1.0, 0.0]);
    assert!((barrel[2] + 1.0).abs() < 1e-5, "{barrel:?}");
    let wrist = position(&frames.now[1]);
    assert!(wrist.iter().zip([0.0, 1.0, -0.3]).all(|(a, b)| (a - b).abs() < 1e-5), "about the wrist: {wrist:?}");
    assert_eq!(frames.now[0], snapshot.worlds[0], "the forearm stays");
}
