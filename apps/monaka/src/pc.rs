//! What the window asks of the PC beyond its own files: whether SteamVR, another session's viewer
//! and the game run, which games Steam has installed, the player's pick of a game pack file, and
//! starting things through Steam or the shell. [`ThisPc`] asks Windows; the window's tests answer
//! for it.

use monaka_launch::system;
use std::path::PathBuf;
use windows::Win32::UI::Controls::Dialogs::{GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST, OPENFILENAMEW};
use windows::Win32::UI::Input::KeyboardAndMouse::GetActiveWindow;
use windows::core::{HSTRING, PCWSTR};

pub trait Pc {
    fn steamvr_running(&self) -> bool;
    /// Another session's viewer is running.
    fn viewer_running(&self) -> bool;
    /// One of `executables` runs as the game (Steam app `steam_app`).
    fn game_running(&self, executables: &[&str], steam_app: u32) -> bool;
    /// The Steam library folders; none without Steam.
    fn steam_libraries(&self) -> Vec<PathBuf>;
    fn steam_installed(&self, libraries: &[PathBuf], steam_app: u32) -> bool;
    /// The player's pick of a game pack file (a .zip, or a lone .dll), if any.
    fn pick_pack_file(&self) -> Option<PathBuf>;
    fn launch_steam_game(&self, steam_app: u32) -> Result<(), String>;
    /// Opens a folder or a web page as the shell does.
    fn open(&self, target: &str) -> Result<(), String>;
}

/// The PC the window runs on.
pub struct ThisPc;

impl Pc for ThisPc {
    fn steamvr_running(&self) -> bool {
        system::steamvr_running()
    }

    fn viewer_running(&self) -> bool {
        system::viewer_running()
    }

    fn game_running(&self, executables: &[&str], steam_app: u32) -> bool {
        !system::game_processes(executables, steam_app).is_empty()
    }

    fn steam_libraries(&self) -> Vec<PathBuf> {
        system::steam_libraries()
    }

    fn steam_installed(&self, libraries: &[PathBuf], steam_app: u32) -> bool {
        system::steam_installed(libraries, steam_app)
    }

    fn pick_pack_file(&self) -> Option<PathBuf> {
        let mut file = vec![0u16; 1024];
        let filter: Vec<u16> = "Game packs (*.zip, *.dll)\0*.zip;*.dll\0\0".encode_utf16().collect();
        let title = HSTRING::from("Add game pack");
        let mut dialog = OPENFILENAMEW {
            lStructSize: size_of::<OPENFILENAMEW>() as u32,
            // The button was just pressed, so ours is the active window: the dialog stays over it.
            // SAFETY: reads this thread's active window.
            hwndOwner: unsafe { GetActiveWindow() },
            lpstrFilter: PCWSTR(filter.as_ptr()),
            lpstrFile: windows::core::PWSTR(file.as_mut_ptr()),
            nMaxFile: file.len() as u32,
            lpstrTitle: PCWSTR(title.as_ptr()),
            Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
            ..Default::default()
        };
        // SAFETY: the dialog writes the chosen path into our buffer, which outlives the call.
        if !unsafe { GetOpenFileNameW(&mut dialog) }.as_bool() {
            return None;
        }
        let length = file.iter().position(|&c| c == 0).unwrap_or(file.len());
        Some(PathBuf::from(String::from_utf16_lossy(&file[..length])))
    }

    fn launch_steam_game(&self, steam_app: u32) -> Result<(), String> {
        system::launch_steam_game(steam_app)
    }

    fn open(&self, target: &str) -> Result<(), String> {
        system::open(target)
    }
}
