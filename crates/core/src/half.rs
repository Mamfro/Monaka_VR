//! IEEE 754 half floats (`R16G16_FLOAT` motion vectors, `R16G16B16A16_FLOAT` targets) as the GPU
//! stores them, decoded on the CPU for probes and tests.

/// The value of half float `bits`: subnormals exact, exponent 31 as infinity (no mantissa) or NaN,
/// the sign kept on zero.
pub fn to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = i32::from((bits >> 10) & 0x1f);
    let mantissa = f32::from(bits & 0x3ff);
    match exponent {
        0 => sign * mantissa * 2f32.powi(-24),
        31 if mantissa == 0.0 => sign * f32::INFINITY,
        31 => f32::NAN,
        e => sign * (1.0 + mantissa / 1024.0) * 2f32.powi(e - 15),
    }
}

#[cfg(test)]
mod tests {
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
}
