use super::*;
use crate::math::IDENTITY;

/// Feeds a hand pointing ahead (-Z) moving sideways at `speed` m/s (at x = speed * time) for
/// `seconds`, at 90 Hz from `start`, returning the steps.
fn sweep(swing: &mut Swing, start: f64, seconds: f64, speed: f32, rules: &SwingRules) -> Vec<Step> {
    let frames = (seconds * 90.0) as usize;
    (0..frames)
        .map(|i| {
            let t = start + i as f64 / 90.0;
            let x = speed * t as f32;
            swing.feed(t, [x, 1.2, -0.4], IDENTITY, rules)
        })
        .collect()
}

#[test]
fn a_fast_sweep_is_one_swing() {
    let rules = SwingRules::WEAPON;
    let mut swing = Swing::new();
    let steps = sweep(&mut swing, 10.0, 0.3, 5.0, &rules);
    let starts: Vec<_> = steps.iter().filter_map(|s| s.started).collect();
    assert_eq!(starts.len(), 1, "one swing however long it lasts");
    assert!((starts[0].speed - 5.0).abs() < 0.2, "{starts:?}");
    assert!(starts[0].direction[0] > 0.99, "it went right: {starts:?}");
    // It started within a few frames.
    let at = steps.iter().position(|s| s.started.is_some()).unwrap();
    assert!(at <= 5, "started at frame {at}");
}

#[test]
fn a_swing_lasts_until_the_tip_slows() {
    let rules = SwingRules::WEAPON;
    let mut swing = Swing::new();
    sweep(&mut swing, 10.0, 0.3, 5.0, &rules);
    assert!(swing.swinging(), "still moving fast");
    // Held still where the sweep ended.
    let end = 5.0 * (10.0 + 26.0 / 90.0) as f32;
    for i in 0..10 {
        swing.feed(10.3 + i as f64 / 90.0, [end, 1.2, -0.4], IDENTITY, &rules);
    }
    assert!(!swing.swinging(), "slowed below the settle speed");
}

#[test]
fn slow_moves_are_not_swings() {
    let rules = SwingRules::WEAPON;
    let mut swing = Swing::new();
    let steps = sweep(&mut swing, 0.0, 1.0, 1.0, &rules);
    assert!(steps.iter().all(|s| s.started.is_none()));
}

#[test]
fn the_next_swing_waits_for_the_tip_to_settle() {
    let rules = SwingRules::WEAPON;
    let mut swing = Swing::new();
    sweep(&mut swing, 0.0, 0.3, 5.0, &rules);
    // Straight on at the same speed: still the same swing.
    let more = sweep(&mut swing, 0.3, 0.3, 5.0, &rules);
    assert!(more.iter().all(|s| s.started.is_none()));
    // Held still, then swung again.
    let rest: Vec<_> = (0..20).map(|i| swing.feed(0.6 + i as f64 / 90.0, [3.0, 1.2, -0.4], IDENTITY, &rules)).collect();
    let settled: Vec<_> = rest.iter().filter_map(|s| s.settled).collect();
    assert_eq!(settled.len(), 1, "{settled:?}");
    assert!(settled[0].1 && (settled[0].0 - 5.0).abs() < 0.5, "{settled:?}");
    let again = sweep(&mut swing, 2.0, 0.3, 5.0, &rules);
    assert_eq!(again.iter().filter(|s| s.started.is_some()).count(), 1);
}

#[test]
fn a_wind_up_is_not_a_swing_but_the_chop_after_it_is() {
    let rules = SwingRules::WEAPON;
    let mut swing = Swing::new();
    // Up at 4 m/s for 0.2 s, then straight down at 6 m/s without stopping.
    let mut starts = Vec::new();
    for i in 0..40 {
        let t = i as f64 / 90.0;
        let y = if t < 0.2 { 1.0 + 4.0 * t as f32 } else { 1.8 - 6.0 * (t as f32 - 0.2) };
        if let Some(started) = swing.feed(t, [0.2, y, -0.4], IDENTITY, &rules).started {
            starts.push((t, started));
        }
    }
    assert_eq!(starts.len(), 1, "{starts:?}");
    assert!(starts[0].0 > 0.2 && starts[0].1.direction[1] < -0.9, "the chop, on its way down: {starts:?}");
}

