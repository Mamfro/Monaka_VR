use super::*;

fn everything_offered() -> Status {
    Status { offered: Categories(u64::MAX), ..Status::default() }
}

#[test]
fn moving_wraps_and_tabs_keep_their_place() {
    let (mut menu, mut settings, status) = (Menu::default(), Settings::default(), everything_offered());
    menu.press(Press::Up, &mut settings, &status);
    assert_eq!(menu.selected[0], TABS[0].items.len() - 1, "up from the top: the last line");
    menu.press(Press::NextTab, &mut settings, &status);
    menu.press(Press::Down, &mut settings, &status);
    menu.press(Press::PreviousTab, &mut settings, &status);
    assert_eq!((menu.tab, menu.selected[0], menu.selected[1]), (0, TABS[0].items.len() - 1, 1));
    menu.press(Press::PreviousTab, &mut settings, &status);
    assert_eq!(menu.tab, PAGE_TAB - 1, "back past the first tab: the last (no page in this game)");
}

#[test]
fn a_switches_what_is_offered_only() {
    let (mut menu, mut settings) = (Menu::default(), Settings::default());
    menu.selected[0] = 1;
    assert_eq!(menu.item(), Item::Category(Category::Skeleton));
    let none = Status::default();
    menu.press(Press::Switch, &mut settings, &none);
    assert!(!settings.wanted.has(Category::Skeleton), "not offered by this game");
    menu.press(Press::Switch, &mut settings, &everything_offered());
    assert!(settings.wanted.has(Category::Skeleton));
    menu.press(Press::Switch, &mut settings, &everything_offered());
    assert!(!settings.wanted.has(Category::Skeleton));
}

#[test]
fn the_view_steps_both_ways_and_saving_needs_a_recording() {
    let (mut menu, mut settings, status) = (Menu { tab: 2, ..Menu::default() }, Settings::default(), everything_offered());
    menu.press(Press::Left, &mut settings, &status);
    assert_eq!(settings.view, View::GameBuffer);
    menu.press(Press::Right, &mut settings, &status);
    menu.press(Press::Right, &mut settings, &status);
    assert_eq!(settings.view, View::Depth);
    menu.tab = 3;
    assert_eq!(menu.press(Press::Switch, &mut settings, &status), Some(Command::SaveCapture));
    menu.selected[3] = 4;
    assert_eq!(menu.press(Press::Switch, &mut settings, &status), None, "nothing recorded");
    settings.recording = true;
    let recorded = Status { recorded: 12.0, ..status };
    assert_eq!(menu.press(Press::Switch, &mut settings, &recorded), Some(Command::SaveRecording));
    menu.press(Press::Close, &mut settings, &recorded);
    assert!(!menu.open);
}

#[test]
fn the_depth_pass_steps_round_the_passes_there_are() {
    let mut menu = Menu { tab: 2, selected: [0, 0, 3, 0, 0, 0], ..Menu::default() };
    let mut settings = Settings { view: View::Depth, ..Settings::default() };
    assert_eq!(settings.depth_pass, 1, "the first pass by default");
    let none = everything_offered();
    menu.press(Press::Right, &mut settings, &none);
    assert_eq!(settings.depth_pass, 1, "no shared depth: not available");
    let three = Status { depth_passes: 3, ..everything_offered() };
    menu.press(Press::Right, &mut settings, &three);
    menu.press(Press::Right, &mut settings, &three);
    assert_eq!(settings.depth_pass, 3);
    menu.press(Press::Right, &mut settings, &three);
    assert_eq!(settings.depth_pass, 0, "round to the present");
    menu.press(Press::Left, &mut settings, &three);
    assert_eq!(settings.depth_pass, 3);
}

