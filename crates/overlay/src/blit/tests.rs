use super::*;
use monaka_channel::d3d::{create_device, create_texture, create_texture_with, read_image, simple_texture_desc};

#[test]
fn an_image_keeps_its_shape_centred_in_the_window() {
    assert_eq!(fitted([2000, 1000], [1000, 1000]), [0.0, 250.0, 1000.0, 500.0]);
    assert_eq!(fitted([1000, 1000], [1600, 800]), [400.0, 0.0, 800.0, 800.0]);
}

#[test]
fn an_srgb_image_reaches_a_plain_target_as_stored() {
    let (device, context, _) = create_device(None).unwrap();
    let mut source = simple_texture_desc(4, 4, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB);
    source.BindFlags = D3D11_BIND_SHADER_RESOURCE.0 as u32;
    let image = create_texture_with(&device, &source, &[128u8, 64, 32, 255].repeat(16)).unwrap();
    let mut target_desc = simple_texture_desc(8, 4, DXGI_FORMAT_R8G8B8A8_UNORM);
    target_desc.BindFlags = (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32;
    let target = create_texture(&device, &target_desc).unwrap();
    let mut view = None;
    // SAFETY: a view of our own texture.
    unsafe { device.CreateRenderTargetView(&target, None, Some(&mut view)) }.unwrap();
    let view = view.unwrap();
    // SAFETY: clears our own target.
    unsafe { context.ClearRenderTargetView(&view, &[0.0; 4]) };
    let mut blit = BlitPass::new(&device).unwrap();
    blit.draw(&device, &context, &view, &image, fitted([4, 4], [8, 4])).unwrap();
    let (width, _, pixels) = read_image(&device, &context, &target).unwrap();
    let at = |x: u32, y: u32| &pixels[((y * width + x) * 4) as usize..][..4];
    assert_eq!(at(4, 2), [128, 64, 32, 255], "the stored values, not linearized");
    assert_eq!(at(0, 2), [0, 0, 0, 0], "the bars beside it untouched");
}
