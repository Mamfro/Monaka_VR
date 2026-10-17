use super::*;
use monaka_channel::d3d::{create_device, create_texture_with, read_image};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM;

const SIZE: u32 = 256;

#[test]
fn the_eye_behind_a_panel_is_blurred_and_the_rest_left_alone() {
    let (device, context, _) = create_device(None).unwrap();
    // Black and white stripes four pixels wide, the alpha saying where pixels came from.
    let stripes: Vec<u8> = (0..SIZE * SIZE).flat_map(|i| if (i % SIZE / 4).is_multiple_of(2) { [0, 0, 0, 77] } else { [255, 255, 255, 77] }).collect();
    let mut desc = simple_texture_desc(SIZE, SIZE, DXGI_FORMAT_R8G8B8A8_UNORM);
    desc.BindFlags = (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32;
    let image = create_texture_with(&device, &desc, &stripes).unwrap();
    let source = create_texture_with(&device, &desc, &stripes).unwrap();
    let quarter = std::f32::consts::FRAC_PI_4;
    let eye = EyeView { orientation: [0.0, 0.0, 0.0, 1.0], position: [0.0; 3], fov: [-quarter, quarter, quarter, -quarter] };
    // A metre square a metre ahead: the middle half of the image each way (64 to 192).
    let panel = Rectangle { orientation: [0.0, 0.0, 0.0, 1.0], position: [0.0, 0.0, -1.0], size: [1.0, 1.0] };
    FrostPass::new(&device).unwrap().draw(&device, &context, &image, DXGI_FORMAT_R8G8B8A8_UNORM, &source, &eye, &[panel]).unwrap();
    let mut copy_desc = simple_texture_desc(SIZE, SIZE, DXGI_FORMAT_R8G8B8A8_UNORM);
    copy_desc.BindFlags = 0;
    let copy = create_texture(&device, &copy_desc).unwrap();
    // SAFETY: two live textures of one size and format.
    unsafe { context.CopyResource(&copy, &image) };
    let pixels = read_image(&device, &context, &copy).unwrap().2;
    let at = |x: u32, y: u32| -> [u8; 4] { pixels[((y * SIZE + x) * 4) as usize..][..4].try_into().unwrap() };
    for (x, y) in [(128, 128), (100, 150), (70, 128)] {
        let [r, g, b, a] = at(x, y);
        assert!((100..=155).contains(&r) && r == g && g == b, "grey behind the panel at ({x}, {y}): {r} {g} {b}");
        assert_eq!(a, 77, "the image's alpha kept");
    }
    assert_eq!(at(40, 128), stripes_at(&stripes, 40, 128), "beside the panel");
    assert_eq!(at(128, 220), stripes_at(&stripes, 128, 220), "below it");
    assert_eq!(at(64, 64), stripes_at(&stripes, 64, 64), "outside its rounded corner");
}

fn stripes_at(stripes: &[u8], x: u32, y: u32) -> [u8; 4] {
    stripes[((y * SIZE + x) * 4) as usize..][..4].try_into().unwrap()
}
