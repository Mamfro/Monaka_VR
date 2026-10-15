//! What the window asks of the PC beyond its own files: whether SteamVR, another session's viewer
//! and the game run, which games Steam has installed, the player's pick of a game pack file, and
//! starting things through Steam or the shell. [`ThisPc`] asks Windows; the window's tests answer
//! for it.

use monaka_launch::system;
use std::path::PathBuf;
use windows::Win32::UI::Controls::Dialogs::{GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST, OPENFILENAMEW};
use windows::Win32::UI::Input::KeyboardAndMouse::GetActiveWindow;
use windows::core::{HSTRING, PCWSTR};

/// What runs, from one look at the processes.
#[derive(Clone, Copy, Debug, Default)]
pub struct Running {
    pub steamvr: bool,
    /// Another session's viewer (not the window's own headset view).
    pub viewer: bool,
    /// One of the game's executables.
    pub game: bool,
}

pub trait Pc {
    /// SteamVR, a viewer other than `ours` (the window's headset view), and a process named one of
    /// `game` (the game's executables).
    fn running(&self, game: &[&str], ours: Option<u32>) -> Running;
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
    fn running(&self, game: &[&str], ours: Option<u32>) -> Running {
        let mut names = vec![system::STEAMVR_EXE, system::VIEWER_EXE];
        names.extend_from_slice(game);
        let found = system::processes_named(&names);
        let any = |wanted: &[&str]| found.iter().any(|p| wanted.iter().any(|w| w.eq_ignore_ascii_case(&p.executable)));
        // Whether the game runs needs only a process of its name: which copy it is (by its Steam
        // folder) matters at the start, not here.
        let viewer = found.iter().any(|p| p.executable.eq_ignore_ascii_case(system::VIEWER_EXE) && Some(p.pid) != ours);
        Running { steamvr: any(&[system::STEAMVR_EXE]), viewer, game: any(game) }
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
