use super::*;

#[test]
fn tags_are_read_from_the_game_array() {
    // An sl::Resource: type at +32, native at +40, state at +64.
    let mut resource = [0u8; 128];
    resource[32] = RESOURCE_TEX2D;
    resource[40..48].copy_from_slice(&0x1234usize.to_le_bytes());
    resource[64..68].copy_from_slice(&8u32.to_le_bytes());
    let mut tags = [0u8; 2 * TAG_BYTES];
    tags[32..40].copy_from_slice(&(resource.as_ptr() as usize).to_le_bytes());
    tags[40..44].copy_from_slice(&buffer::MOTION_VECTORS.to_le_bytes());
    // The second tag names no resource.
    let read = read_tags(tags.as_ptr(), 2);
    assert_eq!(read.len(), 1);
    assert_eq!((read[0].kind, read[0].native, read[0].state, read[0].resource_type), (buffer::MOTION_VECTORS, 0x1234, 8, RESOURCE_TEX2D));
    assert!(read_tags(std::ptr::null(), 4).is_empty());
}

#[test]
fn eye_viewports_take_the_top_bits() {
    assert_eq!(eye_viewport(5, 1), 5 | (1 << 30));
    assert_eq!(eye_viewport(5, 2), 5 | (2 << 30));
    assert_eq!(eye_viewport(0x4000_0005, 2), 0x8000_0005);
    assert_eq!(own_eye_viewport(5, 1), Some(5 | (1 << 30)));
    assert_eq!(own_eye_viewport(5, 0), None, "eyes are 1 and 2");
    assert_eq!(own_eye_viewport(0x4000_0001, 1), None, "an id already an eye's");
}

#[test]
fn constants_read_and_change() {
    let mut raw = [0u8; constants::BYTES];
    raw[constants::NEAR..constants::NEAR + 4].copy_from_slice(&0.05f32.to_le_bytes());
    let mut c = Constants::read(raw.as_ptr()).unwrap();
    assert_eq!(c.f32(constants::NEAR), 0.05);
    assert!(!c.reset());
    c.set_reset(true);
    assert!(c.reset());
    let m: [f32; 16] = std::array::from_fn(|i| i as f32);
    c.set_matrix(constants::CLIP_TO_PREV_CLIP, &m);
    assert_eq!(c.matrix(constants::CLIP_TO_PREV_CLIP), m);
}
