use super::*;
use monaka_channel::d3d::{create_device, read_pixel};

#[test]
fn a_wide_image_fits_across_with_bands_above_and_below() {
    assert_eq!(fit((200, 100), (400, 400)), [0.0, 100.0, 400.0, 200.0]);
    assert_eq!(fit((100, 100), (300, 300)), [0.0, 0.0, 300.0, 300.0]);
}

#[test]
fn a_small_image_is_scaled_up_with_its_aspect_kept() {
    let (device, context, _) = create_device(None).unwrap();
    let mut scaler = Scaler::new(&device).unwrap();
    // A 32x16 back buffer: left half red, right half blue.
    let mut desc = d3d::simple_texture_desc(32, 16, DXGI_FORMAT_R8G8B8A8_UNORM);
    desc.BindFlags = D3D11_BIND_RENDER_TARGET.0 as u32;
    let pixels: Vec<u8> = (0..32 * 16).flat_map(|i| if i % 32 < 16 { [255, 0, 0, 255] } else { [0, 0, 255, 255] }).collect();
    let data = D3D11_SUBRESOURCE_DATA { pSysMem: pixels.as_ptr().cast(), SysMemPitch: 32 * 4, SysMemSlicePitch: 0 };
    let mut source = None;
    unsafe { device.CreateTexture2D(&desc, Some(&data), Some(&mut source)).unwrap() };
    let source = source.unwrap();
    let out = scaler.run(&context, &source, (64, 64)).unwrap().clone();
    // 64 across, 32 tall, centred: bands of 16 above and below.
    assert_eq!(read_pixel(&device, &context, &out, 10, 5).unwrap(), [0, 0, 0, 255], "the band above");
    assert_eq!(read_pixel(&device, &context, &out, 10, 30).unwrap(), [255, 0, 0, 255], "the left half");
    assert_eq!(read_pixel(&device, &context, &out, 54, 30).unwrap(), [0, 0, 255, 255], "the right half");
    assert_eq!(read_pixel(&device, &context, &out, 54, 60).unwrap(), [0, 0, 0, 255], "the band below");
}
