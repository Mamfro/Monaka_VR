use super::*;

#[test]
fn a_messages_coordinates_keep_their_sign() {
    // x = -5, y = 300, as a captured drag past the window's left edge sends them.
    let packed = ((300isize) << 16) | (-5i16 as u16 as isize);
    assert_eq!(words(packed), (-5.0, 300.0));
}