/// A blade held with `pointer`'s orientation (identity: pointing ahead, -Z, edges up and down)
/// moving at 5 m/s along `direction` for 0.3 s, then held still: whether it swung, and
/// whether the motion was taken as not cutting.
fn blade_sweep(direction: Vec3, pointer: Quat) -> (bool, bool) {
    let rules = SwingRules::BLADE;
    let at = |t: f32| [0.2 + direction[0] * 5.0 * t, 1.2 + direction[1] * 5.0 * t, -0.4 + direction[2] * 5.0 * t];
    let mut swing = Swing::new();
    let (mut started, mut uncut) = (false, false);
    for i in 0..37 {
        let t = i as f32 / 90.0;
        let step = swing.feed(f64::from(t), at(t.min(0.3)), pointer, &rules);
        started |= step.started.is_some();
        uncut |= step.uncut;
    }
    (started, uncut)
}

#[test]
fn a_blade_cuts_along_its_edge_only() {
    // A chop straight down: along the edge.
    assert_eq!(blade_sweep([0.0, -1.0, 0.0], IDENTITY), (true, false));
    // Sideways with the blade upright: the flat.
    assert_eq!(blade_sweep([1.0, 0.0, 0.0], IDENTITY), (false, true));
    // A thrust ahead: the point.
    assert_eq!(blade_sweep([0.0, 0.0, -1.0], IDENTITY), (false, true));
    // Sideways with the wrist turned a quarter (rolled about where it points): a slash.
    let rolled = [0.0, 0.0, std::f32::consts::FRAC_PI_4.sin(), std::f32::consts::FRAC_PI_4.cos()];
    assert_eq!(blade_sweep([1.0, 0.0, 0.0], rolled), (true, false));
    // A blunt weapon: any way.
    let mut swing = Swing::new();
    let blunt = (0..27).any(|i| {
        let t = i as f64 / 90.0;
        swing.feed(t, [0.2 + 5.0 * t as f32, 1.2, -0.4], IDENTITY, &SwingRules::WEAPON).started.is_some()
    });
    assert!(blunt);
}

#[test]
fn a_blade_pointing_up_cuts_across_itself() {
    // The blade up and ahead from the fist at 45 degrees: a flick along it (up and ahead) is
    // a thrust; across it (up and back, or down and ahead) a cut.
    let up_ahead = [0.0, std::f32::consts::FRAC_1_SQRT_2, -std::f32::consts::FRAC_1_SQRT_2];
    let rules = SwingRules::BLADE.pointing(up_ahead);
    let run = |direction: Vec3| {
        let mut swing = Swing::new();
        (0..27).any(|i| {
            let t = i as f32 / 90.0;
            let p = [0.2 + direction[0] * 5.0 * t, 1.2 + direction[1] * 5.0 * t, -0.4 + direction[2] * 5.0 * t];
            swing.feed(f64::from(t), p, IDENTITY, &rules).started.is_some()
        })
    };
    let s = std::f32::consts::FRAC_1_SQRT_2;
    assert!(!run([0.0, s * 0.3, -1.0]), "along the blade, a little up: a thrust");
    assert!(run([0.0, -s, -s]), "down and ahead, across the blade: a cut");
    assert!(!run([1.0, 0.0, 0.0]), "sideways: the flat");
}

#[test]
fn a_tracking_jump_is_not_a_swing() {
    let rules = SwingRules::WEAPON;
    let mut swing = Swing::new();
    sweep(&mut swing, 0.0, 0.2, 0.0, &rules);
    // The controller reappears a metre away, then holds still.
    let steps: Vec<_> = (0..20).map(|i| swing.feed(0.2 + i as f64 / 90.0, [1.0, 1.2, -0.4], IDENTITY, &rules)).collect();
    assert!(steps.iter().all(|s| s.started.is_none()));
}

#[test]
fn a_turning_wrist_swings_the_tip() {
    // The palm stays put while the hand turns 90 degrees in 0.15 s: the tip half a metre out
    // moves about 5 m/s.
    let rules = SwingRules::WEAPON;
    let mut swing = Swing::new();
    let started = (0..14).any(|i| {
        let angle = (i as f32 / 13.0) * std::f32::consts::FRAC_PI_2;
        let pointer = crate::math::yaw_rotation(-angle);
        swing.feed(i as f64 / 90.0, [0.2, 1.2, -0.3], pointer, &rules).started.is_some()
    });
    assert!(started);
}
