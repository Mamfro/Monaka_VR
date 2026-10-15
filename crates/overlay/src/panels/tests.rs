use super::*;
use crate::canvas::Canvas;
use crate::stats::{GameView, StatsView};
use monaka_channel::d3d::{create_device, read_image};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM;

#[test]
fn a_set_of_panels_switches_each_on_and_off() {
    let set = Panels::NONE.with(PanelId::Stats, true);
    assert!(set.has(PanelId::Stats));
    assert_eq!(set.iter().collect::<Vec<_>>(), [PanelId::Stats]);
    assert_eq!(set.with(PanelId::Stats, false), Panels::NONE);
}

#[test]
fn the_stats_panel_draws_in_the_headset() {
    let (device, context, _) = create_device(None).unwrap();
    let mut canvas = Canvas::new(DXGI_FORMAT_R8G8B8A8_UNORM, PanelId::Stats.pixels());
    let stats = StatsView {
        display: "Headset",
        display_fps: 90.0,
        display_ms: 11.1,
        worst_display_ms: 30.0,
        pairs_per_second: 45.0,
        eye_size: [2644, 2644],
        display_times: (0..90).map(|n| if n == 60 { 30.0 } else { 11.0 + (n % 7) as f32 * 0.2 }).collect(),
        eyes: Some(crate::stats::EyeAges { newest: [0.5, 0.5], age: [0.5, 0.5], together: 0.0 }),
        game: Some(GameView {
            frames: Some([90.0, 11.1, 14.0]),
            times: (0..90).map(|n| 11.1 + (n % 5) as f32 * 0.3).collect(),
            costs: [monaka_contract::debug_draw::CostTime { mean_ms: 0.42, worst_ms: 1.1 }; monaka_contract::debug_draw::COSTS],
            size: [3840, 2160],
            settings: "Stereo method: AFW (Frame-warp)\nFrame generation: Off".into(),
        }),
        ..StatsView::default()
    };
    let console = crate::console_view::ConsoleView::default();
    let content = Content { stats: &stats, console: &console, fake_vr: &crate::fake_vr::FakeVr::HEADSET };
    let image = canvas.draw(&device, &context, |ui| {
        headset_page(ui, PanelId::Stats, &content);
    }).unwrap().expect("drawn").clone();
    let (width, height, pixels) = read_image(&device, &context, &image).unwrap();
    assert_eq!([width, height], PanelId::Stats.pixels());
    let shades: std::collections::HashSet<u8> = pixels.chunks(4).map(|p| p[0]).collect();
    assert!(shades.len() > 20, "text drawn: {} shades", shades.len());
    if let Some(path) = std::env::var_os("MONAKA_STATS_PNG") {
        std::fs::write(path, monaka_core::png::rgba(width, height, &pixels)).unwrap();
    }
}

#[test]
fn a_panels_window_matches_the_menu_in_the_2d_window() {
    use crate::menu::{Menu, Settings, Status};
    let (device, context, _) = create_device(None).unwrap();
    let mut canvas = Canvas::new(DXGI_FORMAT_R8G8B8A8_UNORM, [1920, 1100]);
    let stats = StatsView { display: "Window", display_fps: 60.0, display_ms: 16.7, worst_display_ms: 17.5, eye_size: [1920, 2160], ..StatsView::default() };
    let console = crate::console_view::ConsoleView::default();
    let content = Content { stats: &stats, console: &console, fake_vr: &crate::fake_vr::FakeVr::HEADSET };
    let menu = Menu { tab: 4, ..Menu::default() };
    let settings = Settings { panels: Panels::NONE.with(PanelId::Stats, true), ..Settings::default() };
    let status = Status::default();
    let frame = |canvas: &mut Canvas| {
        canvas
            .draw(&device, &context, |ui| {
                let ctx = ui.ctx().clone();
                egui::Window::new("Monaka debug").title_bar(false).frame(egui::Frame::NONE).resizable(false).fixed_size([460.0, 520.0]).fixed_pos([12.0, 12.0]).show(&ctx, |ui| {
                    crate::menu::panel::layout(ui, &menu, &settings, &status, true, false);
                });
                window(PanelId::Stats).fixed_pos([500.0, 12.0]).show(&ctx, |ui| window_page(ui, PanelId::Stats, &content, &mut true));
            })
            .unwrap()
            .expect("drawn")
            .clone()
    };
    // egui sizes a new window unseen on its first frame.
    frame(&mut canvas);
    let image = frame(&mut canvas);
    let (width, height, pixels) = read_image(&device, &context, &image).unwrap();
    let at = |x: u32, y: u32| &pixels[((y * width + x) * 4) as usize..][..4];
    // Points 2 pixels each: inside the menu, inside the panel below its title.
    if let Some(path) = std::env::var_os("MONAKA_WINDOW_PNG") {
        // Over black, as the window shows it over the picture's bars.
        let opaque: Vec<u8> = pixels.chunks(4).flat_map(|p| [p[0], p[1], p[2], 255]).collect();
        std::fs::write(path, monaka_core::png::rgba(width, height, &opaque)).unwrap();
    }
    let (menu_bg, panel_bg) = (at(900, 1000), at(1040, 1200 / 2));
    assert!(menu_bg[3] > 200 && panel_bg[3] > 200, "both drawn: {menu_bg:?} {panel_bg:?}");
    assert_eq!(menu_bg, panel_bg, "the panel's background is the menu's");
}

