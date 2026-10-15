use super::*;
use monaka_channel::d3d::{create_device, read_image};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM;

#[test]
fn the_menu_draws_and_redraws_only_on_change() {
    let (device, context, _) = create_device(None).unwrap();
    let mut panel = SettingsPanel::new(DXGI_FORMAT_R8G8B8A8_UNORM);
    let mut menu = SettingsMenu::parse("controller_smoothing=3,menu_reach=same,menu_reach_depth=15,controller_proxy=off");
    menu.open = true;
    let image = panel.draw(&device, &context, &menu).unwrap().expect("drawn").clone();
    assert!(panel.draw(&device, &context, &menu).unwrap().is_none(), "nothing changed");
    menu.selected = 1;
    assert!(panel.draw(&device, &context, &menu).unwrap().is_some(), "another line picked");
    let (width, height, pixels) = read_image(&device, &context, &image).unwrap();
    assert_eq!([width, height], PIXELS);
    let at = |x: u32, y: u32| pixels[((y * width + x) * 4 + 3) as usize];
    assert!(at(width / 2, height / 2) > 200, "the panel's background is nearly opaque");
    if let Some(path) = std::env::var_os("MONAKA_SETTINGS_PNG") {
        std::fs::write(path, monaka_core::png::rgba(width, height, &pixels)).unwrap();
    }
}
