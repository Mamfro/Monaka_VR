//! What the launcher asks Windows: running processes and their command lines, SteamVR's OpenXR
//! files, Steam's libraries, the Documents folder, and opening `steam://` links.

use crate::text;
use std::path::{Path, PathBuf};
use windows::Wdk::System::Threading::{NtQueryInformationProcess, ProcessCommandLineInformation};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT, STILL_ACTIVE, UNICODE_STRING};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS};
use windows::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW};
use windows::Win32::System::Threading::{GetExitCodeProcess, IsWow64Process, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW};
use windows::Win32::UI::Shell::{FOLDERID_Documents, KF_FLAG_DEFAULT, SHGetKnownFolderPath, ShellExecuteW};
use windows::Win32::UI::Input::KeyboardAndMouse::{INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VK_MENU};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, EnumWindows, GetForegroundWindow, GetWindowRect, GetWindowThreadProcessId, IsIconic, IsWindowVisible, SW_RESTORE, SW_SHOWNORMAL, SetForegroundWindow, ShowWindow,
};
use windows::core::{BOOL, HSTRING, PCWSTR, w};

/// A running process.
#[derive(Clone, Debug)]
pub struct Process {
    pub pid: u32,
    /// The executable's file name (`DyingLightGame.exe`).
    pub executable: String,
}

/// Every running process whose executable is one of `executables` (case-insensitive).
pub fn processes_named(executables: &[&str]) -> Vec<Process> {
    let mut found = Vec::new();
    // SAFETY: a Toolhelp snapshot read into a local entry, closed below.
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return found };
        let mut entry = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut more = Process32FirstW(snapshot, &mut entry).is_ok();
        while more {
            let end = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
            let executable = String::from_utf16_lossy(&entry.szExeFile[..end]);
            if executables.iter().any(|e| e.eq_ignore_ascii_case(&executable)) {
                found.push(Process { pid: entry.th32ProcessID, executable });
            }
            more = Process32NextW(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
    }
    found
}

/// The running processes of a game: those named one of `executables`, and of those, when the
/// game's Steam install folder is known, the ones whose executable lies under it (two games can
/// share an executable name: Kingdom Come: Deliverance and its sequel are both `KingdomCome.exe`).
/// When none lies there (a copy outside Steam), every one named so.
pub fn game_processes(executables: &[&str], steam_app: u32) -> Vec<Process> {
    let named = processes_named(executables);
    let Some(folder) = steam_install_folder(&steam_libraries(), steam_app) else { return named };
    // With its separator: `...\KingdomComeDeliverance` is also the start of `...\KingdomComeDeliverance2`.
    let folder = format!("{}\\", folder.to_string_lossy().to_ascii_lowercase().trim_end_matches('\\'));
    let inside: Vec<Process> = named.iter().filter(|p| process_path(p.pid).is_some_and(|path| path.to_string_lossy().to_ascii_lowercase().starts_with(&folder))).cloned().collect();
    if inside.is_empty() { named } else { inside }
}

/// Where process `pid`'s executable is, when it can be read.
pub fn process_path(pid: u32) -> Option<PathBuf> {
    let mut buffer = vec![0u16; 1024];
    let mut length = buffer.len() as u32;
    // SAFETY: opens a handle owned here, reads the image name into our buffer of `length`
    // characters, and closes the handle.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let read = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, windows::core::PWSTR(buffer.as_mut_ptr()), &mut length);
        let _ = CloseHandle(process);
        read.ok()?;
    }
    Some(PathBuf::from(String::from_utf16_lossy(&buffer[..length as usize])))
}

/// Whether the process `pid` is 32-bit (running under WOW64), when that can be read.
pub fn is_32_bit(pid: u32) -> Option<bool> {
    let mut wow64 = windows::core::BOOL(0);
    // SAFETY: opens a handle owned here, fills a local, and closes the handle.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let read = IsWow64Process(process, &mut wow64);
        let _ = CloseHandle(process);
        read.ok()?;
    }
    Some(wow64.as_bool())
}

/// Whether the process `pid` is still running.
pub fn alive(pid: u32) -> bool {
    // SAFETY: opens, queries and closes a handle owned here.
    unsafe {
        let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return false };
        let mut code = 0u32;
        let running = GetExitCodeProcess(process, &mut code).is_ok() && code == STILL_ACTIVE.0 as u32;
        let _ = CloseHandle(process);
        running
    }
}

