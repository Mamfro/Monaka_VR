use super::*;

fn alpha_at(pixels: &[u8], size: u32, x: u32, y: u32) -> u8 {
    pixels[((y * size + x) * 4 + 3) as usize]
}

#[test]
fn closed_the_middle_is_clear_and_the_edges_black() {
    let image = image(128, 1.0);
    assert_eq!(alpha_at(&image, 128, 64, 64), 0, "the middle clear");
    // 40 degrees off the middle: within the soft edge, partly dark.
    let at = 64 + (40f32.to_radians().tan() / HALF_TAN * 64.0) as u32;
    let partly = alpha_at(&image, 128, at, 64);
    assert!(partly > 0 && partly < 255, "{partly}");
    // 60 degrees off: black.
    let at = 64 + (60f32.to_radians().tan() / HALF_TAN * 64.0) as u32;
    assert_eq!(alpha_at(&image, 128, at, 64), 255);
    assert!(image.chunks(4).all(|p| p[..3] == [0, 0, 0]), "premultiplied black");
}

#[test]
fn half_closed_lets_more_of_the_view_through() {
    let at = 64 + (50f32.to_radians().tan() / HALF_TAN * 64.0) as u32;
    assert!(alpha_at(&image(128, 0.5), 128, at, 64) < alpha_at(&image(128, 1.0), 128, at, 64));
}

#[test]
fn the_sticks_move_the_player_past_their_dead_zone() {
    let pad = |lx: i16, ly: i16, rx: i16| Gamepad { thumb_lx: lx, thumb_ly: ly, thumb_rx: rx, ..Gamepad::default() };
    assert_eq!(moving(&pad(0, 0, 0), false), 0.0);
    assert_eq!(moving(&pad(2000, 1000, 0), false), 0.0, "resting drift");
    assert!((moving(&pad(0, 32767, 0), false) - 1.0).abs() < 1e-4, "walking flat out");
    assert!((moving(&pad(0, 0, -32767), false) - 1.0).abs() < 1e-4, "a smooth turn");
    assert_eq!(moving(&pad(0, 0, -32767), true), 0.0, "a snap turn is none");
}

#[test]
fn the_ring_closes_quickly_and_opens_gently() {
    let closing = eased(0.0, 1.0, 0.1);
    let opening = 1.0 - eased(1.0, 0.0, 0.1);
    assert!(closing > opening && closing > 0.0 && opening > 0.0, "{closing} {opening}");
    assert_eq!(eased(0.0, 1.0, 5.0), 1.0, "no further than asked");
    assert_eq!(level(0.0), 0);
    assert_eq!(level(1.0), LEVELS - 1);
}