#[test]
fn the_depth_buffer_steps_from_auto_through_the_list() {
    let mut menu = Menu { tab: 2, selected: [0, 0, 2, 0, 0, 0], ..Menu::default() };
    assert_eq!(menu.item(), Item::DepthBuffer);
    let mut settings = Settings { view: View::Depth, ..Settings::default() };
    let two = Status { depth: true, depth_buffer_count: 2, ..everything_offered() };
    menu.press(Press::Right, &mut settings, &two);
    assert_eq!(settings.depth_buffer, 1);
    menu.press(Press::Right, &mut settings, &two);
    menu.press(Press::Right, &mut settings, &two);
    assert_eq!(settings.depth_buffer, 0, "round to Auto");
    menu.press(Press::Left, &mut settings, &two);
    assert_eq!(settings.depth_buffer, 2);
}

#[test]
fn the_stick_presses_once_then_repeats_while_held() {
    let mut stick = StickRepeat::default();
    assert_eq!(stick.step(0.0, 0.9, 0.0), Some(Press::Up));
    assert_eq!(stick.step(0.0, 0.9, 0.2), None, "before the first repeat");
    assert_eq!(stick.step(0.0, 0.9, 0.41), Some(Press::Up));
    assert_eq!(stick.step(0.0, 0.9, 0.45), None);
    assert_eq!(stick.step(0.0, 0.9, 0.54), Some(Press::Up));
    assert_eq!(stick.step(0.0, 0.5, 0.6), None, "between push and centre: still held, nothing new");
    assert_eq!(stick.step(0.0, 0.0, 0.7), None);
    assert_eq!(stick.step(0.0, -0.9, 0.71), Some(Press::Down), "a fresh push presses at once");
    assert_eq!(stick.step(0.8, 0.0, 0.72), Some(Press::Right), "another direction presses at once");
}

#[test]
fn the_d_pad_presses_as_the_stick_pushed_all_the_way() {
    use monaka_contract::{gamepad::Gamepad, gamepad::button};
    let mut stick = StickRepeat::default();
    let pad = |buttons: u16| Gamepad { buttons, ..Gamepad::default() };
    assert_eq!(stick.step_pad(&pad(button::DPAD_DOWN), 0.0), Some(Press::Down));
    assert_eq!(stick.step_pad(&pad(button::DPAD_DOWN), 0.2), None, "held: repeats as the stick does");
    assert_eq!(stick.step_pad(&pad(button::DPAD_DOWN), 0.41), Some(Press::Down));
    assert_eq!(stick.step_pad(&pad(0), 0.5), None, "let go");
    assert_eq!(stick.step_pad(&pad(button::DPAD_LEFT), 0.6), Some(Press::Left));
    assert_eq!(stick.step_pad(&Gamepad { thumb_ly: 30000, ..Gamepad::default() }, 0.7), Some(Press::Up), "the stick still works");
}

fn with_page() -> Status {
    use monaka_contract::page::Row;
    let rows = [Row::heading("Camera"), Row::button(10, "Use", "VP +64"), Row::text("Eye", "(1, 2, 3)"), Row::button(11, "Turn test", "")];
    Status { page: Page::new("Explorer", &rows), ..everything_offered() }
}

#[test]
fn the_game_page_opens_first_once_and_its_buttons_answer() {
    let (mut menu, mut settings, status) = (Menu::default(), Settings::default(), with_page());
    menu.opened(&status);
    assert_eq!((menu.tab, menu.selected[PAGE_TAB]), (PAGE_TAB, 1), "on its first button");
    menu.press(Press::Down, &mut settings, &status);
    assert_eq!(menu.selected[PAGE_TAB], 3, "the text row between is skipped");
    assert_eq!(menu.press(Press::Switch, &mut settings, &status), Some(Command::Page { id: 11, press: page::Press::Switch }));
    menu.press(Press::Down, &mut settings, &status);
    assert_eq!(menu.press(Press::Right, &mut settings, &status), Some(Command::Page { id: 10, press: page::Press::Right }));
    menu.tab = 1;
    menu.opened(&status);
    assert_eq!(menu.tab, 1, "only the first opening goes to the page");
    let gone = everything_offered();
    menu.tab = PAGE_TAB;
    menu.opened(&gone);
    assert_eq!(menu.tab, 0, "a page gone is left");
}