/// The biggest visible top-level window of process `pid`: a game's own window.
fn main_window(pid: u32) -> Option<HWND> {
    struct Search {
        pid: u32,
        best: Option<(HWND, i64)>,
    }
    unsafe extern "system" fn visit(window: HWND, data: LPARAM) -> BOOL {
        // SAFETY: `data` is the search below, alive for the enumeration.
        let search = unsafe { &mut *(data.0 as *mut Search) };
        let mut owner = 0;
        let mut rect = RECT::default();
        // SAFETY: plain queries on a window handed to us.
        unsafe {
            GetWindowThreadProcessId(window, Some(&mut owner));
            if owner == search.pid && IsWindowVisible(window).as_bool() && GetWindowRect(window, &mut rect).is_ok() {
                let area = i64::from(rect.right - rect.left) * i64::from(rect.bottom - rect.top);
                if search.best.is_none_or(|(_, a)| area > a) {
                    search.best = Some((window, area));
                }
            }
        }
        BOOL(1)
    }
    let mut search = Search { pid, best: None };
    // SAFETY: the callback only touches `search` during the call.
    let _ = unsafe { EnumWindows(Some(visit), LPARAM(&mut search as *mut Search as isize)) };
    search.best.map(|(window, _)| window)
}

/// Whether a window of process `pid` has the keyboard (is the foreground window).
pub fn in_front(pid: u32) -> bool {
    let mut owner = 0;
    // SAFETY: plain queries.
    unsafe { GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut owner)) };
    owner == pid
}

/// Brings process `pid`'s main window to the front, so the game has the keyboard and does not
/// pause as a window in the background. Whether it is in front now.
pub fn focus(pid: u32) -> bool {
    if in_front(pid) {
        return true;
    }
    let Some(window) = main_window(pid) else { return false };
    // SAFETY: a window of the game's, and input events of our own.
    unsafe {
        if IsIconic(window).as_bool() {
            let _ = ShowWindow(window, SW_RESTORE);
        }
        if !SetForegroundWindow(window).as_bool() {
            // A process may take the front only right after input: an Alt tap first.
            let alt = |up: bool| INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VK_MENU, dwFlags: if up { KEYEVENTF_KEYUP } else { Default::default() }, ..Default::default() } },
            };
            SendInput(&[alt(false), alt(true)], size_of::<INPUT>() as i32);
            let _ = BringWindowToTop(window);
            let _ = SetForegroundWindow(window);
        }
    }
    in_front(pid)
}

/// The command line process `pid` was started with, when it can be read.
pub fn command_line(pid: u32) -> Option<String> {
    // The information is a UNICODE_STRING followed by its text; a command line is at most 32767
    // characters. u64s keep the buffer aligned for the header.
    let mut buffer = vec![0u64; (size_of::<UNICODE_STRING>() + 2 * 32_768).div_ceil(8)];
    let bytes = (buffer.len() * 8) as u32;
    // SAFETY: opens, queries and closes a handle owned here; the query writes at most `bytes`
    // into our buffer, and its string points inside that buffer, `Length` bytes long.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut written = 0u32;
        let status = NtQueryInformationProcess(process, ProcessCommandLineInformation, buffer.as_mut_ptr().cast(), bytes, &mut written);
        let _ = CloseHandle(process);
        if status.is_err() {
            return None;
        }
        let string = &*buffer.as_ptr().cast::<UNICODE_STRING>();
        if string.Buffer.is_null() {
            return Some(String::new());
        }
        Some(String::from_utf16_lossy(std::slice::from_raw_parts(string.Buffer.0, usize::from(string.Length) / 2)))
    }
}

/// The player's Documents folder (wherever it was moved, OneDrive included).
pub fn documents() -> Option<PathBuf> {
    // SAFETY: the returned string is ours to read and free with CoTaskMemFree.
    unsafe {
        let path = SHGetKnownFolderPath(&FOLDERID_Documents, KF_FLAG_DEFAULT, None).ok()?;
        let text = path.to_string().ok();
        CoTaskMemFree(Some(path.0 as *const _));
        text.map(PathBuf::from)
    }
}

/// Whether a VR session is already running: a viewer, from this launcher or a PowerShell one. A
/// second session would stop the first one's producer and fight its viewer.
pub fn viewer_running() -> bool {
    !processes_named(&["monaka_viewer.exe"]).is_empty()
}

/// Whether SteamVR is running (its `vrserver`).
pub fn steamvr_running() -> bool {
    !processes_named(&["vrserver.exe"]).is_empty()
}

/// SteamVR's OpenXR loader and runtime manifest, which the viewer is pointed at.
pub struct OpenXr {
    pub loader: PathBuf,
    pub runtime: PathBuf,
}

