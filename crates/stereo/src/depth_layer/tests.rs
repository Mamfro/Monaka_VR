use super::*;
use monaka_framegen::inputs::Rendering;
use monaka_streamline::{Constants, constants as at};

#[test]
fn the_games_projection_gives_the_near_plane_in_metres() {
    // DLSS's view to clip (row vectors): reverse-Z infinite, near 0.05, the camera looking down -z.
    let mut bytes = vec![0u8; at::BYTES];
    let mut put = |offset: usize, values: &[f32]| {
        for (i, v) in values.iter().enumerate() {
            bytes[offset + 4 * i..offset + 4 * i + 4].copy_from_slice(&v.to_le_bytes());
        }
    };
    put(at::CAMERA_VIEW_TO_CLIP, &[1.036, 0.0, 0.0, 0.0, 0.0, 1.84, 0.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.05, 0.0]);
    put(at::UP, &[0.0, 1.0, 0.0]);
    put(at::RIGHT, &[1.0, 0.0, 0.0]);
    put(at::FORWARD, &[0.0, 0.0, -1.0]);
    put(at::NEAR, &[0.05]);
    UNITS_PER_METRE.store(1.0f32.to_bits(), Release);
    let inputs = EyeInputs { rendering: Constants::read(bytes.as_ptr()).as_ref().map(Rendering::from_constants), ..EyeInputs::default() };
    assert_eq!(params(&inputs), Some(DepthParams::new(0.05, f32::INFINITY, true)));
    assert_eq!(params(&EyeInputs::default()), None, "no camera");
}
