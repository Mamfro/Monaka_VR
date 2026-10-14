use super::*;

#[test]
fn a_stretch_counts_frames_pairs_and_their_age() {
    let start = Instant::now();
    let mut meter = Meter::new("Headset", [2644, 2644], start);
    let at = |ms: u64| start + Duration::from_millis(ms);
    // Frames every 10 ms; pairs 1, 2, then 4 (3 never shown), one frame showing an old pair.
    meter.frame(at(0), Some((1, 1000)), 1010);
    meter.frame(at(10), Some((2, 1010)), 1030);
    meter.frame(at(20), None, 1040);
    meter.frame(at(40), Some((4, 1040)), 1050);
    meter.head(true);
    meter.hands([true, false]);
    let view = meter.view(at(250), None).clone();
    assert_eq!(view.display, "Headset");
    assert!((view.display_ms - 40.0 / 3.0).abs() < 0.01, "the mean of 10, 10 and 20 ms: {}", view.display_ms);
    assert!((view.worst_display_ms - 20.0).abs() < 0.01);
    assert!((view.pairs_per_second - 12.0).abs() < 0.01, "3 pairs in a quarter second");
    assert!((view.pair_age_ms - 40.0 / 3.0).abs() < 0.01, "ages 10, 20 and 10 ms");
    assert_eq!(view.worst_pair_age_ms, 20.0);
    assert!((view.repeated_per_second - 4.0).abs() < 0.01);
    assert!((view.skipped_per_second - 4.0).abs() < 0.01, "pair 3");
    assert!(view.poses.head_tracked && view.poses.controllers == [true, false]);
    assert!((view.poses.head - 4.0).abs() < 0.01);
}

#[test]
fn the_numbers_hold_until_a_stretch_ends_and_the_game_follows_its_producer() {
    let start = Instant::now();
    let mut meter = Meter::new("Window", [100, 100], start);
    meter.frame(start, Some((1, 0)), 0);
    assert_eq!(meter.view(start + Duration::from_millis(100), None).pairs_per_second, 0.0, "the first stretch is not over");
    let mut game = GameReport::EMPTY;
    game.tick = 5;
    game.fps = 60.0;
    game.set_text("Stereo method: AER (Alt-eye)");
    let view = meter.view(start + Duration::from_millis(300), Some(game)).clone();
    assert!(view.pairs_per_second > 0.0);
    assert_eq!(view.game.map(|g| (g.frames.map(|f| f[0]), g.settings)), Some((Some(60.0), "Stereo method: AER (Alt-eye)".to_owned())));
    let mut untimed = GameReport::EMPTY;
    untimed.set_text("Stereo method: AER (Alt-eye)");
    assert_eq!(meter.view(start + Duration::from_millis(305), Some(untimed)).game.as_ref().map(|g| g.frames), Some(None), "settings but no timing: a game with its own present loop");
    assert_eq!(meter.view(start + Duration::from_millis(310), Some(GameReport::EMPTY)).game, None, "a game side that never wrote says nothing");
}

#[test]
fn each_eyes_age_is_counted_against_the_pairs_newer_eye() {
    let start = Instant::now();
    let mut meter = Meter::new("Headset", [100, 100], start);
    // Alternate eyes: left one frame behind, then right one frame behind, then both together.
    meter.eyes([9, 10]);
    meter.eyes([11, 10]);
    meter.eyes([12, 12]);
    meter.eyes([12, 14]);
    let eyes = meter.view(start + Duration::from_millis(300), None).eyes.unwrap();
    assert_eq!(eyes.newest, [0.5, 0.75]);
    assert_eq!(eyes.age, [0.75, 0.25], "left: 1 + 0 + 0 + 2 behind over 4 pairs");
    assert_eq!(eyes.together, 0.25);
}

#[test]
fn the_games_frames_are_graphed_once_each() {
    let start = Instant::now();
    let mut meter = Meter::new("Headset", [100, 100], start);
    let mut game = GameReport::EMPTY;
    game.tick = 1;
    game.push_frames(&[11.0, 12.0]);
    assert_eq!(meter.view(start, Some(game)).game.as_ref().unwrap().times, [11.0, 12.0]);
    assert_eq!(meter.view(start, Some(game)).game.as_ref().unwrap().times, [11.0, 12.0], "read again: not added twice");
    game.push_frames(&[30.0]);
    assert_eq!(meter.view(start, Some(game)).game.as_ref().unwrap().times, [11.0, 12.0, 30.0]);
    // A new run counts from 0 again: its frames replace the old ones.
    let mut next = GameReport::EMPTY;
    next.tick = 1;
    next.push_frames(&[9.0]);
    assert_eq!(meter.view(start, Some(next)).game.as_ref().unwrap().times, [9.0]);
}
