use super::*;

#[test]
fn a_centre_going_back_against_its_motion_is_a_step_back() {
    assert_eq!(step_back([0.0; 3], [0.01, 0.0, 0.0], [0.0, 0.0, 0.0]).map(|s| s.0), Some(true));
    assert_eq!(step_back([0.0; 3], [0.01, 0.0, 0.0], [0.02, 0.0, 0.0]).map(|s| s.0), Some(false));
    assert_eq!(step_back([0.0; 3], [0.001, 0.0, 0.0], [0.0; 3]), None, "standing still");
    assert_eq!(step_back([f32::NAN; 3], [0.0; 3], [0.01, 0.0, 0.0]), None, "no history yet");
}

#[test]
fn a_smooth_turn_stays_on_its_line_across_the_wrap() {
    assert!(off_line(0.0, 0.1, 0.2).abs() < 1e-6);
    let pi = std::f32::consts::PI;
    assert!(off_line(pi - 0.2, pi - 0.1, -pi + 0.0).abs() < 1e-5, "through +-180 degrees");
    assert!((off_line(0.0, 0.1, 0.3) - 0.1).abs() < 1e-6);
}

#[test]
fn the_camera_read_is_a_turn_the_games_or_neither() {
    assert_eq!(camera_kind(0.001, 0.5, 0.01), 0);
    assert_eq!(camera_kind(0.5, 0.001, 0.01), 1);
    assert_eq!(camera_kind(0.02, 0.03, 0.01), 2);
    assert_eq!(camera_kind(0.005, 0.005, 0.01), 0, "a tie is a turn of ours");
}
