use super::*;

#[test]
fn turns_wrap() {
    assert!((turned(-170.0, 170.0) - 20.0).abs() < 1e-4);
    assert!((turned(170.0, -170.0) + 20.0).abs() < 1e-4);
    assert!((turned(10.0, 5.0) - 5.0).abs() < 1e-4);
}
