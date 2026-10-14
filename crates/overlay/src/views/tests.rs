use super::*;
use monaka_channel::d3d::{create_device, create_texture, create_texture_with, read_image, simple_texture_desc};

const SIZE: u32 = 8;

fn picture(device: &ID3D11Device, rgba: [u8; 4]) -> ID3D11Texture2D {
    let mut desc = simple_texture_desc(SIZE, SIZE, DXGI_FORMAT_R8G8B8A8_UNORM);
    desc.BindFlags = (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32;
    create_texture_with(device, &desc, &rgba.repeat((SIZE * SIZE) as usize)).unwrap()
}

/// Draws `view` of an eye (`this`, against `other`, with `depth`) and gives its pixels.
fn drawn(view: View, this: [u8; 4], other: [u8; 4], depth: Option<f32>) -> Option<Vec<u8>> {
    let (device, context, _) = create_device(None).unwrap();
    let (image, other) = (picture(&device, this), picture(&device, other));
    let target = picture(&device, [0; 4]);
    let mut desc = simple_texture_desc(SIZE, SIZE, DXGI_FORMAT_R32_TYPELESS);
    desc.BindFlags = D3D11_BIND_SHADER_RESOURCE.0 as u32;
    let depth_texture = depth.map(|d| create_texture_with(&device, &desc, &d.to_le_bytes().repeat((SIZE * SIZE) as usize)).unwrap());
    // Reversed depth from 0.1 m: 1 at 10 cm, 0 at infinity.
    let depth = depth_texture.as_ref().map(|texture| Depth { texture, rect: [0, 0, SIZE, SIZE], distances: (f32::INFINITY, 0.1) });
    let mut pass = ViewPass::new(&device).unwrap();
    let eye = Eye { image: &image, other: &other, depth, age: 1, range: 10.0, logarithmic: false, buffer: None };
    if !pass.draw(&device, &context, &target, DXGI_FORMAT_R8G8B8A8_UNORM, view, &eye).unwrap() {
        return None;
    }
    let mut copy_desc = simple_texture_desc(SIZE, SIZE, DXGI_FORMAT_R8G8B8A8_UNORM);
    copy_desc.BindFlags = 0;
    let copy = create_texture(&device, &copy_desc).unwrap();
    // SAFETY: two live textures of one size and format.
    unsafe { context.CopyResource(&copy, &target) };
    Some(read_image(&device, &context, &copy).unwrap().2)
}

#[test]
fn depth_is_white_near_and_dark_far_and_keeps_alpha() {
    // Depth 1 (reversed) is 10 cm: nearly white over a 10 m range. Depth 0.02 is 5 m: half.
    // Depth 0.001 is 100 m: black.
    let near = drawn(View::Depth, [10, 20, 30, 170], [0; 4], Some(1.0)).unwrap();
    assert!(near[0] >= 250 && near[3] == 170, "{:?}", &near[..4]);
    let half = drawn(View::Depth, [10, 20, 30, 170], [0; 4], Some(0.02)).unwrap();
    assert!((120..=135).contains(&half[0]), "{:?}", &half[..4]);
    let far = drawn(View::Depth, [10, 20, 30, 170], [0; 4], Some(0.001)).unwrap();
    assert!(far[0] < 5, "{:?}", &far[..4]);
    assert_eq!(drawn(View::Depth, [0; 4], [0; 4], None), None, "no depth, no depth view");
    assert_eq!(drawn(View::Picture, [0; 4], [0; 4], None), None, "the picture is the plain copy");
}

#[test]
fn the_difference_and_the_age_tint_show() {
    let difference = drawn(View::Difference, [100, 100, 100, 255], [90, 100, 130, 255], None).unwrap();
    assert_eq!(&difference[..4], &[40, 0, 120, 255]);
    let aged = drawn(View::FrameAge, [100, 100, 100, 255], [0; 4], None).unwrap();
    assert!(aged[2] > aged[0] + 60, "a frame older by one is tinted blue: {:?}", &aged[..4]);
}
