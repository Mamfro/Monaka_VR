//! The game never hears that its window lost focus while VR runs (`keep_focus`), as a ready-made
//! component ([`keep_focus`]): many games pause whenever their window loses it, and the player is
//! in the headset, the monitor's window often behind another. The game window's procedure is
//! subclassed: the messages that tell it it was deactivated (`WM_ACTIVATEAPP` off, `WM_ACTIVATE`
//! inactive, `WM_KILLFOCUS`) stop there, everything else goes on to the game's own; it is put back
//! at the give-back.

use monaka_contract::manifest::{Kind, Opt, opt};
use monaka_producer::game::{Component, Game, Switch};
use monaka_producer::log;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, EnumWindows, GWLP_WNDPROC, GetClientRect, GetWindowLongPtrW, GetWindowThreadProcessId, IsWindowVisible, SetWindowLongPtrW, WA_INACTIVE, WM_ACTIVATE,
    WM_ACTIVATEAPP, WM_KILLFOCUS, WNDPROC,
};
use windows::core::BOOL;

const KEEP_FOCUS: Opt = opt("keep_focus", "Keep playing in the background", "The game does not pause when its window loses focus.", Kind::Switch { default: true });

/// The game keeps playing when its window loses focus (`keep_focus`), for game `G`: ours in front
/// of the game window's procedure from the arm, the window's own back at the give-back.
pub const fn keep_focus<G: Game>() -> Component<G> {
    Component {
        name: "keep focus",
        options: &[KEEP_FOCUS],
        switch: Some(Switch::new("keep_focus", true, "the game pauses when its window loses focus")),
        arm: install,
        give_back: uninstall,
        ..Component::NONE
    }
}

/// The window's own procedure, while ours stands in front of it.
static ORIGINAL: AtomicUsize = AtomicUsize::new(0);
static WINDOW: AtomicUsize = AtomicUsize::new(0);
/// Deactivation messages kept from the game.
static KEPT: AtomicU64 = AtomicU64::new(0);

unsafe extern "system" fn procedure(window: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    let deactivated = match message {
        WM_ACTIVATEAPP => w.0 == 0,
        WM_ACTIVATE => (w.0 & 0xffff) as u32 == WA_INACTIVE,
        WM_KILLFOCUS => true,
        _ => false,
    };
    if deactivated {
        KEPT.fetch_add(1, Ordering::Relaxed);
        return LRESULT(0);
    }
    // SAFETY: the window's own procedure, as it was before ours.
    unsafe {
        let original: WNDPROC = std::mem::transmute(ORIGINAL.load(Ordering::Acquire));
        CallWindowProcW(original, window, message, w, l)
    }
}

/// The process's largest visible top-level window: the game's.
fn game_window() -> Option<HWND> {
    struct Found {
        process: u32,
        best: Option<(HWND, i64)>,
    }
    unsafe extern "system" fn each(window: HWND, data: LPARAM) -> BOOL {
        // SAFETY: `data` is our `Found`, alive for the enumeration.
        let found = unsafe { &mut *(data.0 as *mut Found) };
        let mut process = 0u32;
        let mut client = RECT::default();
        // SAFETY: plain queries on a window handle the enumeration gave.
        unsafe {
            GetWindowThreadProcessId(window, Some(&mut process));
            if process == found.process && IsWindowVisible(window).as_bool() && GetClientRect(window, &mut client).is_ok() {
                let area = i64::from(client.right - client.left) * i64::from(client.bottom - client.top);
                if found.best.is_none_or(|(_, best)| area > best) {
                    found.best = Some((window, area));
                }
            }
        }
        BOOL(1)
    }
    // SAFETY: the current process's id.
    let mut found = Found { process: unsafe { GetCurrentProcessId() }, best: None };
    // SAFETY: enumerates top-level windows with a callback that only reads them.
    let _ = unsafe { EnumWindows(Some(each), LPARAM(&mut found as *mut Found as isize)) };
    found.best.map(|(window, _)| window)
}

/// Puts our procedure in front of the game window's.
fn install() {
    if ORIGINAL.load(Ordering::Acquire) != 0 {
        return;
    }
    let Some(window) = game_window() else { return log!("focus: no game window found; the game still pauses when it loses focus") };
    // SAFETY: subclasses a window of this process; ours forwards everything but deactivation.
    let previous = unsafe { SetWindowLongPtrW(window, GWLP_WNDPROC, procedure as *const () as usize as _) };
    if previous == 0 {
        return log!("focus: the window's procedure could not be replaced");
    }
    ORIGINAL.store(previous as usize, Ordering::Release);
    WINDOW.store(window.0 as usize, Ordering::Release);
    log!("focus: the game window {:#x} no longer hears it loses focus", window.0 as usize);
}

/// Puts the window's own procedure back (if ours is still the one in front).
fn uninstall() {
    let original = ORIGINAL.swap(0, Ordering::AcqRel);
    if original == 0 {
        return;
    }
    let window = HWND(WINDOW.load(Ordering::Acquire) as *mut _);
    // SAFETY: queries and restores the procedure of a window of this process.
    unsafe {
        if GetWindowLongPtrW(window, GWLP_WNDPROC) as usize == procedure as *const () as usize {
            SetWindowLongPtrW(window, GWLP_WNDPROC, original as _);
        } else {
            log!("focus: someone subclassed the window after us; ours stays (it forwards)");
            ORIGINAL.store(original, Ordering::Release);
        }
    }
    log!("focus: {} deactivations kept from the game", KEPT.load(Ordering::Relaxed));
}