/// Finds SteamVR through `%LOCALAPPDATA%\openvr\openvrpaths.vrpath`.
pub fn steamvr_openxr() -> Result<OpenXr, String> {
    let local = std::env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA is not set")?;
    let paths = Path::new(&local).join(r"openvr\openvrpaths.vrpath");
    let json = std::fs::read_to_string(&paths).map_err(|_| "SteamVR is not installed (no openvrpaths.vrpath). Install SteamVR from Steam.".to_owned())?;
    let steamvr = PathBuf::from(text::vrpath_runtime(&json).ok_or("SteamVR's install folder is not listed in openvrpaths.vrpath")?);
    let openxr = OpenXr { loader: steamvr.join(r"bin\win64\openxr_loader.dll"), runtime: steamvr.join("steamxr_win64.json") };
    for path in [&openxr.loader, &openxr.runtime] {
        if !path.exists() {
            return Err(format!("Missing SteamVR component: {}", path.display()));
        }
    }
    Ok(openxr)
}

/// Steam's install folder, from the registry.
fn steam_folder() -> Option<PathBuf> {
    registry_text(HKEY_CURRENT_USER, w!(r"Software\Valve\Steam"), w!("SteamPath")).map(PathBuf::from)
}

/// Windows' version and build (`24H2, build 26100.4061`), for logs.
pub fn windows_build() -> String {
    let key = w!(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion");
    let build = registry_text(HKEY_LOCAL_MACHINE, key, w!("CurrentBuild")).unwrap_or_else(|| "?".into());
    let update = registry_number(HKEY_LOCAL_MACHINE, key, w!("UBR")).map_or_else(String::new, |ubr| format!(".{ubr}"));
    let release = registry_text(HKEY_LOCAL_MACHINE, key, w!("DisplayVersion")).map_or_else(String::new, |r| format!("{r}, "));
    format!("{release}build {build}{update}")
}

fn registry_text(root: HKEY, key: PCWSTR, value: PCWSTR) -> Option<String> {
    let mut buffer = vec![0u16; 1024];
    let mut bytes = (buffer.len() * 2) as u32;
    // SAFETY: reads a string value into our buffer, whose size in bytes is passed.
    unsafe { RegGetValueW(root, key, value, RRF_RT_REG_SZ, None, Some(buffer.as_mut_ptr().cast()), Some(&mut bytes)) }.ok().ok()?;
    let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    Some(String::from_utf16_lossy(&buffer[..end]))
}

fn registry_number(root: HKEY, key: PCWSTR, value: PCWSTR) -> Option<u32> {
    let mut number = 0u32;
    let mut bytes = size_of::<u32>() as u32;
    // SAFETY: reads a DWORD value into `number`, whose size in bytes is passed.
    unsafe { RegGetValueW(root, key, value, RRF_RT_REG_DWORD, None, Some((&raw mut number).cast()), Some(&mut bytes)) }.ok().ok()?;
    Some(number)
}

/// Every Steam library folder (including Steam's own).
pub fn steam_libraries() -> Vec<PathBuf> {
    let Some(steam) = steam_folder() else { return Vec::new() };
    let mut libraries = std::fs::read_to_string(steam.join(r"steamapps\libraryfolders.vdf"))
        .map(|vdf| text::vdf_library_paths(&vdf).into_iter().map(PathBuf::from).collect::<Vec<_>>())
        .unwrap_or_default();
    if libraries.is_empty() {
        libraries.push(steam);
    }
    libraries
}

/// Whether the Steam app `app` is installed in any of `libraries`.
pub fn steam_installed(libraries: &[PathBuf], app: u32) -> bool {
    libraries.iter().any(|library| library.join(format!(r"steamapps\appmanifest_{app}.acf")).exists())
}

/// The folder Steam installed app `app` in (`steamapps\common\<installdir>`), when its app
/// manifest is in one of `libraries`.
pub fn steam_install_folder(libraries: &[PathBuf], app: u32) -> Option<PathBuf> {
    libraries.iter().find_map(|library| {
        let acf = std::fs::read_to_string(library.join(format!(r"steamapps\appmanifest_{app}.acf"))).ok()?;
        Some(library.join(r"steamapps\common").join(text::acf_install_dir(&acf)?))
    })
}

/// Opens a link with its registered handler (`steam://rungameid/...`, a folder, a file).
pub fn open(target: &str) -> Result<(), String> {
    // SAFETY: ShellExecuteW with owned strings; no window owner.
    let result = unsafe { ShellExecuteW(None, w!("open"), &HSTRING::from(target), None, None, SW_SHOWNORMAL) };
    // ShellExecute reports success as a value above 32.
    if result.0 as usize > 32 { Ok(()) } else { Err(format!("Could not open {target}")) }
}

/// Starts a Steam game.
pub fn launch_steam_game(app: u32) -> Result<(), String> {
    open(&format!("steam://rungameid/{app}"))
}

#[cfg(test)]
mod tests;
