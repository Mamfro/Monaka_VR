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
