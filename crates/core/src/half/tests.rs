use super::*;

#[test]
fn normal_values() {
    assert_eq!(to_f32(0x3c00), 1.0);
    assert_eq!(to_f32(0xc000), -2.0);
    assert_eq!(to_f32(0x3555), 1365.0 / 4096.0, "nearest to a third");
    assert_eq!(to_f32(0x7bff), 65504.0, "largest finite");
    assert_eq!(to_f32(0x0400), 2f32.powi(-14), "smallest normal");
}

#[test]
fn zeros_and_subnormals() {
    assert_eq!(to_f32(0x0000).to_bits(), 0.0f32.to_bits());
    assert_eq!(to_f32(0x8000).to_bits(), (-0.0f32).to_bits(), "negative zero keeps its sign");
    assert_eq!(to_f32(0x0001), 2f32.powi(-24), "smallest subnormal");
    assert_eq!(to_f32(0x03ff), 1023.0 * 2f32.powi(-24), "largest subnormal");
    assert_eq!(to_f32(0x8001), -(2f32.powi(-24)));
}

#[test]
fn infinities_and_nan() {
    assert_eq!(to_f32(0x7c00), f32::INFINITY);
    assert_eq!(to_f32(0xfc00), f32::NEG_INFINITY);
    assert!(to_f32(0x7e00).is_nan());
    assert!(to_f32(0xfc01).is_nan());
}