/// The Console panel in the 2D window, with a console that has a probe in and has answered.
#[test]
fn the_console_panel_shows_its_status_log_and_buttons() {
    let (device, context, _) = create_device(None).unwrap();
    let mut canvas = Canvas::new(DXGI_FORMAT_R8G8B8A8_UNORM, [1280, 1100]);
    let stats = StatsView::default();
    let console = crate::console_view::ConsoleView {
        open: true,
        status: "1 probes in\n  0 arm_palette  180.0 a frame, 16 threads".into(),
        log: "> hook exe+0x25f930 arm_palette\nprobe 0: arm_palette at Dishonored2.exe+0x25f930\n> class 0x19aadb0b4e0\n.?AVarkSkinnedRenderModel@@\n".into(),
    };
    let content = Content { stats: &stats, console: &console, fake_vr: &crate::fake_vr::FakeVr::HEADSET };
    let frame = |canvas: &mut Canvas| {
        canvas
            .draw(&device, &context, |ui| {
                let ctx = ui.ctx().clone();
                window(PanelId::Console).fixed_pos([12.0, 12.0]).show(&ctx, |ui| window_page(ui, PanelId::Console, &content, &mut true));
            })
            .unwrap()
            .expect("drawn")
            .clone()
    };
    frame(&mut canvas);
    let image = frame(&mut canvas);
    let (width, height, pixels) = read_image(&device, &context, &image).unwrap();
    if let Some(path) = std::env::var_os("MONAKA_CONSOLE_PNG") {
        let opaque: Vec<u8> = pixels.chunks(4).flat_map(|p| [p[0], p[1], p[2], 255]).collect();
        std::fs::write(path, monaka_core::png::rgba(width, height, &opaque)).unwrap();
    }
    let drawn = pixels.chunks(4).filter(|p| p[3] > 200).count();
    assert!(drawn > 100_000, "the panel is drawn ({drawn} pixels)");
}

/// The Fake VR panel in the 2D window, the hands held forward.
#[test]
fn the_fake_vr_panel_shows_the_head_and_hands() {
    let (device, context, _) = create_device(None).unwrap();
    let mut canvas = Canvas::new(DXGI_FORMAT_R8G8B8A8_UNORM, [1280, 600]);
    let (stats, console, fake) = (StatsView::default(), crate::console_view::ConsoleView::default(), crate::fake_vr::FakeVr::desktop());
    let content = Content { stats: &stats, console: &console, fake_vr: &fake };
    let frame = |canvas: &mut Canvas| {
        canvas
            .draw(&device, &context, |ui| {
                let ctx = ui.ctx().clone();
                window(PanelId::FakeVr).fixed_pos([12.0, 12.0]).show(&ctx, |ui| window_page(ui, PanelId::FakeVr, &content, &mut true));
            })
            .unwrap()
            .expect("drawn")
            .clone()
    };
    frame(&mut canvas);
    let image = frame(&mut canvas);
    let (width, height, pixels) = read_image(&device, &context, &image).unwrap();
    if let Some(path) = std::env::var_os("MONAKA_FAKE_VR_PNG") {
        let opaque: Vec<u8> = pixels.chunks(4).flat_map(|p| [p[0], p[1], p[2], 255]).collect();
        std::fs::write(path, monaka_core::png::rgba(width, height, &opaque)).unwrap();
    }
    assert!(pixels.chunks(4).filter(|p| p[3] > 200).count() > 50_000, "the panel is drawn");
}

/// The Console panel in the headset: its title and its long log, the newest lines at the bottom.
#[test]
fn the_console_panel_in_the_headset_shows_its_log() {
    let (device, context, _) = create_device(None).unwrap();
    let mut canvas = Canvas::new(DXGI_FORMAT_R8G8B8A8_UNORM, PanelId::Console.pixels());
    let stats = StatsView::default();
    let console = crate::console_view::ConsoleView {
        open: true,
        status: "1 probes in\n  0 arm_palette  180.0 a frame, 16 threads".into(),
        log: (0..120).map(|n| format!("> line {n}: probe 0 at exe+0x{:x}\n", 0x25f930 + n)).collect(),
    };
    let content = Content { stats: &stats, console: &console, fake_vr: &crate::fake_vr::FakeVr::HEADSET };
    let image = canvas
        .draw(&device, &context, |ui| {
            headset_page(ui, PanelId::Console, &content);
        })
        .unwrap()
        .expect("drawn")
        .clone();
    let (width, height, pixels) = read_image(&device, &context, &image).unwrap();
    if let Some(path) = std::env::var_os("MONAKA_CONSOLE_HEADSET_PNG") {
        std::fs::write(path, monaka_core::png::rgba(width, height, &pixels)).unwrap();
    }
    let shades: std::collections::HashSet<u8> = pixels.chunks(4).map(|p| p[0]).collect();
    assert!(shades.len() > 20, "{} shades", shades.len());
}
