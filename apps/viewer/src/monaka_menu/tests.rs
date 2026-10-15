use super::*;
use monaka_core::hold::{HOLD, TAP};
use std::time::Duration;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM;

fn menu(live: &str) -> MonakaMenu<'static> {
    MonakaMenu::new(DXGI_FORMAT_R8G8B8A8_UNORM, live)
}

/// The pad the game gets for `buttons` at `at`, and what the menu asked for.
fn read(menu: &mut MonakaMenu<'_>, buttons: u16, at: Instant, busy: bool) -> (u16, Asked) {
    let mut pad = Gamepad { buttons, ..Default::default() };
    let asked = menu.pad(&mut pad, None, at, busy);
    (pad.buttons, asked)
}

#[test]
fn start_held_opens_the_menu_which_takes_the_pad_until_held_again() {
    let t = Instant::now();
    let mut m = menu("controller_proxy=off");
    assert!(read(&mut m, button::START, t, false).0 == 0, "kept from the game while it may be a hold");
    let (_, opened) = read(&mut m, button::START, t + HOLD, false);
    assert!(m.menu.open && !opened.buzzes.is_empty(), "opened, with a buzz");
    read(&mut m, 0, t + HOLD + Duration::from_millis(50), false);
    assert_eq!(read(&mut m, button::X, t + HOLD + Duration::from_millis(100), false).0, 0, "the game gets an idle pad while it is open");
    // A changes the selected line; the change is asked for.
    let (_, switched) = read(&mut m, button::A, t + HOLD + Duration::from_millis(150), false);
    assert_eq!(switched.changed.map(|(id, _)| id), Some("controller_proxy"));
    read(&mut m, 0, t + HOLD + Duration::from_millis(200), false);
    // Held again: closed, and the game has its pad back.
    let again = t + Duration::from_secs(2);
    read(&mut m, button::START, again, false);
    read(&mut m, button::START, again + HOLD, false);
    assert!(!m.menu.open);
    assert_eq!(read(&mut m, button::X, again + HOLD + Duration::from_millis(50), false).0, button::X);
}

#[test]
fn a_short_start_reaches_the_game_as_a_tap() {
    let t = Instant::now();
    let mut m = menu("controller_proxy=off");
    read(&mut m, button::START, t, false);
    let let_go = t + Duration::from_millis(150);
    assert_eq!(read(&mut m, 0, let_go, false).0, button::START, "the tap, once let go");
    assert_eq!(read(&mut m, 0, let_go + TAP, false).0, 0);
    assert!(!m.menu.open);
}

#[test]
fn the_menu_leaves_the_chord_and_a_busy_pad_alone() {
    let t = Instant::now();
    let mut m = menu("controller_proxy=off");
    let both = button::START | button::BACK;
    read(&mut m, both, t, false);
    assert_eq!(read(&mut m, both, t + HOLD * 2, false).0, both, "Start with Back is the capture chord's");
    assert!(!m.menu.open);
    // The debug menu has the pad: Start passes untouched.
    let later = t + Duration::from_secs(5);
    assert_eq!(read(&mut m, button::START, later, true).0, button::START);
    assert_eq!(read(&mut m, button::START, later + HOLD, true).0, button::START);
    assert!(!m.menu.open);
    // Nothing on offer: no menu at all.
    let mut empty = menu("");
    assert_eq!(read(&mut empty, button::START, t, false).0, button::START);
    assert_eq!(read(&mut empty, button::START, t + HOLD, false).0, button::START);
}
