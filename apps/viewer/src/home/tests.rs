use super::*;
use monaka_contract::protocol::LEFT_HAND;
use monaka_overlay::egui::Pos2;
use monaka_core::hold::HOLD;
use monaka_core::math;
use monaka_overlay::home::Page;
use monaka_overlay::home::place::TURN_SECONDS;
use std::time::Duration;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM;

const HEAD: (Quat, Vec3) = (math::IDENTITY, [0.0, 1.6, 0.0]);

fn home() -> HomeView<'static> {
    HomeView::new(DXGI_FORMAT_R8G8B8A8_UNORM, None, true)
}

/// A right hand at the head pointing straight ahead, and an untracked left one.
fn ahead() -> [HandPose; 2] {
    aimed(0.0)
}

/// The right hand turned `yaw` radians left of straight ahead.
fn aimed(yaw: f32) -> [HandPose; 2] {
    [HandPose::default(), HandPose { orientation: math::yaw_rotation(yaw), position: [0.0, 1.5, -0.2], valid: 1, received_tick: 1 }]
}

/// The pointer presses and releases among panel `page`'s events: true for a press, false for a release,
/// with where.
fn buttons(h: &HomeView<'_>, page: usize) -> Vec<(bool, Pos2)> {
    h.events[page]
        .iter()
        .filter_map(|e| match e {
            egui::Event::PointerButton { pressed, pos, .. } => Some((*pressed, *pos)),
            _ => None,
        })
        .collect()
}

/// Opens `h` with Start held from `t`; the time after.
fn open(h: &mut HomeView<'_>, t: Instant) -> Instant {
    let mut pad = Gamepad { buttons: button::START, ..Default::default() };
    h.pad(&mut pad, Some(HEAD), &ahead(), t, false);
    let mut pad = Gamepad { buttons: button::START, ..Default::default() };
    let asked = h.pad(&mut pad, Some(HEAD), &ahead(), t + HOLD, false);
    assert!(h.is_open() && !asked.buzzes.is_empty(), "opened, with a buzz");
    let mut pad = Gamepad::default();
    h.pad(&mut pad, Some(HEAD), &ahead(), t + HOLD + Duration::from_millis(20), false);
    t + HOLD + Duration::from_millis(40)
}

#[test]
fn start_held_opens_the_home_which_takes_the_pad() {
    let mut h = home();
    let t = open(&mut h, Instant::now());
    let mut pad = Gamepad { buttons: button::X, ..Default::default() };
    h.pad(&mut pad, Some(HEAD), &ahead(), t, false);
    assert_eq!(pad, Gamepad::default(), "the game gets an idle pad while it is open");
    // Another menu holding the pad leaves it alone.
    let mut busy = home();
    let mut pad = Gamepad { buttons: button::START, ..Default::default() };
    busy.pad(&mut pad, Some(HEAD), &ahead(), t, true);
    busy.pad(&mut pad, Some(HEAD), &ahead(), t + HOLD, true);
    assert!(!busy.is_open());
}

#[test]
fn the_pointer_lands_on_the_panel_in_front_and_clicks_it() {
    let mut h = home();
    let t = open(&mut h, Instant::now());
    let front = h.home.front;
    assert_eq!(PAGES[front], Page::Settings);
    assert!(matches!(h.pointer.at(), Some((page, _)) if page == front), "pointing ahead: the front panel");
    assert!(h.events[front].iter().any(|e| matches!(e, egui::Event::PointerMoved(_))));
    // The trigger pulled and let go: a click there, with a buzz.
    let mut pulled = Gamepad { right_trigger: 255, ..Default::default() };
    let asked = h.pad(&mut pulled, Some(HEAD), &ahead(), t, false);
    assert!(!asked.buzzes.is_empty());
    let mut let_go = Gamepad::default();
    h.pad(&mut let_go, Some(HEAD), &ahead(), t + Duration::from_millis(20), false);
    assert_eq!(buttons(&h, front).iter().map(|(pressed, _)| *pressed).collect::<Vec<_>>(), [true, false]);
}

