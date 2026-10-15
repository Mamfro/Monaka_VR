use super::*;

const DOWN: [[f32; 3]; 2] = [[-0.3, -0.7, -0.1], [0.3, -0.37, -0.36]];
const UP: [[f32; 3]; 2] = [[-0.3, 0.1, -0.4], [0.3, 0.12, -0.4]];

#[test]
fn both_hands_up_hand_the_arms_to_the_game_and_back() {
    let mut t = Traversal::new();
    assert_eq!(t.step(1000, DOWN, false, None), (1.0, false));
    assert_eq!(t.step(1016, UP, false, None), (1.0, false), "not before the hold");
    let (weight, switched) = t.step(1100, UP, false, None);
    assert!(switched && t.game && weight < 1.0);
    let (weight, _) = t.step(1300, UP, false, None);
    assert_eq!(weight, 0.0, "blended out");
    // One hand down mid-climb keeps it.
    let one_down = [UP[0], DOWN[1]];
    assert!(!t.step(1400, one_down, false, None).1);
    assert!(!t.step(2000, one_down, false, None).1 && t.game);
    assert!(!t.step(2050, [[-0.3, -0.1, -0.4], DOWN[1]], false, None).1 && t.game, "just above the stay line");
    // Both down: back after the hold, blending in.
    assert!(!t.step(2100, DOWN, false, None).1);
    assert!(!t.step(2390, DOWN, false, None).1);
    let (weight, switched) = t.step(2400, DOWN, false, None);
    assert!(switched && !t.game && weight > 0.0 && weight < 1.0);
    assert_eq!(t.step(2700, DOWN, false, None).0, 1.0);
}

#[test]
fn one_hand_holding_on_the_other_reaching_back() {
    let mut t = Traversal::new();
    let left_holds = [UP[0], [0.35, -0.5, 0.2]];
    t.step(1000, left_holds, false, None);
    assert!(t.step(1100, left_holds, false, None).1 && t.game);
}

#[test]
fn turned_away_from_the_ledge_the_hand_behind_still_holds_on() {
    let mut t = Traversal::new();
    t.step(1000, UP, false, None);
    assert!(t.step(1100, UP, false, None).1 && t.game);
    // Measured: left reaching ahead, right still on the ledge behind; then mirrored; then mid-turn.
    let poses = [[[-0.13, -0.39, -0.49], [0.69, -0.23, 0.21]], [[-0.67, -0.24, 0.27], [0.10, -0.39, -0.49]], [[0.48, -0.12, -0.27], [0.38, -0.19, 0.65]]];
    for (i, pose) in poses.iter().enumerate() {
        for now in (0..1000).step_by(16) {
            assert!(!t.step(2000 + i as u64 * 1000 + now, *pose, false, None).1 && t.game, "{pose:?}");
        }
    }
    // Back to walking with a weapon.
    t.step(6000, DOWN, false, None);
    assert!(t.step(6300, DOWN, false, None).1 && !t.game);
}

#[test]
fn a_hand_raised_short_of_the_eyes_or_an_attack_keeps_the_rig() {
    let mut t = Traversal::new();
    let one_up = [[-0.3, 0.0, -0.4], DOWN[1]];
    for now in (1000..2000).step_by(16) {
        assert_eq!(t.step(now, one_up, false, None), (1.0, false));
    }
    for now in (2000..3000).step_by(16) {
        assert_eq!(t.step(now, UP, true, None), (1.0, false), "an attack's own animation");
    }
}

#[test]
fn a_long_gap_finishes_the_blend() {
    let mut t = Traversal::new();
    t.step(1000, UP, false, None);
    t.step(1100, UP, false, None);
    assert_eq!(t.step(5000, UP, false, None).0, 0.0);
}

#[test]
fn climbing_reports_reach_the_run_s_situations() {
    report_climbing(true);
    assert_eq!(situation::state(Situation::Traversal), Some(true));
    report_climbing(false);
    assert_eq!(situation::state(Situation::Traversal), Some(false));
}

#[test]
fn a_game_that_reports_climbing_decides_whatever_the_wrists_do() {
    let mut t = Traversal::new();
    // Hands up (a throw's wind-up), the game says it is not climbing: the arms stay.
    t.step(1000, UP, false, Some(false));
    assert_eq!(t.step(1200, UP, false, Some(false)), (1.0, false));
    // Hands down on a ladder, the game says it climbs: the arms go to its animation.
    t.step(2000, DOWN, false, Some(true));
    assert!(t.step(2100, DOWN, false, Some(true)).1 && t.game);
}
