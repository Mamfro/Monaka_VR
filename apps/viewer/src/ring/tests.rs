use super::*;
use monaka_core::math;
use monaka_overlay::home::place::{RADIUS, SIDE_ANGLE};
use std::time::Duration;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM;

const HEAD: (Quat, Vec3) = (math::IDENTITY, [0.0, 1.6, 0.0]);

fn ring(pages: usize) -> RingView<'static> {
    RingView::new("Test ring", DXGI_FORMAT_R8G8B8A8_UNORM, pages, 0)
}

/// How far round to the right of straight ahead panel `page` stands (radians).
fn angle(ring: &RingView<'_>, page: usize) -> f32 {
    let at = ring.rectangle(page).unwrap().position;
    at[0].atan2(-at[2])
}

#[test]
fn a_ring_stands_nowhere_until_placed() {
    let r = ring(2);
    assert!(!r.placed() && r.rectangle(0).is_none());
}

#[test]
fn the_panel_in_front_stands_ahead_and_the_rest_round_the_ring_the_same_size() {
    let mut r = ring(4);
    r.place_at(HEAD);
    let ahead = r.rectangle(0).unwrap();
    assert!(ahead.position[0].abs() < 1e-4 && (ahead.position[2] + RADIUS).abs() < 1e-4, "{:?}", ahead.position);
    assert!((angle(&r, 1) - SIDE_ANGLE).abs() < 1e-4, "the next on the right");
    assert!((angle(&r, 3) + SIDE_ANGLE).abs() < 1e-4, "the last on the left");
    assert!((0..4).all(|page| r.rectangle(page).unwrap().size == [place::WIDTH, place::height()]), "every panel one size");
}

#[test]
fn a_turn_brings_the_next_panel_in_front_going_round() {
    let mut r = ring(3);
    r.place_at(HEAD);
    let now = Instant::now();
    r.turn(1, now);
    assert_eq!(r.front(), 1);
    r.step(now + Duration::from_secs_f32(TURN_SECONDS / 2.0));
    assert!(angle(&r, 1) > 0.0 && angle(&r, 1) < SIDE_ANGLE, "on its way in: {}", angle(&r, 1));
    r.step(now + Duration::from_secs_f32(TURN_SECONDS * 1.5));
    assert!(angle(&r, 1).abs() < 1e-4, "in front once done");
    r.turn(-2, now);
    assert_eq!(r.front(), 2, "turned back past the first, round to the last");
}

#[test]
fn panels_come_and_go_at_the_end_and_the_front_stays_on_the_ring() {
    let mut r = ring(3);
    r.place_at(HEAD);
    r.show_in_front(2);
    r.set_pages(1);
    assert_eq!(r.front(), 0, "the front went: the last left in front");
    assert!(r.rectangle(1).is_none());
    r.set_pages(3);
    assert!(r.rectangle(2).is_some(), "a new panel stands on the ring at once");
}
