use super::*;
use egui::Pos2;
use crate::home::tests::run_folder;
use monaka_channel::d3d::{create_device, read_image};
use monaka_contract::manifest::Values;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM;

/// Each panel draws: an opaque background with text on it, the cursor where the pointer is, and
/// nothing drawn again while nothing changed. `MONAKA_HOME_PNG=<folder>` saves them.
#[test]
fn the_panels_draw() {
    let folder = run_folder("pages", &Values::default());
    std::fs::write(folder.join("monaka.log"), (0..80).map(|i| format!("Line {i} of the run's log\n")).collect::<String>()).unwrap();
    let mut home = Home::load(Some(&folder));
    home.refresh(std::time::Instant::now());
    let (device, context, _) = create_device(None).unwrap();
    let mut pages = Pages::new(DXGI_FORMAT_R8G8B8A8_UNORM);
    for (page, which) in PAGES.iter().enumerate() {
        let events = vec![egui::Event::PointerMoved(Pos2::new(200.0, 150.0))];
        let image = pages.draw(page, &device, &context, &mut home, events).unwrap().expect("drawn").clone();
        let (width, height, pixels) = read_image(&device, &context, &image).unwrap();
        assert_eq!([width, height], PIXELS);
        let alpha = |x: u32, y: u32| pixels[((y * width + x) * 4 + 3) as usize];
        assert!(alpha(width / 2, height / 2) > 200, "{which:?}: the background is nearly opaque");
        assert_eq!(alpha(1, 1), 0, "{which:?}: its rounded corner is clear");
        let shades: std::collections::HashSet<u8> = pixels.chunks(4).map(|p| p[0]).collect();
        assert!(shades.len() > 20, "{which:?}: {} shades", shades.len());
        // The panel in front shows its whole outline, the bottom edge too: nothing runs off the image.
        if page == home.front {
            let rgb = |x: u32, y: u32| &pixels[((y * width + x) * 4) as usize..][..3];
            let bottom = rgb(width / 2, height - 3);
            assert!(bottom[0] > 200 && bottom[2] < 120, "{which:?}: the outline's bottom edge is {bottom:?}");
        }
        if let Some(out) = std::env::var_os("MONAKA_HOME_PNG") {
            let out = std::path::Path::new(&out).join(format!("home-{}.png", which.title().to_lowercase()));
            std::fs::write(out, monaka_core::png::rgba(width, height, &pixels)).unwrap();
        }
    }
    let _ = std::fs::remove_dir_all(&folder);
}

/// What a pointer press and release at `at` and `to` (points) do on panel `page`, the panel drawn
/// once first (egui finds what is under the pointer from the frame before).
fn click(home: &mut Home, page: usize, at: Pos2, to: Pos2) {
    let (device, context, _) = create_device(None).unwrap();
    let mut pages = Pages::new(DXGI_FORMAT_R8G8B8A8_UNORM);
    let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
    pages.draw(page, &device, &context, home, Vec::new()).unwrap();
    pages.draw(page, &device, &context, home, vec![egui::Event::PointerMoved(at), button(at, true)]).unwrap();
    let mut release = Vec::new();
    if to != at {
        release.push(egui::Event::PointerMoved(to));
    }
    release.push(button(to, false));
    pages.draw(page, &device, &context, home, release).unwrap();
}

/// The Settings panel's second tab, where it is drawn (points).
const SECOND_TAB: Pos2 = Pos2::new(131.0, 72.0);

#[test]
fn a_tab_switches_with_a_click() {
    let folder = run_folder("tab", &Values::default());
    let mut home = Home::load(Some(&folder));
    assert_eq!(home.tab, monaka_contract::manifest::Shown::Hud);
    click(&mut home, 1, SECOND_TAB, SECOND_TAB);
    assert_eq!(home.tab, monaka_contract::manifest::Shown::Rendering);
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn a_pointer_moved_between_press_and_release_is_a_drag_not_a_click() {
    // Why the viewer holds the pointer still while the trigger is down.
    let folder = run_folder("drag", &Values::default());
    let mut home = Home::load(Some(&folder));
    click(&mut home, 1, SECOND_TAB, SECOND_TAB + Vec2::new(12.0, 0.0));
    assert_eq!(home.tab, monaka_contract::manifest::Shown::Hud);
    let _ = std::fs::remove_dir_all(&folder);
}
