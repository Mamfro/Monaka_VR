use super::*;
use monaka_channel::d3d::{create_device, create_texture, read_image, simple_texture_desc};
use monaka_contract::debug_draw::{Category, RED, WHITE};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R8G8B8A8_TYPELESS, DXGI_FORMAT_R8G8B8A8_UNORM};

const SIZE: u32 = 64;

/// Draws `primitives` over a black 64x64 image seen by an eye at the origin looking down -Z with
/// a 90 degree square field of view, and gives its pixels (RGBA8 rows).
fn drawn(primitives: &[Primitive], storage: DXGI_FORMAT) -> Vec<u8> {
    let (device, context, _) = create_device(None).unwrap();
    let mut desc = simple_texture_desc(SIZE, SIZE, storage);
    desc.BindFlags = (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32;
    let image = create_texture(&device, &desc).unwrap();
    let quarter = std::f32::consts::FRAC_PI_4;
    let eye = EyeView { orientation: [0.0, 0.0, 0.0, 1.0], position: [0.0; 3], fov: [-quarter, quarter, quarter, -quarter] };
    let mut pass = LinePass::new(&device).unwrap();
    pass.draw(&device, &context, &image, DXGI_FORMAT_R8G8B8A8_UNORM, &eye, primitives, 3.0).unwrap();
    let mut copy_desc = simple_texture_desc(SIZE, SIZE, DXGI_FORMAT_R8G8B8A8_UNORM);
    copy_desc.BindFlags = 0;
    let typed = create_texture(&device, &copy_desc).unwrap();
    // SAFETY: two live textures of one size and format family.
    unsafe { context.CopyResource(&typed, &image) };
    read_image(&device, &context, &typed).unwrap().2
}

fn at(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * SIZE + x) * 4) as usize;
    [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
}

#[test]
fn a_level_line_crosses_the_middle() {
    let line = Primitive::segment(Category::Probe, [-1.0, 0.0, -2.0], [1.0, 0.0, -2.0], RED);
    let pixels = drawn(&[line], DXGI_FORMAT_R8G8B8A8_UNORM);
    // x = +-1 at 2 m ahead is at tan = 0.5: a quarter of the way in from each edge.
    assert_eq!(at(&pixels, 32, 32)[0], 255, "on the line");
    assert_eq!(at(&pixels, 20, 32)[0], 255);
    assert_eq!(at(&pixels, 32, 20), [0; 4], "above it");
    assert_eq!(at(&pixels, 4, 32), [0; 4], "past its end");
}

#[test]
fn a_typeless_image_and_a_line_through_the_eye_are_drawn() {
    // From behind the eye to ahead of it: cut at the near plane, the part ahead drawn.
    let line = Primitive::segment(Category::Probe, [0.5, 0.5, 1.0], [0.5, 0.5, -1.0], WHITE);
    let point = Primitive::point(Category::Probe, [-0.5, -0.5, -1.0], WHITE);
    let pixels = drawn(&[line, point], DXGI_FORMAT_R8G8B8A8_TYPELESS);
    // The far end, at tan 0.5 right and up: three quarters across, a quarter down. White, and
    // the image's alpha (0 here) kept.
    assert_eq!(at(&pixels, 48, 16), [255, 255, 255, 0]);
    // The point at tan 0.5 left and down.
    assert_eq!(at(&pixels, 16, 48), [255, 255, 255, 0]);
    assert_eq!(at(&pixels, 16, 16), [0; 4]);
}
