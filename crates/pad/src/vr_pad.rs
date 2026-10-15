//! The VR pad itself: the hand block the viewer writes it into ([`CHANNEL`]), whether it is handed
//! out ([`ACTIVE`], from the install to the stop), telling the game a device came when it goes live
//! ([`announce`]), and the pad for a reader other than XInput's ([`virtual_pad`]).

use crate::presses::{FLICK, HELD_BUTTONS, HELD_UNTIL_LET_GO, PRESSED, flick, pressed};
use crate::stick::{TAKE_RIGHT, takes_y};
use crate::xinput::PAD_MAX_AGE_MS;
use monaka_channel::ChannelName;
use monaka_channel::hands::HandChannel;
use monaka_core::pad::merge;
use monaka_contract::gamepad::Gamepad;
use monaka_core::sync::Locked;
use monaka_producer::{Rejection, log};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering::*};

pub(super) static ACTIVE: AtomicBool = AtomicBool::new(false);
/// Diagnostic: hand out the VR pad alone, the real pad's input dropped (does the game take the
/// real pad through these reads at all?).
pub(super) static MASK_REAL: AtomicBool = AtomicBool::new(false);

/// Diagnostic switch for [`MASK_REAL`].
pub fn mask_real(on: bool) {
    MASK_REAL.store(on, Relaxed);
}
pub(super) static CHANNEL: Mutex<Option<HandChannel>> = Mutex::new(None);

/// Opens the hand block beside `channel`, where the viewer writes the VR pad.
pub(super) fn open_block(channel: &ChannelName) -> Result<(), Rejection> {
    let block = HandChannel::open(channel).map_err(|e| Rejection::not_ready(format!("controller pad: hand block: {e}")))?;
    *CHANNEL.locked() = Some(block);
    Ok(())
}

/// A HID device interface arriving, for [`announce`]. Static: the posted message points at it.
pub(super) static ARRIVAL: windows::Win32::UI::WindowsAndMessaging::DEV_BROADCAST_DEVICEINTERFACE_W = windows::Win32::UI::WindowsAndMessaging::DEV_BROADCAST_DEVICEINTERFACE_W {
    dbcc_size: size_of::<windows::Win32::UI::WindowsAndMessaging::DEV_BROADCAST_DEVICEINTERFACE_W>() as u32,
    dbcc_devicetype: 5, // DBT_DEVTYP_DEVICEINTERFACE
    dbcc_reserved: 0,
    // GUID_DEVINTERFACE_HID
    dbcc_classguid: windows::core::GUID::from_u128(0x4d1e55b2_f16f_11cf_88cb_001111000030),
    dbcc_name: [0],
};

/// The thread [`announce`] starts, joined at [`stop`].
pub(super) static ANNOUNCER: Mutex<Option<std::thread::JoinHandle<()>>> = Mutex::new(None);

