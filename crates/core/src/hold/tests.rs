use super::*;

fn read(toggle: &mut HoldToggle, buttons: u16, at: Instant) -> (u16, bool) {
    let mut pad = Gamepad { buttons, ..Default::default() };
    let toggled = toggle.toggled(&mut pad, at);
    (pad.buttons, toggled)
}

#[test]
fn a_hold_toggles_once_and_a_short_press_reaches_the_game_as_a_tap() {
    let t = Instant::now();
    let ms = |n: u64| t + Duration::from_millis(n);
    let mut start = HoldToggle::new(button::START);
    // Held: kept from the game, toggling once at the hold's length.
    assert_eq!(read(&mut start, button::START, ms(0)), (0, false));
    assert_eq!(read(&mut start, button::START, ms(499)), (0, false));
    assert_eq!(read(&mut start, button::START, ms(500)), (0, true));
    assert_eq!(read(&mut start, button::START, ms(900)), (0, false), "once per hold");
    assert_eq!(read(&mut start, 0, ms(950)), (0, false), "a hold that acted is no tap");
    // Pressed briefly: the game gets a tap once it is let go.
    assert_eq!(read(&mut start, button::START, ms(1000)), (0, false));
    assert_eq!(read(&mut start, 0, ms(1100)), (button::START, false));
    assert_eq!(read(&mut start, 0, ms(1100) + TAP), (0, false));
}

#[test]
fn start_and_back_together_are_the_chords_not_a_menus() {
    let t = Instant::now();
    let ms = |n: u64| t + Duration::from_millis(n);
    let mut start = HoldToggle::new(button::START);
    let both = button::START | button::BACK;
    assert_eq!(read(&mut start, both, ms(0)), (both, false), "the chord's to take");
    assert_eq!(read(&mut start, both, ms(800)), (both, false));
    // Let go of Back, Start still held: this hold has gone to the chord, so it neither toggles nor taps.
    assert_eq!(read(&mut start, button::START, ms(900)), (0, false));
    assert_eq!(read(&mut start, button::START, ms(2000)), (0, false));
    assert_eq!(read(&mut start, 0, ms(2100)), (0, false));
}
