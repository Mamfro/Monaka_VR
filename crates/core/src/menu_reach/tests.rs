use super::*;

/// Level, at 1.7 m, facing -Z.
const HEAD: (Quat, Vec3) = ([0.0, 0.0, 0.0, 1.0], [0.0, 1.7, 0.0]);
const SHOULDER_L: Vec3 = [-0.2, 1.55, 0.25];
const SHOULDER_R: Vec3 = [0.2, 1.55, 0.25];
const FRONT_L: Vec3 = [-0.2, 1.3, -0.35];
const FRONT_R: Vec3 = [0.2, 1.3, -0.35];

fn ms(t: Instant, n: u64) -> Instant {
    t + Duration::from_millis(n)
}

/// One frame with the left hand at `left`, the right at `right` and the sticks as given.
fn frame(reach: &mut Reach, at: Instant, left: Vec3, right: Vec3, lstick: (i16, i16), rstick: (i16, i16)) -> (Gamepad, Vec<Buzz>) {
    let mut pad = Gamepad { thumb_lx: lstick.0, thumb_ly: lstick.1, thumb_rx: rstick.0, thumb_ry: rstick.1, ..Default::default() };
    let buzz = reach.update(&mut pad, Some(HEAD), [Some(left), Some(right)], at);
    (pad, buzz)
}

const MAX: i16 = i16::MAX;

#[test]
fn the_zone_is_over_the_shoulder_and_turns_with_the_head() {
    let (q, eyes) = HEAD;
    assert!(in_zone(q, eyes, SHOULDER_L, DEPTH) && in_zone(q, eyes, SHOULDER_R, DEPTH));
    assert!(!in_zone(q, eyes, FRONT_L, DEPTH), "in front");
    assert!(!in_zone(q, eyes, [-0.2, 1.0, 0.25], DEPTH), "behind the back, too low");
    // Only just behind the head: not far enough by default, enough for a shallower reach.
    let shallow = [-0.2, 1.55, 0.08];
    assert!(!in_zone(q, eyes, shallow, DEPTH) && in_zone(q, eyes, shallow, 0.05));
    // Once open, a hand drifting a little forward keeps it open; well forward closes it.
    assert!(!out_of_zone(q, eyes, [-0.2, 1.55, 0.10], DEPTH) && out_of_zone(q, eyes, [-0.2, 1.55, 0.0], DEPTH));
    // Turned to face +X (yaw -90 degrees): behind is now -X.
    let s = std::f32::consts::FRAC_1_SQRT_2;
    let turned = [0.0, -s, 0.0, s];
    let right_back = [0.3, 1.55, 0.25];
    assert!(in_zone(q, eyes, right_back, DEPTH), "behind the unturned head");
    assert!(in_zone(turned, eyes, [-0.3, 1.55, 0.0], DEPTH) && !in_zone(turned, eyes, right_back, DEPTH), "in front of the turned one");
}

#[test]
fn resting_over_the_shoulder_opens_and_a_flick_picks() {
    let t = Instant::now();
    let mut reach = Reach::new(Pick::Same, button::DPAD_LEFT | button::DPAD_RIGHT, DEPTH);
    // Arrives (still moving: nothing), comes to rest, and opens 150 ms later with a buzz on the left.
    frame(&mut reach, t, FRONT_L, FRONT_R, (0, 0), (0, 0));
    assert_eq!(frame(&mut reach, ms(t, 500), SHOULDER_L, FRONT_R, (0, 0), (0, 0)).0.buttons, 0, "moving");
    assert_eq!(frame(&mut reach, ms(t, 600), SHOULDER_L, FRONT_R, (0, 0), (0, 0)).0.buttons, 0, "resting");
    assert_eq!(frame(&mut reach, ms(t, 700), SHOULDER_L, FRONT_R, (0, 0), (0, 0)).0.buttons, 0, "not yet");
    let (pad, buzz) = frame(&mut reach, ms(t, 760), SHOULDER_L, FRONT_R, (0, 0), (0, 0));
    assert_eq!((pad.buttons, buzz), (button::DPAD_LEFT, vec![(0, OPENED, BUZZ_FOR)]));
    // The left stick (same hand) flicked up: Y, the stick taken from the game; the right stick
    // still turns.
    let (pad, buzz) = frame(&mut reach, ms(t, 800), SHOULDER_L, FRONT_R, (0, MAX), (MAX, 0));
    assert_eq!((pad.buttons, pad.thumb_ly, pad.thumb_rx), (button::DPAD_LEFT | button::Y, 0, MAX));
    assert_eq!(buzz, vec![(0, PICKED, BUZZ_FOR)]);
    // Back to the centre: Y let go, the menu still open; then right: B.
    assert_eq!(frame(&mut reach, ms(t, 850), SHOULDER_L, FRONT_R, (0, 0), (0, 0)).0.buttons, button::DPAD_LEFT);
    assert_eq!(frame(&mut reach, ms(t, 900), SHOULDER_L, FRONT_R, (MAX, 0), (0, 0)).0.buttons, button::DPAD_LEFT | button::B);
    // The hand comes forward: everything let go.
    assert_eq!(frame(&mut reach, ms(t, 950), FRONT_L, FRONT_R, (0, 0), (0, 0)).0.buttons, 0);
}

