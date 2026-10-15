use super::*;

#[test]
fn layout_is_xinputs() {
    assert_eq!(size_of::<Gamepad>(), 12);
    assert_eq!(std::mem::offset_of!(Gamepad, thumb_lx), 4);
}
