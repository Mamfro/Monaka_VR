use super::*;

fn run(chord: &mut Chord, buttons: u16, at: Instant) -> (u16, bool) {
    let mut pad = Gamepad { buttons: buttons | button::A, ..Default::default() };
    let fired = chord.step(&mut pad, at);
    (pad.buttons & !button::A, fired)
}

#[test]
fn both_held_fire_once_and_never_reach_the_game() {
    let t = Instant::now();
    let ms = |n: u64| t + Duration::from_millis(n);
    let mut chord = Chord::default();
    // Start a little before Back: held back, then both are ours.
    assert_eq!(run(&mut chord, button::START, ms(0)), (0, false));
    assert_eq!(run(&mut chord, KEYS, ms(40)), (0, false));
    assert_eq!(run(&mut chord, KEYS, ms(539)), (0, false));
    assert_eq!(run(&mut chord, KEYS, ms(540)), (0, true));
    assert_eq!(run(&mut chord, KEYS, ms(900)), (0, false), "once per hold");
    // Let go one at a time: the one still held stays ours.
    assert_eq!(run(&mut chord, button::BACK, ms(950)), (0, false));
    assert_eq!(run(&mut chord, 0, ms(1000)), (0, false));
    // A second hold fires again.
    run(&mut chord, KEYS, ms(1100));
    assert_eq!(run(&mut chord, KEYS, ms(1600)), (0, true));
}

#[test]
fn a_lone_press_reaches_the_game_after_the_grace_and_a_quick_one_as_a_tap() {
    let t = Instant::now();
    let ms = |n: u64| t + Duration::from_millis(n);
    let mut chord = Chord::default();
    // Held: nothing for the grace, then Start as long as it is held.
    assert_eq!(run(&mut chord, button::START, ms(0)), (0, false));
    assert_eq!(run(&mut chord, button::START, ms(100)), (0, false));
    assert_eq!(run(&mut chord, button::START, ms(120)), (button::START, false));
    assert_eq!(run(&mut chord, button::START, ms(400)), (button::START, false));
    assert_eq!(run(&mut chord, 0, ms(410)), (0, false));
    // Tapped: Back shows for the tap's length once let go.
    assert_eq!(run(&mut chord, button::BACK, ms(500)), (0, false));
    assert_eq!(run(&mut chord, 0, ms(550)), (button::BACK, false));
    assert_eq!(run(&mut chord, 0, ms(600)), (button::BACK, false));
    assert_eq!(run(&mut chord, 0, ms(640)), (0, false));
    // Other buttons are never touched (`run` checks A passes by masking it).
    let mut pad = Gamepad { buttons: button::A | button::X, ..Default::default() };
    chord.step(&mut pad, ms(700));
    assert_eq!(pad.buttons, button::A | button::X);
}

#[test]
fn a_menu_holding_the_pad_still_lets_the_chord_fire() {
    let t = Instant::now();
    let ms = |n: u64| t + Duration::from_millis(n);
    let mut chord = Chord::default();
    assert!(!chord.step_held(KEYS, ms(0)));
    assert!(chord.step_held(KEYS, ms(500)));
    assert!(!chord.step_held(KEYS, ms(800)), "once per hold");
}
