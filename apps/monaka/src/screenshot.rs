//! `monaka --screenshot FILE.bmp [--game ID] [--tab NAME|About] [--scroll PIXELS] [--theme dark|light]`:
//! draws the window off-screen without ever activating it, saves its inside as a BMP and quits. For
//! checking the layout while a game runs: nothing takes the game's focus, so it doesn't pause.

use eframe::egui::ColorImage;
use std::path::{Path, PathBuf};

/// What the command line asked for.
pub struct Request {
    pub file: PathBuf,
    pub game: Option<String>,
    /// The options tab to show (Main, Controls, HUD, Skeleton, Rendering, Game, Monaka), or About.
    pub tab: Option<String>,
    /// `--scroll`: how far the options are scrolled, when they overflow.
    pub scroll: f32,
    /// `--theme`: dark or light whatever Windows' app mode says.
    pub dark: Option<bool>,
}

pub fn request() -> Option<Request> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let after = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned();
    let dark = after("--theme").map(|t| t.eq_ignore_ascii_case("dark"));
    let scroll = after("--scroll").and_then(|s| s.parse().ok()).unwrap_or(0.0);
    Some(Request { file: PathBuf::from(after("--screenshot")?), game: after("--game"), tab: after("--tab"), scroll, dark })
}

/// Saves egui's picture of the window as a 32-bit BMP.
pub fn save(image: &ColorImage, file: &Path) -> Result<(), String> {
    let [width, height] = image.size;
    let pixels: Vec<u8> = image.pixels.iter().flat_map(|p| [p.b(), p.g(), p.r(), 255]).collect();
    let bmp = monaka_core::bmp::bgra(width as u32, height as u32, &pixels);
    std::fs::write(file, bmp).map_err(|e| format!("{}: {e}", file.display()))
}
