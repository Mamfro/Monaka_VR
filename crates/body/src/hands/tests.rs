use super::*;

#[test]
fn a_point_on_the_palm_is_in_its_frame() {
    // A palm turned a quarter turn about +y: its +x points along the world's -z.
    let s = std::f32::consts::FRAC_1_SQRT_2;
    let m: Mat34 = [0.0, 0.0, 1.0, 2.0, 0.0, 1.0, 0.0, 3.0, -1.0, 0.0, 0.0, 4.0];
    assert_eq!(offset(&m, [1.0, 0.0, 0.0]), [2.0, 3.0, 3.0]);
    assert_eq!(offset(&m, [0.0; 3]), [2.0, 3.0, 4.0]);
    let turned = HandPose { orientation: [0.0, s, 0.0, s], position: [2.0, 3.0, 4.0], valid: 1, received_tick: 0 };
    let at = on_palm(&turned, [1.0, 0.0, 0.0]);
    assert!((at[0] - 2.0).abs() < 1e-6 && (at[1] - 3.0).abs() < 1e-6 && (at[2] - 3.0).abs() < 1e-6, "{at:?}");
}
