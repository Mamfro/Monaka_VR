use super::*;
use crate::menu::View;
use monaka_channel::d3d::{create_device, read_image};
use monaka_contract::debug_draw::{Categories, Category};

#[test]
fn the_menu_draws_and_redraws_only_on_change() {
    let (device, context, _) = create_device(None).unwrap();
    let mut panel = Panel::new(DXGI_FORMAT_R8G8B8A8_UNORM);
    let menu = Menu { tab: 0, selected: [1, 0, 0, 0, 0, 0], open: true, paged: false };
    let settings = Settings { wanted: Categories::NONE.with(Category::Skeleton, true), view: View::Depth, ..Settings::default() };
    let status = Status { offered: Categories::NONE.with(Category::Skeleton, true).with(Category::Controllers, true), synced: true, ..Status::default() };
    let image = panel.draw(&device, &context, &menu, &settings, &status, true, Vec::new()).unwrap().0.expect("drawn").clone();
    assert!(panel.draw(&device, &context, &menu, &settings, &status, true, Vec::new()).unwrap().0.is_none(), "nothing changed");
    let (width, height, pixels) = read_image(&device, &context, &image).unwrap();
    assert_eq!([width, height], PIXELS);
    let at = |x: u32, y: u32| pixels[((y * width + x) * 4 + 3) as usize];
    assert!(at(width / 2, height / 2) > 200, "the panel's background is nearly opaque");
    assert_eq!(at(1, 1), 0, "its rounded corner is clear");
    // Text drawn: more than one shade inside.
    let shades: std::collections::HashSet<u8> = pixels.chunks(4).map(|p| p[0]).collect();
    assert!(shades.len() > 20, "{} shades", shades.len());
    if let Some(path) = std::env::var_os("MONAKA_MENU_PNG") {
        std::fs::write(path, monaka_core::png::rgba(width, height, &pixels)).unwrap();
    }
}

#[test]
fn the_game_page_draws() {
    use monaka_contract::page::{Page, Row, Tone};
    let (device, context, _) = create_device(None).unwrap();
    let mut panel = Panel::new(DXGI_FORMAT_R8G8B8A8_UNORM);
    let mut rows = vec![
        Row::text("Next", "Unpause, stand still in gameplay, then A on Turn test").tone(Tone::Warn),
        Row::heading("The game"),
        Row::text("Dishonored2.exe", "Direct3D 11, 4297 frames"),
        Row::heading("Ladder"),
        Row::text("0  Screen", "the game on a screen in front of you").tone(Tone::Good),
        Row::text("1  Depth", "Views tab: Eyes show Depth, pick its buffer"),
        Row::text("2  Head look", "needs the camera: pick it below").tone(Tone::Dim),
        Row::heading("Camera"),
    ];
    for k in 0..6 {
        rows.push(Row::button(100 + k, &format!("view-projection +{}", 32 + 128 * k), "turned 51.8, eye -76.7 -14.6 32.6, fov 80x51, 100 copies, and more text than fits").tone(if k == 0 { Tone::Good } else { Tone::Dim }));
    }
    rows.push(Row::button(1, "Turn test", "A: moves the mouse right, then back"));
    rows.push(Row::text("Found", "19 turned: 51.8 degrees, 0.1297 per count").tone(Tone::Good));
    rows.push(Row::text("Field of view", "80.0 x 51.0 degrees, standard depth").tone(Tone::Good));
    rows.push(Row::text("Mouse", "0.1297 degrees per count").tone(Tone::Good));
    let status = Status { page: Page::new("Explorer", &rows), ..Status::default() };
    let menu = Menu { tab: crate::menu::PAGE_TAB, selected: [0, 0, 0, 0, 0, 14], open: true, paged: true };
    let image = panel.draw(&device, &context, &menu, &Settings::default(), &status, false, Vec::new()).unwrap().0.expect("drawn").clone();
    let (width, height, pixels) = read_image(&device, &context, &image).unwrap();
    let shades: std::collections::HashSet<u8> = pixels.chunks(4).map(|p| p[0]).collect();
    assert!(shades.len() > 20, "{} shades", shades.len());
    if let Some(path) = std::env::var_os("MONAKA_PAGE_PNG") {
        std::fs::write(path, monaka_core::png::rgba(width, height, &pixels)).unwrap();
    }
}
