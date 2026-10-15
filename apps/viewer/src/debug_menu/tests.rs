use super::*;
use monaka_core::debug_draw::{Categories, Category};
use std::time::Duration;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM;

fn pad_with(buttons: u16) -> Gamepad {
    Gamepad { buttons, ..Gamepad::default() }
}

/// Feeds `buttons` at `ms` after `start`; the pad the game gets and what was asked.
fn step(menu: &mut HeadsetMenu<'_>, buttons: u16, start: Instant, ms: u64) -> (Gamepad, Asked) {
    let (mut settings, status) = (Settings::default(), Status::default());
    let mut pad = pad_with(buttons);
    let head = Some(([0.0, 0.0, 0.0, 1.0], [0.0, 1.6, 0.0]));
    let asked = menu.pad(&mut pad, head, start + Duration::from_millis(ms), &mut settings, &status, true);
    (pad, asked)
}

#[test]
fn select_held_opens_the_menu_and_a_short_press_is_a_tap() {
    let mut menu = HeadsetMenu::new(DXGI_FORMAT_R8G8B8A8_UNORM);
    let start = Instant::now();
    let (pad, _) = step(&mut menu, button::BACK, start, 0);
    assert_eq!(pad.buttons, 0, "held back while it may be long");
    let (pad, asked) = step(&mut menu, button::BACK, start, 520);
    assert!(menu.open() && asked.buzzes.len() == 2);
    assert_eq!(pad, Gamepad::default(), "the game gets an idle pad while it is open");
    step(&mut menu, button::BACK, start, 1200);
    assert!(menu.open(), "held on: it does not close again");
    let (pad, _) = step(&mut menu, 0, start, 1300);
    assert_eq!(pad, Gamepad::default(), "let go after acting: no tap");
    // A short press, closed menu: a tap when let go.
    let mut closed = HeadsetMenu::new(DXGI_FORMAT_R8G8B8A8_UNORM);
    step(&mut closed, button::BACK, start, 0);
    let (pad, _) = step(&mut closed, 0, start, 150);
    assert_eq!(pad.buttons, button::BACK, "the tap");
    let (pad, _) = step(&mut closed, 0, start, 260);
    assert_eq!(pad.buttons, 0, "and over");
    assert!(!closed.open());
}

#[test]
fn start_is_left_alone_and_select_with_it_is_the_capture_chord() {
    let mut menu = HeadsetMenu::new(DXGI_FORMAT_R8G8B8A8_UNORM);
    let start = Instant::now();
    let (pad, _) = step(&mut menu, button::START, start, 0);
    assert_eq!(pad.buttons, button::START, "Start is not the debug menu's");
    step(&mut menu, button::START, start, 600);
    assert!(!menu.open());
    // Select with Start passes through, and Select neither acts nor taps afterwards.
    let mut both = HeadsetMenu::new(DXGI_FORMAT_R8G8B8A8_UNORM);
    let (pad, _) = step(&mut both, button::BACK | button::START, start, 0);
    assert_eq!(pad.buttons, button::BACK | button::START);
    step(&mut both, button::BACK, start, 900);
    assert!(!both.open());
    let (pad, _) = step(&mut both, 0, start, 950);
    assert_eq!(pad.buttons, 0, "no tap after the chord");
}

#[test]
fn an_open_menu_moves_switches_and_closes() {
    let mut menu = HeadsetMenu::new(DXGI_FORMAT_R8G8B8A8_UNORM);
    menu.menu.open = true;
    let mut settings = Settings::default();
    let status = Status { offered: Categories(u64::MAX), ..Status::default() };
    let now = Instant::now();
    menu.pad(&mut Gamepad { thumb_ly: -30000, ..Gamepad::default() }, None, now, &mut settings, &status, true);
    menu.pad(&mut Gamepad::default(), None, now + Duration::from_millis(50), &mut settings, &status, true);
    menu.pad(&mut pad_with(button::A), None, now + Duration::from_millis(100), &mut settings, &status, true);
    assert!(settings.wanted.has(Category::Skeleton), "down one line, then A");
    menu.pad(&mut pad_with(button::A), None, now + Duration::from_millis(150), &mut settings, &status, true);
    assert!(settings.wanted.has(Category::Skeleton), "A held: one press");
    menu.pad(&mut Gamepad { right_trigger: 255, ..Gamepad::default() }, None, now + Duration::from_millis(200), &mut settings, &status, true);
    assert_eq!(menu.menu.tab, 1);
    menu.pad(&mut pad_with(button::B), None, now + Duration::from_millis(250), &mut settings, &status, true);
    assert!(!menu.open());
}

#[test]
fn lb_rb_step_the_row_and_a_panel_driven_gets_the_presses() {
    let mut menu = HeadsetMenu::new(DXGI_FORMAT_R8G8B8A8_UNORM);
    let (mut settings, status) = (Settings::default(), Status::default());
    let now = Instant::now();
    menu.menu.open = true;
    let asked = menu.pad(&mut pad_with(button::RIGHT_SHOULDER), None, now, &mut settings, &status, true);
    assert_eq!(asked.focus_step, 1);
    let asked = menu.pad(&mut pad_with(button::RIGHT_SHOULDER), None, now + Duration::from_millis(50), &mut settings, &status, true);
    assert_eq!(asked.focus_step, 0, "held: no new step");
    let asked = menu.pad(&mut pad_with(button::RIGHT_SHOULDER | button::LEFT_SHOULDER), None, now + Duration::from_millis(75), &mut settings, &status, true);
    assert_eq!(asked.focus_step, -1, "LB: back a step");
    menu.pad(&mut Gamepad::default(), None, now + Duration::from_millis(100), &mut settings, &status, false);
    // A panel driven: the D-pad and A go to it, B still closes the menu.
    let asked = menu.pad(&mut pad_with(button::DPAD_DOWN | button::A), None, now + Duration::from_millis(150), &mut settings, &status, false);
    assert_eq!(asked.panel_presses, vec![Press::Down, Press::Switch]);
    let asked = menu.pad(&mut Gamepad { thumb_ry: 30000, ..Gamepad::default() }, None, now + Duration::from_millis(200), &mut settings, &status, false);
    assert_eq!(asked.scroll, SCROLL_LINES, "the right stick scrolls up");
    menu.pad(&mut pad_with(button::B), None, now + Duration::from_millis(250), &mut settings, &status, false);
    assert!(!menu.open(), "B closes whatever the pad drives");
}
