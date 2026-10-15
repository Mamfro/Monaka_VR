use super::*;
use crate::math::IDENTITY;
use std::f32::consts::{FRAC_1_SQRT_2, FRAC_PI_2};

const UP: Vec3 = [0.0, 1.0, 0.0];

#[test]
fn a_reload_is_a_touch_then_away_within_a_second_and_not_twice_at_once() {
    let rules = TouchAwayRules::RELOAD;
    let mut gesture = TouchAway::new();
    assert!(!gesture.feed(0.0, 0.5, &rules), "never touched");
    assert!(!gesture.feed(0.1, 0.1, &rules));
    assert!(!gesture.feed(0.2, 0.2, &rules), "between near and away");
    assert!(gesture.feed(0.3, 0.3, &rules));
    assert!(!gesture.feed(0.4, 0.1, &rules) && !gesture.feed(0.5, 0.3, &rules), "cooling down");
    assert!(!gesture.feed(2.0, 0.1, &rules) && !gesture.feed(3.5, 0.3, &rules), "too long after the touch");
    assert!(!gesture.feed(4.0, 0.1, &rules) && gesture.feed(4.2, 0.3, &rules));
}

#[test]
fn a_level_hand_is_unrolled_and_a_quarter_roll_is_a_quarter() {
    assert!(wrist_roll(IDENTITY, UP).unwrap().abs() < 1e-5);
    // A quarter turn about the pointing axis (-z), clockwise from behind: about +z by -90.
    let clockwise = [0.0, 0.0, -FRAC_1_SQRT_2, FRAC_1_SQRT_2];
    assert!((wrist_roll(clockwise, UP).unwrap() - FRAC_PI_2).abs() < 1e-4);
    // Pointing straight up: no reference.
    let up = [FRAC_1_SQRT_2, 0.0, 0.0, FRAC_1_SQRT_2];
    assert!(wrist_roll(up, UP).is_none());
    let mut twist = Twist::new();
    assert!(twist.begin(clockwise, UP));
    assert!((twist.since(IDENTITY, UP).unwrap() + FRAC_PI_2).abs() < 1e-4);
}

#[test]
fn a_pull_back_along_the_arrow_draws_its_share() {
    let rules = PullRules::BOW;
    let (bow, arrow) = ([0.0, 0.0, -0.6], [0.0, 0.0, -1.0]);
    assert!((Pull::back(bow, arrow, [0.0, 0.0, -0.35], &rules) - 0.25).abs() < 1e-6);
    assert_eq!(Pull::back(bow, arrow, [0.0, 0.0, 0.5], &rules), 0.7, "at most the longest");
    assert_eq!(Pull::back(bow, arrow, [0.0, 0.0, -0.8], &rules), 0.0, "ahead of the bow");
    assert_eq!(Pull::share(0.25, &rules), Some(0.5));
    assert_eq!(Pull::share(0.02, &rules), None);
}

#[test]
fn a_thrust_drives_along_the_blade_and_a_sideways_cut_does_not() {
    let rules = DirectedRules::THRUST;
    let blade = [0.0, 0.0, -1.0];
    let mut thrust = Trail::new(1.0);
    let mut cut = Trail::new(1.0);
    for i in 0..6 {
        let t = 0.02 * i as f64;
        thrust.push(t, [0.0, 0.0, -3.0 * t as f32], IDENTITY);
        cut.push(t, [3.0 * t as f32, 0.0, -0.5 * t as f32], IDENTITY);
    }
    let mut detector = Directed::new();
    assert!(detector.feed(0.1, &cut, blade, &rules).is_none());
    let motion = detector.feed(0.1, &thrust, blade, &rules).unwrap();
    assert!((motion.speed - 3.0).abs() < 1e-3);
    assert!(detector.feed(0.2, &thrust, blade, &rules).is_none(), "within the gap");
    assert_eq!(rules.share, 40f32.to_radians().cos(), "within 40 degrees of the blade");
}

