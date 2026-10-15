use super::*;
use monaka_channel::d3d::{create_device, read_pixel};

#[test]
fn a_half_transparent_hud_lands_in_its_rect() {
    let (device, context, _) = create_device(None).unwrap();
    let mut hud = HudLayer::new(&device).unwrap();
    let targets = hud.begin(&context, 32, 16).unwrap();
    // The game's HUD: the whole layer half-covered with red (premultiplied (0.5, 0, 0), a 0.5),
    // as an ordinary over-blend would leave it on black and on white.
    unsafe {
        context.ClearRenderTargetView(&targets[0], &[0.5, 0.0, 0.0, 0.0]);
        context.ClearRenderTargetView(&targets[1], &[1.0, 0.5, 0.5, 1.0]);
    }
    // An eye image of plain blue, the HUD placed in its right half.
    let mut desc = d3d::simple_texture_desc(64, 16, DXGI_FORMAT_R8G8B8A8_UNORM);
    desc.BindFlags = (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32;
    let base = d3d::create_texture(&device, &desc).unwrap();
    let mut view = None;
    unsafe {
        device.CreateRenderTargetView(&base, None, Some(&mut view)).unwrap();
        context.ClearRenderTargetView(view.as_ref().unwrap(), &[0.0, 0.0, 1.0, 1.0]);
    }
    let out = hud.composite(&context, 0, &base, [32.0, 0.0, 32.0, 16.0]).unwrap().clone();
    assert_eq!(read_pixel(&device, &context, &out, 5, 5).unwrap(), [0, 0, 255, 255], "outside the rect: the eye image");
    let mixed = read_pixel(&device, &context, &out, 50, 5).unwrap();
    assert!(mixed[0].abs_diff(128) <= 2 && mixed[1] == 0 && mixed[2].abs_diff(128) <= 2, "half red over blue: {mixed:?}");
}
