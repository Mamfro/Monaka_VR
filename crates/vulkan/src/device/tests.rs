use super::*;

#[test]
fn the_record_fits_a_page_or_two_and_keeps_its_layout() {
    assert_eq!(size_of::<Chain>(), 88);
    assert_eq!(size_of::<DepthImage>(), 24);
    assert_eq!(size_of::<ColorImage>(), 24);
    assert!(size_of::<Record>() < 8192, "{}", size_of::<Record>());
}
