use super::*;

const S: f32 = std::f32::consts::FRAC_1_SQRT_2;

fn hand(orientation: Quat) -> HandPose {
    HandPose { orientation, position: [0.0; 3], valid: 1, received_tick: 0 }
}

/// A hand pointing ahead rolled `angle` radians about where it points.
fn rolled(angle: f32) -> Quat {
    [0.0, 0.0, (angle / 2.0).sin(), (angle / 2.0).cos()]
}

const FROM_START: Lockpicking = Lockpicking { twist: Twist::FromStart, pick_gain: 1.0, full_turn: 0.8, hand: (Point::Controller, At::Frame) };
const AGAINST_UP: Lockpicking = Lockpicking { twist: Twist::AgainstUp, pick_gain: 2.0, full_turn: std::f32::consts::FRAC_PI_3, hand: (Point::Palm, At::Latest) };

#[test]
fn a_quarter_roll_reads_a_quarter_turn_either_way() {
    let quarter = roll_between([0.0, 0.0, 0.0, 1.0], [0.0, 0.0, S, S]);
    assert!((quarter.abs() - std::f32::consts::FRAC_PI_2).abs() < 1e-4, "{quarter}");
    let back = roll_between([0.0, 0.0, 0.0, 1.0], [0.0, 0.0, -S, S]);
    assert!(quarter * back < 0.0, "{quarter} {back}");
}

#[test]
fn the_pick_scales_and_the_turn_clamps() {
    assert_eq!(AGAINST_UP.answer(Action::Pick, 0.5), 1.0);
    assert_eq!(FROM_START.answer(Action::Turn, 0.4), 0.5);
    assert_eq!(FROM_START.answer(Action::Turn, -3.0), -1.0, "past a full turn: a full turn");
}

#[test]
fn an_unreadable_right_wrist_starts_level_against_up() {
    let mut picking = Picking::new();
    let level = hand([0.0, 0.0, 0.0, 1.0]);
    picking.value(&AGAINST_UP, Action::Pick, Reading { fresh: true }, [Some(level), None]);
    // The right wrist a sixth of a turn from level: fully turned.
    let turn = picking.value(&AGAINST_UP, Action::Turn, Reading::default(), [Some(level), Some(hand(rolled(-1.1)))]).unwrap();
    assert!((turn.abs() - 1.0).abs() < 1e-6, "{turn}");
    // Pointing straight up there is no roll: no value.
    assert_eq!(picking.value(&AGAINST_UP, Action::Pick, Reading::default(), [Some(hand([S, 0.0, 0.0, S])), None]), None);
}

#[test]
fn a_turn_begun_before_the_pick_keeps_its_start() {
    let mut picking = Picking::new();
    let (level, twisted) = (hand(rolled(0.0)), hand(rolled(0.4)));
    // The turn's own first read, then the pick's with the right hand turned since: the turn
    // counts from its own start.
    picking.value(&FROM_START, Action::Turn, Reading { fresh: true }, [None, Some(level)]);
    picking.value(&FROM_START, Action::Pick, Reading { fresh: true }, [Some(level), Some(twisted)]);
    let turn = picking.value(&FROM_START, Action::Turn, Reading::default(), [Some(level), Some(twisted)]).unwrap();
    assert!((turn.abs() - 0.5).abs() < 1e-5, "{turn}");
    // The next lock begins at the pick: both hands start there.
    picking.value(&FROM_START, Action::Pick, Reading { fresh: true }, [Some(level), Some(twisted)]);
    assert_eq!(picking.value(&FROM_START, Action::Turn, Reading::default(), [Some(level), Some(twisted)]), Some(0.0));
    assert_eq!(picking.locks(), 2);
}

#[test]
fn a_hand_untracked_as_the_lock_begins_starts_at_its_first_read() {
    let mut picking = Picking::new();
    picking.value(&FROM_START, Action::Pick, Reading { fresh: true }, [Some(hand(rolled(0.0))), None]);
    let right = Some(hand(rolled(0.7)));
    assert_eq!(picking.value(&FROM_START, Action::Turn, Reading::default(), [None, right]), Some(0.0));
}
