//! `monaka --screenshot FILE.bmp [--game ID] [--tab NAME] [--scroll PIXELS] [--theme dark|light]`: draws the window off-screen without ever
//! activating it, saves it as a BMP and quits. For checking the layout (and README pictures)
//! while a game runs: nothing takes the game's focus, so it doesn't pause.

use std::path::{Path, PathBuf};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleBitmap, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits,
    HGDIOBJ, ReleaseDC, SelectObject,
};
use windows::Win32::Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow};
use windows::Win32::UI::WindowsAndMessaging::PW_RENDERFULLCONTENT;
use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

/// What the command line asked for.
pub struct Request {
    pub file: PathBuf,
    pub game: Option<String>,
    /// The options tab to show (Main, Controls, HUD, Skeleton, Rendering, Game, Monaka).
    pub tab: Option<String>,
    /// `--scroll`: how far the options are scrolled, when they overflow.
    pub scroll: i32,
    /// `--theme`: dark or light whatever Windows' app mode says.
    pub dark: Option<bool>,
}

pub fn request() -> Option<Request> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let after = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned();
    let dark = after("--theme").map(|t| t.eq_ignore_ascii_case("dark"));
    let scroll = after("--scroll").and_then(|s| s.parse().ok()).unwrap_or(0);
    Some(Request { file: PathBuf::from(after("--screenshot")?), game: after("--game"), tab: after("--tab"), scroll, dark })
}

/// Saves the whole window (frame included) as a 32-bit BMP.
pub fn save(hwnd: HWND, file: &Path) -> Result<(), String> {
    let mut rect = RECT::default();
    // SAFETY: our own window's rectangle.
    unsafe { GetWindowRect(hwnd, &mut rect) }.map_err(|e| e.to_string())?;
    let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
    let mut pixels = vec![0u8; (width * height * 4) as usize];
    // SAFETY: a memory DC and bitmap owned here, the window printed into it and read back as
    // top-down 32-bit rows into our buffer of exactly that size; everything freed after.
    unsafe {
        let screen = GetDC(None);
        let memory = CreateCompatibleDC(Some(screen));
        let bitmap = CreateCompatibleBitmap(screen, width, height);
        let old = SelectObject(memory, HGDIOBJ(bitmap.0));
        let printed = PrintWindow(hwnd, memory, PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT)).as_bool();
        SelectObject(memory, old);
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let rows = GetDIBits(memory, bitmap, 0, height as u32, Some(pixels.as_mut_ptr().cast()), &mut info, DIB_RGB_COLORS);
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        let _ = DeleteDC(memory);
        ReleaseDC(None, screen);
        if !printed || rows != height {
            return Err("the window could not be printed".into());
        }
    }
    // GetDIBits wrote top-down BGRA rows, as a 32-bit BMP stores them.
    let bmp = monaka_core::bmp::bgra(width as u32, height as u32, &pixels);
    std::fs::write(file, bmp).map_err(|e| format!("{}: {e}", file.display()))
}
