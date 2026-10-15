use super::*;
use monaka_channel::d3d::{create_device, read_pixel};

#[test]
fn black_and_white_draws_give_premultiplied_coverage() {
    let (device, context, _) = create_device(None).unwrap();
    let channel = ChannelName::new("monaka-panel-test".to_string()).unwrap();
    let panel = Panel::new(&device, &channel, "weapon", (16, 8), DXGI_FORMAT_R8G8B8A8_UNORM, false).unwrap();
    assert!(Panel::new(&device, &channel, "weapon", (16, 8), DXGI_FORMAT_R8G8B8A8_UNORM_SRGB, false).is_ok(), "an sRGB HUD target");
    let masked = Panel::new(&device, &channel, "weapon", (16, 8), DXGI_FORMAT_R8G8B8A8_UNORM, true).unwrap();
    assert!(masked.depth_views().iter().all(Option::is_some) && panel.depth_views().iter().all(Option::is_none));
    masked.clear(&context);
    // Half-covering grey: onto black 0.25, onto white 0.75 (alpha one half).
    let [black, white] = panel.target_views();
    unsafe {
        context.ClearRenderTargetView(black, &[0.25, 0.25, 0.25, 1.0]);
        context.ClearRenderTargetView(white, &[0.75, 0.75, 0.75, 1.0]);
    }
    panel.resolve(&context);
    let pixel = read_pixel(&device, &context, panel.out(), 3, 3).unwrap();
    assert!(pixel.iter().take(3).all(|&c| (63..=65).contains(&c)) && (126..=129).contains(&pixel[3]), "{pixel:?}");
}
