use super::*;

/// A turn of `degrees` about the up axis.
fn yaw(degrees: f32) -> [f32; 4] {
    let half = degrees.to_radians() / 2.0;
    [0.0, half.sin(), 0.0, half.cos()]
}

#[test]
fn pairs_held_two_refreshes_each_are_counted_as_two() {
    let now = Instant::now();
    let mut cadence = Cadence::new(now);
    for sequence in [1, 1, 2, 2, 3, 3, 4] {
        cadence.refresh(Some((sequence, yaw(0.0))), Some(yaw(0.0)), now);
    }
    assert_eq!(cadence.held, [0, 3, 0, 0], "the fourth pair is still up");
}

#[test]
fn a_pair_held_three_refreshes_shows_as_a_three() {
    let now = Instant::now();
    let mut cadence = Cadence::new(now);
    for sequence in [1, 1, 2, 2, 2, 3] {
        cadence.refresh(Some((sequence, yaw(0.0))), Some(yaw(0.0)), now);
    }
    assert_eq!(cadence.held, [0, 1, 1, 0]);
}

#[test]
fn the_turn_to_the_head_and_its_change_are_measured() {
    let now = Instant::now();
    let mut cadence = Cadence::new(now);
    cadence.refresh(Some((1, yaw(10.0))), Some(yaw(11.0)), now);
    cadence.refresh(Some((1, yaw(10.0))), Some(yaw(13.5)), now);
    assert!((cadence.angle_max - 3.5).abs() < 0.01, "{}", cadence.angle_max);
    assert!((cadence.change_max - 2.5).abs() < 0.01, "{}", cadence.change_max);
    assert!((angle_between(yaw(0.0), yaw(90.0)) - 90.0).abs() < 0.01);
}
