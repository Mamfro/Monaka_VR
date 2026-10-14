use super::*;

#[test]
fn a_capture_image_is_a_jpeg_and_a_warped_one_gets_its_sources() {
    let folder = std::env::temp_dir().join(format!("monaka-capture-test-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let (width, height) = (64u32, 32u32);
    // A gradient, the right half from the eye's own last frame (alpha in the 128-212 band).
    let texels: Vec<u8> = (0..width * height).flat_map(|i| [(i % width * 4) as u8, (i / width * 8) as u8, 90, if i % width < width / 2 { 255 } else { 170 }]).collect();
    let path = folder.join("pair-1-left.jpg");
    write_jpeg(&path, width, height, dxgi_format::R8G8B8A8_UNORM, &texels);
    for name in ["pair-1-left.jpg", "pair-1-left-sources.jpg"] {
        let bytes = std::fs::read(folder.join(name)).unwrap();
        assert_eq!(&bytes[..2], &[0xff, 0xd8], "{name} starts as a JPEG");
        assert_eq!(&bytes[bytes.len() - 2..], &[0xff, 0xd9], "{name} ends as a JPEG");
    }
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn the_first_pair_goes_side_by_side_left_first_the_shorter_eye_padded_black() {
    let eye = |width: u32, height: u32, value: u8| (width, height, dxgi_format::R8G8B8A8_UNORM, vec![value; (width * height * 4) as usize]);
    let (width, height, rgb) = side_by_side(&[eye(2, 2, 10), eye(1, 3, 200)]).unwrap();
    assert_eq!((width, height), (3, 3));
    let pixel = |x: u32, y: u32| rgb[((y * width + x) * 3) as usize];
    assert_eq!([pixel(0, 0), pixel(1, 1), pixel(2, 0), pixel(2, 2)], [10, 10, 200, 200]);
    assert_eq!(pixel(0, 2), 0, "below the shorter left eye: black");
}
