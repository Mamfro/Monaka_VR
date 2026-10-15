use super::*;

/// 0.02 s steps: 0.1 m each along x (5 m/s), then slowing to 0.02 m each (1 m/s) while
/// drifting down along y.
fn throw() -> Trail {
    let mut trail = Trail::new(1.0);
    let (mut x, mut y) = (0.0, 0.0);
    for i in 0..20 {
        if i < 10 {
            x += 0.1;
        } else {
            x += 0.02;
            y -= 0.05;
        }
        trail.push(0.02 * i as f64, [x, y, 0.0], IDENTITY);
    }
    trail
}

#[test]
fn the_peak_is_the_fastest_stretch_and_the_stroke_its_way() {
    let trail = throw();
    let peak = trail.fastest(1.0, 0.06).unwrap();
    assert!((peak.velocity[0] - 5.0).abs() < 0.01 && peak.velocity[1].abs() < 0.01, "{peak:?}");
    let late = trail.fastest(0.1, 0.06).unwrap();
    assert!((late.velocity[0] - 1.0).abs() < 0.01, "only the slow end is recent: {late:?}");
    // Through the peak the hand went along x: the slow drift after it is not the throw.
    let way = trail.stroke(1.0, 0.06, 0.1, 0.04).unwrap();
    assert!(way[0] > 0.95, "{way:?}");
}

#[test]
fn one_update_read_twice_is_one_sample_and_old_samples_go() {
    let mut trail = Trail::new(0.5);
    assert!(trail.push(1.0, [0.0; 3], IDENTITY));
    assert!(!trail.push(1.001, [0.1, 0.0, 0.0], IDENTITY), "the same update");
    assert!(!trail.push(0.9, [0.0; 3], IDENTITY), "older than the latest");
    assert!(!trail.push(1.1, [f32::NAN, 0.0, 0.0], IDENTITY));
    assert!(trail.push(1.6, [0.0; 3], IDENTITY));
    assert_eq!(trail.len(), 1, "the sample at 1.0 is past the kept half second");
    assert!(trail.fresh(1.65, 0.1).is_some() && trail.fresh(1.8, 0.1).is_none());
}

#[test]
fn a_tip_follows_the_hand_s_turn() {
    // The tip half a metre ahead (-z) of a palm that turns a quarter about y (to face -x): the
    // palm stays put, the tip sweeps.
    let mut trail = Trail::of_point(1.0, [0.0, 0.0, -0.5]);
    let quarter = [0.0, std::f32::consts::FRAC_1_SQRT_2, 0.0, std::f32::consts::FRAC_1_SQRT_2];
    trail.push(0.0, [0.0; 3], IDENTITY);
    trail.push(0.1, [0.0; 3], quarter);
    let motion = trail.motion(0.1).unwrap();
    assert!((motion.moved[0] + 0.5).abs() < 1e-5 && (motion.moved[2] - 0.5).abs() < 1e-5, "{motion:?}");
    assert!((motion.speed - 0.5f32.hypot(0.5) / 0.1).abs() < 1e-3);
    assert!((motion.share_along([-1.0, 0.0, 0.0]) - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5);
}

#[test]
fn a_trail_no_longer_fed_empties_as_time_goes_on() {
    let mut trail = Trail::new(0.5);
    for i in 0..5 {
        assert!(trail.push_position(0.1 * i as f64, [0.1 * i as f32, 0.0, 0.0]));
    }
    let speed = trail.velocity_at(4, 0.06).unwrap();
    assert!((speed[0] - 1.0).abs() < 1e-4, "{speed:?}");
    assert!(trail.velocity_at(0, 0.06).is_none(), "nothing before the first");
    trail.expire(0.75);
    assert_eq!(trail.len(), 2, "the samples at 0.3 and 0.4 are within half a second");
    trail.expire(2.0);
    assert!(trail.is_empty() && trail.fastest(1.0, 0.06).is_none());
}

#[test]
fn where_the_hand_was_a_moment_ago() {
    let trail = throw();
    assert_eq!(trail.at(0.05).unwrap().time, 0.04);
    assert_eq!(trail.at(-1.0).unwrap().time, 0.0, "before the trail: its oldest");
    assert!(trail.motion(10.0).is_none(), "not that long yet");
}