#[test]
fn a_throw_leaves_with_its_peak_speed_times_the_gain_along_its_stroke() {
    let mut trail = Trail::new(1.0);
    for i in 0..20 {
        let t = 0.02 * i as f64;
        // 4 m/s forward (-z) for 0.2 s, then stopping.
        let z = -4.0 * (t.min(0.2)) as f32;
        trail.push(t, [0.0, 0.0, z], IDENTITY);
    }
    let release = Release::read(&trail, &ReleaseRules::THROW).unwrap();
    assert!(release.way[2] < -0.99, "{release:?}");
    let thrown = release.thrown(&ThrowSpeed::gain(1.6)).unwrap();
    assert!((thrown[2] + 4.0 * 1.6).abs() < 0.05, "{thrown:?}");
}

#[test]
fn a_throw_keeps_the_hands_speed_and_takes_its_gain() {
    let hands: [Vec3; 7] = [[0.0; 3], [0.01, 0.02, 0.0], [0.0, 0.0, -1.0], [3.0, 1.0, -4.0], [0.0, 0.0, -5.0], [10.0, -2.0, -30.0], [0.2, 0.3, 0.1]];
    let strokes = [None, Some([0.0, 0.0, -1.0]), Some([0.6, 0.0, -0.8])];
    for fastest in hands {
        for stroke in strokes {
            let release = Release::from_peak(fastest, stroke);
            assert!((length(release.velocity) - length(fastest)).abs() < 1e-4, "{fastest:?} {stroke:?}");
            // The hand's velocity times its gains, component by component.
            for gain in [1.6, 3.0] {
                assert_eq!(release.thrown(&ThrowSpeed::gain(gain)), Some(release.velocity.map(|c| c * gain)));
            }
        }
    }
}

#[test]
fn a_held_value_counts_after_its_hold_and_jitter_does_not() {
    let mut zone = Hold::new();
    assert_eq!(zone.feed(0.0, Some(1), 0.12), None);
    assert_eq!(zone.feed(0.05, Some(2), 0.12), None, "jitter restarts the hold");
    assert_eq!(zone.feed(0.1, Some(1), 0.12), None);
    assert_eq!(zone.feed(0.25, Some(1), 0.12), Some(1));
    assert_eq!(zone.held(), Some(1));
    assert_eq!(zone.feed(0.3, None, 0.12), None);
    assert_eq!(zone.held(), Some(1), "no reading keeps it");
    assert_eq!(zone.feed(0.4, Some(1), 0.12), None, "already held");
}

#[test]
fn ducking_crouches_past_the_band_and_slouching_sinks_the_standing_height() {
    let rules = DuckRules::CROUCH;
    let mut duck = Duck::new();
    assert!(!duck.feed(0.0, 1.7, &rules));
    assert!(!duck.feed(0.1, 1.4, &rules), "0.3 m down is within the band");
    assert!(duck.feed(0.2, 1.3, &rules));
    assert!(duck.feed(0.3, 1.45, &rules), "0.25 m down stays crouched");
    assert!(!duck.feed(0.4, 1.5, &rules));
    // A slow slouch: the standing height follows it down, never crouching.
    let mut slouch = Duck::new();
    let mut t = 0.0;
    let mut height = 1.7;
    while height > 1.2 {
        t += 0.05;
        height -= 0.001;
        assert!(!slouch.feed(t, height, &rules), "crouched at {height}");
    }
}

#[test]
fn a_hand_reaching_across_blends_by_how_far_it_crossed() {
    let (own, other) = ([0.0; 3], [1.0, 0.0, 0.0]);
    assert_eq!(reach_share(own, own, other, 0.3, 0.7), 0.0);
    assert_eq!(reach_share([0.5, 0.0, 0.0], own, other, 0.3, 0.7), 0.5);
    assert_eq!(reach_share(other, own, other, 0.3, 0.7), 1.0);
}