#[test]
fn with_the_stick_a_flick_reaches_the_game_as_its_right_stick() {
    let t = Instant::now();
    let mut reach = Reach::new(Pick::Same, button::DPAD_LEFT | button::DPAD_RIGHT, DEPTH).picking_with(PickWith::Stick);
    // The left hand rests over the left shoulder: its menu opens.
    frame(&mut reach, t, SHOULDER_L, FRONT_R, (0, 0), (0, 0));
    frame(&mut reach, ms(t, 200), SHOULDER_L, FRONT_R, (0, 0), (0, 0));
    // The left stick flicked up: the game's right stick, up; no face button, no walking, and
    // the right stick's own push does not reach the game.
    let (pad, _) = frame(&mut reach, ms(t, 250), SHOULDER_L, FRONT_R, (0, MAX), (MAX, 0));
    assert_eq!((pad.buttons, pad.thumb_lx, pad.thumb_ly, pad.thumb_rx, pad.thumb_ry), (button::DPAD_LEFT, 0, 0, 0, MAX));
    // Back at the centre: nothing on the right stick.
    let (pad, _) = frame(&mut reach, ms(t, 300), SHOULDER_L, FRONT_R, (0, 0), (MAX, 0));
    assert_eq!((pad.thumb_rx, pad.thumb_ry), (0, 0));
    assert_eq!(parse_with("stick"), Ok(PickWith::Stick));
    // The right hand's menu, walking forward: the right stick picks, the left reaches the game
    // centred (its wheel follows it too).
    let mut reach = Reach::new(Pick::Same, button::DPAD_LEFT | button::DPAD_RIGHT, DEPTH).picking_with(PickWith::Stick);
    frame(&mut reach, t, FRONT_L, SHOULDER_R, (0, MAX), (0, 0));
    let (pad, _) = frame(&mut reach, ms(t, 200), FRONT_L, SHOULDER_R, (0, MAX), (0, 0));
    assert_eq!((pad.buttons, pad.thumb_lx, pad.thumb_ly), (button::DPAD_RIGHT, 0, 0), "open: no walking into the wheel");
    let (pad, _) = frame(&mut reach, ms(t, 250), FRONT_L, SHOULDER_R, (0, MAX), (0, MAX));
    assert_eq!((pad.thumb_ly, pad.thumb_ry), (0, MAX), "the right stick's flick picks");
    // The hand comes down: walking again.
    let (pad, _) = frame(&mut reach, ms(t, 300), FRONT_L, FRONT_R, (0, MAX), (0, 0));
    assert_eq!((pad.buttons, pad.thumb_ly), (0, MAX));
}

#[test]
fn a_swing_through_the_zone_opens_nothing() {
    let t = Instant::now();
    let mut reach = Reach::new(Pick::Same, button::DPAD_LEFT | button::DPAD_RIGHT, DEPTH);
    // The right hand winds up fast over the shoulder and pauses there briefly before the swing.
    frame(&mut reach, t, FRONT_L, FRONT_R, (0, 0), (0, 0));
    frame(&mut reach, ms(t, 100), FRONT_L, SHOULDER_R, (0, 0), (0, 0));
    for n in [150, 200, 250, 300, 350, 400, 450] {
        assert_eq!(frame(&mut reach, ms(t, n), FRONT_L, SHOULDER_R, (0, 0), (0, 0)).0.buttons, 0, "{n} ms");
    }
    // Resting on after the grace: it opens (d-pad right, the weapons).
    frame(&mut reach, ms(t, 520), FRONT_L, SHOULDER_R, (0, 0), (0, 0));
    assert_eq!(frame(&mut reach, ms(t, 700), FRONT_L, SHOULDER_R, (0, 0), (0, 0)).0.buttons, button::DPAD_RIGHT);
}

