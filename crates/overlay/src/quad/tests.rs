use super::*;
use monaka_channel::d3d::{create_device, create_texture, create_texture_with, read_image, simple_texture_desc};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM;

const SIZE: u32 = 64;

#[test]
fn a_panel_a_metre_ahead_lands_where_the_eye_sees_it() {
    let (device, context, _) = create_device(None).unwrap();
    let mut desc = simple_texture_desc(SIZE, SIZE, DXGI_FORMAT_R8G8B8A8_UNORM);
    desc.BindFlags = (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32;
    let image = create_texture_with(&device, &desc, &[0, 0, 0, 77].repeat((SIZE * SIZE) as usize)).unwrap();
    let mut picture_desc = simple_texture_desc(4, 4, DXGI_FORMAT_R8G8B8A8_UNORM);
    picture_desc.BindFlags = D3D11_BIND_SHADER_RESOURCE.0 as u32;
    let picture = create_texture_with(&device, &picture_desc, &[255, 0, 0, 255].repeat(16)).unwrap();
    let quarter = std::f32::consts::FRAC_PI_4;
    let eye = EyeView { orientation: [0.0, 0.0, 0.0, 1.0], position: [0.0; 3], fov: [-quarter, quarter, quarter, -quarter] };
    // A metre square a metre ahead, facing the eye: half the image's width and height.
    let panel = Rectangle { orientation: [0.0, 0.0, 0.0, 1.0], position: [0.0, 0.0, -1.0], size: [1.0, 1.0] };
    QuadPass::new(&device).unwrap().draw(&device, &context, &image, DXGI_FORMAT_R8G8B8A8_UNORM, &eye, &picture, &panel).unwrap();
    let mut copy_desc = simple_texture_desc(SIZE, SIZE, DXGI_FORMAT_R8G8B8A8_UNORM);
    copy_desc.BindFlags = 0;
    let copy = create_texture(&device, &copy_desc).unwrap();
    // SAFETY: two live textures of one size and format.
    unsafe { context.CopyResource(&copy, &image) };
    let pixels = read_image(&device, &context, &copy).unwrap().2;
    let at = |x: u32, y: u32| -> [u8; 4] { pixels[((y * SIZE + x) * 4) as usize..][..4].try_into().unwrap() };
    assert_eq!(at(32, 32), [255, 0, 0, 77], "the panel, the image's alpha kept");
    assert_eq!(at(18, 18), [255, 0, 0, 77], "near its top left corner (16, 16)");
    assert_eq!(at(12, 32), [0, 0, 0, 77], "beside it");
}
