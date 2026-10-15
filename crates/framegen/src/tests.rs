use super::*;

#[test]
fn halfway_pose() {
    let a = HeadPose { orientation: [0.0, 0.0, 0.0, 1.0], position: [0.0, 1.0, 0.0], ipd: 0.06, ..HeadPose::default() };
    let half = 0.2f32;
    let b = HeadPose { orientation: [0.0, half.sin(), 0.0, half.cos()], position: [0.2, 1.0, 0.0], ipd: 0.06, ..HeadPose::default() };
    let m = blend(&a, &b, 0.5);
    assert!((m.position[0] - 0.1).abs() < 1e-6);
    assert!((m.orientation[1] - 0.1f32.sin()).abs() < 1e-3);
}
