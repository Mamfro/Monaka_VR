use super::*;

#[test]
fn a_side_by_side_back_buffer_splits_into_two_halves() {
    assert_eq!(eye_size(3840, 2160, 1, 24), Ok((1920, 2160)));
    assert_eq!(half(0, (1920, 2160)), [0, 0, 1920, 2160]);
    assert_eq!(half(1, (1920, 2160)), [1920, 0, 3840, 2160]);
    assert!(eye_size(3841, 2160, 1, 24).is_err(), "an odd width has no halves");
    assert!(eye_size(3840, 2160, 4, 24).is_err(), "a multisampled back buffer cannot be copied");
}
