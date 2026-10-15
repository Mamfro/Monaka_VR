use super::*;

#[test]
fn a_right_eye_pairs_only_with_the_left_eye_since_the_last_right() {
    let pairs = AtomicU64::new(0);
    let left = pair_of(0, &pairs);
    assert_eq!(pair_of(1, &pairs), left, "a right eye ends the pair its left eye started");
    let lone = pair_of(1, &pairs);
    assert_ne!(lone, left, "a right eye with no left eye since belongs to no pair");
    let next = pair_of(0, &pairs);
    assert!(next > lone && pair_of(1, &pairs) == next);
}

#[test]
fn a_pause_holds_stereo_off_until_resumed_while_publishing() {
    let pairs = Pairs::new();
    assert!(!pairs.resume(), "not publishing: stereo stays off");
    pairs.capture.store(true, Release);
    assert!(pairs.resume());
    pairs.pause("a test");
    assert!(!pairs.running());
    assert_eq!(pairs.pairs_done().1, 1);
    pairs.resume_at.store(0, Release);
    assert!(pairs.resume(), "the pause is over");
    pairs.stop_for_run("a test");
    pairs.resume_at.store(0, Release);
    assert!(!pairs.resume(), "stopped for the run");
}

#[test]
fn frames_between_pairs_are_not_flat() {
    let pairs = Pairs::new();
    assert!(!pairs.recent_pair());
    pairs.pair_done("test", 1);
    assert!(pairs.recent_pair());
    assert_eq!(pairs.pairs_done().0, 1);
}

#[test]
fn a_frame_holding_both_eyes_shows_the_pose_it_was_built_from() {
    let pairs = Pairs::new();
    let pose = |yaw: f32| HeadPose::synthetic(yaw, 0.0, HeadPose::SYNTHETIC_FOV, 0.064);
    pairs.built(View::Both, pose(10.0));
    pairs.built(View::Both, pose(20.0));
    assert_eq!(pairs.rendering(), Some(pose(10.0)), "the render thread draws the older frame");
    assert_eq!(pairs.shown(), (Some(pose(10.0)), 2));
    assert_eq!(pairs.shown(), (Some(pose(20.0)), 1));
    assert_eq!(pairs.shown(), (None, 0));
}

#[test]
fn the_left_eyes_present_stays_off_the_monitor_and_lone_frames_go_flat() {
    use d3d11::{Shown, shown};
    assert_eq!(shown(true, Some(0), true), Shown::Hidden);
    assert_eq!(shown(false, Some(0), false), Shown::AsIs, "another swapchain's present");
    assert_eq!(shown(true, Some(1), false), Shown::AsIs, "the right eye shows");
    assert_eq!(shown(false, None, false), Shown::Flat);
    assert_eq!(shown(false, None, true), Shown::AsIs, "between pairs");
}

#[test]
fn side_by_side_eyes_are_half_the_frame() {
    assert_eq!(gl::eye_size((3840, 2160), true), (1920, 2160));
    assert_eq!(gl::eye_size((1920, 1080), false), (1920, 1080));
}