#[test]
fn a_stick_pushed_before_the_menu_opened_picks_nothing_until_centred() {
    let t = Instant::now();
    let mut reach = Reach::new(Pick::Other, button::DPAD_LEFT | button::DPAD_RIGHT, DEPTH);
    // Running (left stick forward) while the right hand reaches back: the other hand's stick,
    // the left, picks for the right hand's menu.
    frame(&mut reach, t, FRONT_L, SHOULDER_R, (0, MAX), (0, 0));
    frame(&mut reach, ms(t, 200), FRONT_L, SHOULDER_R, (0, MAX), (0, 0));
    let (pad, _) = frame(&mut reach, ms(t, 250), FRONT_L, SHOULDER_R, (0, MAX), (0, 0));
    assert_eq!((pad.buttons, pad.thumb_ly), (button::DPAD_RIGHT, 0), "open; no pick, no running");
    frame(&mut reach, ms(t, 300), FRONT_L, SHOULDER_R, (0, 0), (0, 0));
    assert_eq!(frame(&mut reach, ms(t, 350), FRONT_L, SHOULDER_R, (0, -MAX), (0, 0)).0.buttons, button::DPAD_RIGHT | button::A);
}

#[test]
fn one_menu_at_a_time_and_none_for_a_direction_not_held() {
    let t = Instant::now();
    let mut reach = Reach::new(Pick::Same, button::DPAD_LEFT, DEPTH);
    assert!(reach.active());
    // The right hand has no menu here (d-pad right is not held).
    frame(&mut reach, t, FRONT_L, SHOULDER_R, (0, 0), (0, 0));
    assert_eq!(frame(&mut reach, ms(t, 400), FRONT_L, SHOULDER_R, (0, 0), (0, 0)).0.buttons, 0);
    assert!(!Reach::new(Pick::Same, button::DPAD_UP, DEPTH).active());
    assert_eq!(parse("other"), Ok(Some(Pick::Other)));
    assert_eq!(parse("off"), Ok(None));
    assert!(parse("left").is_err());
}

#[test]
fn a_lost_controller_closes_its_menu_after_a_while() {
    let t = Instant::now();
    let mut reach = Reach::new(Pick::Same, button::DPAD_LEFT | button::DPAD_RIGHT, DEPTH);
    frame(&mut reach, t, SHOULDER_L, FRONT_R, (0, 0), (0, 0));
    assert_eq!(frame(&mut reach, ms(t, 200), SHOULDER_L, FRONT_R, (0, 0), (0, 0)).0.buttons, button::DPAD_LEFT);
    let lost = |reach: &mut Reach, n: u64| {
        let mut pad = Gamepad::default();
        reach.update(&mut pad, Some(HEAD), [None, Some(FRONT_R)], ms(t, n));
        pad.buttons
    };
    assert_eq!(lost(&mut reach, 700), button::DPAD_LEFT, "waits");
    assert_eq!(lost(&mut reach, 1300), 0, "then closes");
}

#[test]
fn a_glance_over_a_shoulder_hardly_turns_the_body_but_a_turn_is_followed() {
    let mut reach = Reach::new(Pick::Same, button::DPAD_LEFT | button::DPAD_RIGHT, DEPTH);
    let t = Instant::now();
    let ahead: Quat = [0.0, 0.0, 0.0, 1.0];
    // The head turned 90 degrees left (to face -X).
    let half = std::f32::consts::FRAC_PI_4;
    let left: Quat = [0.0, half.sin(), 0.0, half.cos()];
    let yaw = |q: Quat| {
        let f = math::forward_axis(q);
        (-f[0]).atan2(-f[2]).to_degrees()
    };
    reach.body_facing(ahead, t);
    let glanced = reach.body_facing(left, t + Duration::from_millis(100));
    assert!(yaw(glanced).abs() < 12.0, "a 100 ms glance: {}", yaw(glanced));
    let mut turned = glanced;
    for k in 2..=30 {
        turned = reach.body_facing(left, t + Duration::from_millis(100 * k));
    }
    assert!(yaw(turned) > 80.0, "held for three seconds: {}", yaw(turned));
}
