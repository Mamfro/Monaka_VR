use super::*;
use std::time::Duration;

fn all(_: Minutes) -> bool {
    true
}

#[test]
fn the_clock_shows_its_hour_and_fades_into_the_next() {
    let mut hour = TimeShown::default();
    let at = Instant::now();
    assert_eq!(hour.pick(all, 15, 16, 0.0, at), Some((15, 16, 0.0)));
    assert_eq!(hour.pick(all, 15, 16, 0.7, at), Some((15, 16, 0.7)));
    // The hour turns: no second fade back from the hour before.
    assert_eq!(hour.pick(all, 16, 17, 0.0, at), Some((16, 17, 0.0)));
}

#[test]
fn a_set_hour_fades_in_from_the_one_shown_to_the_end() {
    let mut hour = TimeShown::default();
    let at = Instant::now();
    hour.pick(all, 9, 10, 0.0, at);
    assert_eq!(hour.pick(all, 20, 21, 0.0, at), Some((9, 20, 0.0)));
    let (from, to, far) = hour.pick(all, 20, 21, 0.0, at + Duration::from_millis(300)).unwrap();
    assert_eq!((from, to), (9, 20));
    assert!((far - 0.6).abs() < 1e-3, "past half way it goes on: {far}");
    assert_eq!(hour.pick(all, 20, 21, 0.0, at + Duration::from_millis(400)).map(|(f, t, _)| (f, t)), Some((9, 20)));
    assert_eq!(hour.pick(all, 20, 21, 0.0, at + Duration::from_millis(600)), Some((20, 21, 0.0)));
}

#[test]
fn set_again_mid_fade_it_starts_from_the_hour_mostly_shown() {
    let mut hour = TimeShown::default();
    let at = Instant::now();
    hour.pick(all, 9, 10, 0.0, at);
    hour.pick(all, 20, 21, 0.0, at);
    hour.pick(all, 20, 21, 0.0, at + Duration::from_millis(400));
    assert_eq!(hour.pick(all, 5, 6, 0.0, at + Duration::from_millis(410)), Some((20, 5, 0.0)));
}

#[test]
fn the_hour_shown_stays_while_the_new_one_is_read() {
    let mut hour = TimeShown::default();
    let at = Instant::now();
    hour.pick(all, 9, 10, 0.0, at);
    assert_eq!(hour.pick(|h| h == 9, 20, 21, 0.0, at), Some((9, 9, 0.0)));
    // Read now: it fades in from there.
    assert_eq!(hour.pick(all, 20, 21, 0.0, at), Some((9, 20, 0.0)));
    assert_eq!(TimeShown::default().pick(|_| false, 20, 21, 0.0, at), None, "nothing read yet: nothing to draw");
}

fn close(a: Vec3, b: Vec3) -> bool {
    (0..3).all(|k| (a[k] - b[k]).abs() < 1e-4)
}

#[test]
fn a_flick_turns_once_until_the_stick_comes_back() {
    let mut stick = SnapStick::default();
    assert_eq!(stick.flick(0.5), None, "not far enough");
    assert_eq!(stick.flick(0.9), Some(SNAP));
    assert_eq!(stick.flick(1.0), None, "held: one turn");
    assert_eq!(stick.flick(0.5), None, "not back yet");
    assert_eq!(stick.flick(0.1), None);
    assert_eq!(stick.flick(-0.9), Some(-SNAP), "left");
}

#[test]
fn turning_right_shows_what_was_on_the_right_and_keeps_the_player_in_place() {
    let head = [0.3, 1.6, -0.2];
    let turned = Placement::default().turned(std::f32::consts::FRAC_PI_2, head);
    // Where the head is, the scene is as before: the player turns where they stand.
    assert!(close(turned.scene_point(head), head));
    // Straight ahead of the head now is what was on its right.
    let ahead = [head[0], head[1], head[2] - 1.0];
    assert!(close(turned.scene_point(ahead), [head[0] + 1.0, head[1], head[2]]));
    // Turning back undoes it, from wherever the head is by then.
    let back = turned.turned(-std::f32::consts::FRAC_PI_2, head);
    assert!(close(back.scene_point([1.0, 0.0, 2.0]), [1.0, 0.0, 2.0]));
}

#[test]
fn the_triggers_sweep_the_time_and_letting_go_settles_on_a_capture() {
    // Midnight, then every half hour from 5:00 to 20:00.
    let times: Vec<Minutes> = std::iter::once(0).chain((10..=40).map(|k| k * 30)).collect();
    let mut sweep = Sweep::default();
    let at = Instant::now();
    let base = 14.0 * 3600.0;
    assert_eq!(sweep.time(base, Some(14), &times, at), (base, false), "untouched: the setting's time");
    sweep.triggers(0, 255);
    sweep.time(base, Some(14), &times, at + Duration::from_millis(100));
    let (swept, blending) = sweep.time(base, Some(14), &times, at + Duration::from_millis(200));
    assert!(blending);
    assert!((swept - base - 2.0 * 0.1 * 1.5 * 3600.0).abs() < 1.0, "a full pull: 1.5 hours a second ({swept})");
    // Let go at 14:18: onto 14:30, the nearest capture, within a second or so, then left there.
    sweep.triggers(0, 0);
    let mut t = at + Duration::from_millis(200);
    let mut last = (0.0, true);
    for _ in 0..40 {
        t += Duration::from_millis(50);
        last = sweep.time(base, Some(14), &times, t);
    }
    assert_eq!(last, (14.5 * 3600.0, false));
    // LT goes back.
    sweep.triggers(255, 0);
    sweep.time(base, Some(14), &times, t + Duration::from_millis(100));
    assert!(sweep.time(base, Some(14), &times, t + Duration::from_millis(200)).0 < 14.5 * 3600.0);
    // The player sets another time: the sweep is let go.
    sweep.triggers(0, 0);
    assert_eq!(sweep.time(9.0 * 3600.0, Some(9), &times, t + Duration::from_millis(300)), (9.0 * 3600.0, false));
}