#[test]
fn a_hand_that_trembles_while_pulling_still_clicks() {
    let mut h = home();
    let t = open(&mut h, Instant::now());
    let front = h.home.front;
    let mut pulled = Gamepad { right_trigger: 255, ..Default::default() };
    h.pad(&mut pulled, Some(HEAD), &aimed(0.0), t, false);
    // The hand drifts a little while the trigger is held, then lets go: one place for both.
    let mut held = Gamepad { right_trigger: 255, ..Default::default() };
    h.pad(&mut held, Some(HEAD), &aimed(0.01), t + Duration::from_millis(20), false);
    let mut let_go = Gamepad::default();
    h.pad(&mut let_go, Some(HEAD), &aimed(0.012), t + Duration::from_millis(40), false);
    let presses = buttons(&h, front);
    assert_eq!(presses.len(), 2);
    assert_eq!(presses[0].1, presses[1].1, "let go where it was pulled: a click");
    assert!(!h.events[front].iter().skip_while(|e| !matches!(e, egui::Event::PointerButton { pressed: true, .. })).any(|e| matches!(e, egui::Event::PointerMoved(_))), "the pointer held still");
}

#[test]
fn a_hand_moved_well_away_while_pulling_drags() {
    let mut h = home();
    let t = open(&mut h, Instant::now());
    let front = h.home.front;
    let mut pulled = Gamepad { right_trigger: 255, ..Default::default() };
    h.pad(&mut pulled, Some(HEAD), &aimed(0.0), t, false);
    let mut held = Gamepad { right_trigger: 255, ..Default::default() };
    h.pad(&mut held, Some(HEAD), &aimed(0.15), t + Duration::from_millis(20), false);
    let mut let_go = Gamepad::default();
    h.pad(&mut let_go, Some(HEAD), &aimed(0.15), t + Duration::from_millis(40), false);
    let presses = buttons(&h, front);
    assert!(presses[1].1.x < presses[0].1.x - 28.0, "let go where the hand went: {presses:?}");
}

#[test]
fn the_other_hand_points_and_clicks_once_its_trigger_is_pulled() {
    let mut h = home();
    let t = open(&mut h, Instant::now());
    let both = [ahead()[1], ahead()[1]];
    let mut pad = Gamepad { left_trigger: 255, ..Default::default() };
    let asked = h.pad(&mut pad, Some(HEAD), &both, t, false);
    assert_eq!(h.pointer.hand(), LEFT_HAND);
    assert!(!asked.buzzes.is_empty(), "taking the pointer clicks where it points too");
}

#[test]
fn the_shoulders_turn_the_ring_round() {
    let mut h = home();
    let t = open(&mut h, Instant::now());
    let before = h.quads[0].anchor.position.xyz();
    let mut pad = Gamepad { buttons: button::LEFT_SHOULDER, ..Default::default() };
    h.pad(&mut pad, Some(HEAD), &ahead(), t, false);
    assert_eq!(PAGES[h.home.front], Page::Games);
    // On its way: part way round, then in front once the turn is done.
    let mut idle = Gamepad::default();
    h.pad(&mut idle, Some(HEAD), &ahead(), t + Duration::from_secs_f32(TURN_SECONDS / 2.0), false);
    let halfway = h.quads[0].anchor.position.xyz();
    assert!(halfway[0] > before[0] + 0.05 && halfway[0] < -0.05, "{before:?} -> {halfway:?}");
    let mut idle = Gamepad::default();
    h.pad(&mut idle, Some(HEAD), &ahead(), t + Duration::from_secs_f32(TURN_SECONDS + 0.05), false);
    let settled = h.quads[0].anchor.position.xyz();
    assert!(settled[0].abs() < 1e-3, "Games came round to the front: {settled:?}");
    // Log left the right end and went round behind to the left.
    assert!(h.quads[2].anchor.position.xyz()[0] < -0.5);
}

#[test]
fn the_debug_menu_asked_for_closes_the_home() {
    let mut h = home();
    let t = open(&mut h, Instant::now());
    h.home.ask_debug_menu();
    h.home.ask_capture();
    let mut pad = Gamepad::default();
    let asked = h.pad(&mut pad, Some(HEAD), &ahead(), t, false);
    assert!(asked.debug_menu && asked.capture && !h.is_open());
}

#[test]
fn b_closes_and_the_pointer_leaves() {
    let mut h = home();
    let t = open(&mut h, Instant::now());
    let front = h.home.front;
    let mut pad = Gamepad { buttons: button::B, ..Default::default() };
    h.pad(&mut pad, Some(HEAD), &ahead(), t, false);
    assert!(!h.is_open() && h.pointer.at().is_none());
    assert!(matches!(h.events[front].last(), Some(egui::Event::PointerGone)));
}

#[test]
fn a_stop_asked_on_the_games_panel_reaches_the_viewer() {
    let mut h = home();
    h.home.ask_stop();
    let mut pad = Gamepad::default();
    assert!(h.pad(&mut pad, Some(HEAD), &ahead(), Instant::now(), false).stop);
}
