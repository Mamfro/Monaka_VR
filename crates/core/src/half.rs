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
mod tests;
