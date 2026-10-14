use super::*;

#[test]
fn the_eye_is_its_regions_share_of_the_frame() {
    assert_eq!(region_size(3840, 2160, [0.0, 0.0, 0.5, 1.0]), (1920, 2160));
    assert_eq!(region_size(3839, 2160, [0.0, 0.0, 0.5, 1.0]), (1920, 2160), "rounded");
    assert_eq!(region_size(10, 10, [0.0, 0.0, 0.0, 0.0]), (1, 1), "never empty");
}

#[test]
fn the_depth_is_taken_whole_when_rendered_at_the_views_size() {
    let left = [0.0, 0.0, 0.5, 1.0];
    assert_eq!(depth_region(left, 3840, 3840), left, "the depth covers the window: the same half");
    assert_eq!(depth_region(left, 3840, 1920), [0.0, 0.0, 1.0, 1.0], "the depth is the view's own: all of it");
    assert_eq!(depth_region(left, 3840, 3072), left, "a render scale of 0.8 still covers the window");
}
