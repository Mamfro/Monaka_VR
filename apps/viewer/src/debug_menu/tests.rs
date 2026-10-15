use super::*;
use monaka_contract::debug_draw::Categories;
use std::time::Duration;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM;

fn pad_with(buttons: u16) -> Gamepad {
    Gamepad { buttons, ..Gamepad::default() }
}

/// Feeds `buttons` at `ms` after `start`; the pad the game gets and what was asked.
fn step(menu: &mut HeadsetMenu<'_>, buttons: u16, start: Instant, ms: u64) -> (Gamepad, Asked) {
    let mut pad = pad_with(buttons);
    let head = Some(([0.0, 0.0, 0.0, 1.0], [0.0, 1.6, 0.0]));
    let asked = menu.pad(&mut pad, head, start + Duration::from_millis(ms), &Status::default());
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
    assert_eq!(pad.buttons, 0, "Select is not the game's while it opens the menu");
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
fn an_open_menu_leaves_the_triggers_and_sticks_to_the_pointer_and_b_closes_it() {
    let mut menu = HeadsetMenu::new(DXGI_FORMAT_R8G8B8A8_UNORM);
    menu.menu.open = true;
    let status = Status { offered: Categories(u64::MAX), ..Status::default() };
    let now = Instant::now();
    let mut pulled = Gamepad { right_trigger: 255, thumb_ry: 20000, ..Gamepad::default() };
    menu.pad(&mut pulled, None, now, &status);
    assert_eq!((pulled.right_trigger, pulled.thumb_ry, menu.menu.tab), (255, 20000, 0), "the pointer's, not a tab");
    menu.pad(&mut pad_with(button::B), None, now + Duration::from_millis(50), &status);
    assert!(!menu.open());
}

#[test]
fn lb_rb_slide_the_row() {
    let mut menu = HeadsetMenu::new(DXGI_FORMAT_R8G8B8A8_UNORM);
    let status = Status::default();
    let now = Instant::now();
    menu.menu.open = true;
    let asked = menu.pad(&mut pad_with(button::RIGHT_SHOULDER), None, now, &status);
    assert_eq!(asked.focus_step, 1);
    let asked = menu.pad(&mut pad_with(button::RIGHT_SHOULDER), None, now + Duration::from_millis(50), &status);
    assert_eq!(asked.focus_step, 0, "held: no new step");
    let asked = menu.pad(&mut pad_with(button::RIGHT_SHOULDER | button::LEFT_SHOULDER), None, now + Duration::from_millis(75), &status);
    assert_eq!(asked.focus_step, -1, "LB: back a step");
}
