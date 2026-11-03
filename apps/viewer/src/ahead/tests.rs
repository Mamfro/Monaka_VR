use super::*;

const PERIOD: i64 = 11_111_111;

#[test]
fn the_pose_is_predicted_ahead_by_the_pairs_age_within_three_periods() {
    let mut ahead = Ahead::default();
    assert_eq!(ahead.time(1_000, PERIOD), 1_000, "no pair yet: the display time");
    ahead.pair(20);
    assert_eq!(ahead.time(1_000, PERIOD), 1_000 + 20_000_000);
    // A pair far older moves the estimate a little, not to it.
    ahead.pair(52);
    assert_eq!(ahead.time(0, PERIOD), 21_000_000);
    // Very old pairs: at most three periods ahead.
    for _ in 0..500 {
        ahead.pair(200);
    }
    assert_eq!(ahead.time(0, PERIOD), 3 * PERIOD);
    assert_eq!(ahead.time(0, 0), 0, "no period known: nothing ahead");
}

#[test]
fn a_pair_drawn_with_a_head_sent_predicts_the_whole_delay() {
    let mut ahead = Ahead::default();
    ahead.pair(20);
    let ms = 1_000_000;
    for (i, frame) in (0..10).map(|i| (i, i * 8 * ms)) {
        ahead.sent([0.0, i as f32, 0.0, 1.0], frame);
    }
    // The head sent at 16 ms is first shown at 90 ms: the game's work and the way to the headset.
    assert!(ahead.shown([0.0, 2.0, 0.0, 1.0], 90 * ms).is_some(), "the first match is said");
    assert_eq!(ahead.time(1_000, PERIOD), 1_000 + 74 * ms, "the whole delay, past three periods");
    assert!(ahead.shown([0.0, 9.0, 0.5, 1.0], 100 * ms).is_none(), "a pose of the game's own matches nothing");
    // Older heads went with the match; a head sent after it still matches.
    assert!(ahead.shown([0.0, 1.0, 0.0, 1.0], 100 * ms).is_none());
    ahead.shown([0.0, 3.0, 0.0, 1.0], 98 * ms);
    assert_eq!(ahead.time(0, PERIOD), 74 * ms, "the same delay again");
}

#[test]
fn the_measured_delay_is_capped() {
    let mut ahead = Ahead::default();
    ahead.sent([1.0, 0.0, 0.0, 0.0], 0);
    ahead.shown([1.0, 0.0, 0.0, 0.0], 400_000_000);
    assert_eq!(ahead.time(0, PERIOD), MAX_AHEAD_NS);
}