/// For games that look for pads only when a device comes: each time the VR pad goes live (the
/// viewer's controllers start sending), tells the game's windows a device came. Dishonored: Death
/// of the Outsider rescans its XInput pads on `WM_DEVICECHANGE` alone, so with no real pad plugged
/// in or out it never asks about the VR one; a rescan before the VR pad is live would
/// find pad 0 absent. Call after the pad hooks are on.
pub fn announce() {
    let watcher = std::thread::spawn(|| {
        let mut live = false;
        while ACTIVE.load(Acquire) {
            let now = vr_pad().is_some();
            if now && !live {
                post_arrival();
            }
            live = now;
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    });
    *ANNOUNCER.locked() = Some(watcher);
}

/// Tells the game's windows a device came with `WM_DEVICECHANGE`: posts "device nodes changed" and
/// sends a HID interface arrival. The arrival carries a pointer, so Windows refuses to post it
/// (a posted arrival reaches no window): it is sent, and waited for.
pub(super) fn post_arrival() {
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_DEVICECHANGE,
    };
    const DBT_DEVNODES_CHANGED: usize = 0x0007;
    const DBT_DEVICEARRIVAL: usize = 0x8000;

    unsafe extern "system" fn each(window: HWND, found: LPARAM) -> windows::core::BOOL {
        let mut process = 0;
        // SAFETY: reads a window's owner; `found` is the caller's list, alive for the enumeration.
        unsafe {
            GetWindowThreadProcessId(window, Some(&mut process));
            if process == GetCurrentProcessId() {
                (*(found.0 as *mut Vec<(HWND, bool)>)).push((window, IsWindowVisible(window).as_bool()));
            }
        }
        true.into()
    }
    let mut found: Vec<(HWND, bool)> = Vec::new();
    // SAFETY: the callback only reads window owners into `found`, which outlives the call.
    let _ = unsafe { EnumWindows(Some(each), LPARAM(&raw mut found as isize)) };
    let (mut posted, mut sent) = (0, 0);
    for &(window, _) in found.iter().filter(|(_, visible)| *visible) {
        // SAFETY: a window of this process; the arrival is a static the receiver only reads, and
        // the send waits for it (at most a second, and not on a hung window).
        unsafe {
            if PostMessageW(Some(window), WM_DEVICECHANGE, WPARAM(DBT_DEVNODES_CHANGED), LPARAM(0)).is_ok() {
                posted += 1;
            }
            let mut result = 0;
            let arrival = LPARAM(&raw const ARRIVAL as isize);
            if SendMessageTimeoutW(window, WM_DEVICECHANGE, WPARAM(DBT_DEVICEARRIVAL), arrival, SMTO_ABORTIFHUNG, 1000, Some(&mut result)).0 != 0 {
                sent += 1;
            }
        }
    }
    let visible = found.iter().filter(|(_, v)| *v).count();
    log!(
        "controller pad: the VR pad is live; told the game a device came ({} windows, {visible} visible; devices-changed posted to {posted}, arrival sent to {sent})",
        found.len()
    );
}

/// Stops handing out the VR pad (the hooks themselves go with the producer's).
pub fn stop() {
    ACTIVE.store(false, Release);
    HELD_BUTTONS.store(0, Relaxed);
    PRESSED.locked().retain(|(_, _, until)| *until != HELD_UNTIL_LET_GO);
    if let Ok(mut flick) = FLICK.lock() {
        flick.1 = 0;
    }
    if let Some(watcher) = ANNOUNCER.locked().take() {
        let _ = watcher.join();
    }
}

/// Closes the hand block after the hooks are off.
pub fn release() {
    *CHANNEL.locked() = None;
}

/// The VR pad, while the viewer keeps it fresh, its buttons as the game's VR button layout has them
/// ([`crate::remap`]).
pub(super) fn vr_pad() -> Option<Gamepad> {
    if !ACTIVE.load(Acquire) {
        return None;
    }
    let pad = CHANNEL.lock().ok()?.as_mut()?.read_pad(PAD_MAX_AGE_MS)?;
    Some(crate::remap::controllers(pad))
}

/// Whether the viewer keeps the VR pad fresh now (a producer can then tell its game a pad is there).
pub fn live() -> bool {
    vr_pad().is_some()
}

/// What the VR pad and the producer's own presses hold now, merged, for a reader other than
/// XInput's ([`gameinput`]); `None` while neither holds anything.
pub(crate) fn virtual_pad() -> Option<Gamepad> {
    let pressed = pressed();
    vr_pad().map(|vr| merge(vr, pressed)).or((pressed != Gamepad::default() && ACTIVE.load(Acquire)).then_some(pressed))
}

/// The right stick's takes ([`take_right_stick`], [`take_right_stick_y`]) and the producer's flick,
/// for a reader other than XInput's.
pub(crate) fn right_stick_rules() -> (bool, bool, Option<[i16; 2]>) {
    (TAKE_RIGHT.load(Acquire), takes_y(), flick())
}

