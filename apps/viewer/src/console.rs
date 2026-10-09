//! Process-level odds and ends that need `unsafe`: Ctrl+C handling, the launcher's stop event and
//! the runtime override.
#![allow(unsafe_code)]

use std::path::Path;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use windows::Win32::Foundation::{FALSE, HANDLE, TRUE, WAIT_OBJECT_0};
use windows::Win32::System::Console::{CTRL_BREAK_EVENT, CTRL_C_EVENT, SetConsoleCtrlHandler};
use windows::Win32::System::Threading::{OpenEventW, SYNCHRONIZATION_SYNCHRONIZE, WaitForSingleObject};
use windows::core::{BOOL, HSTRING};

static STOP: AtomicBool = AtomicBool::new(false);
/// The launcher's stop event (`--stop-event`), as a raw handle value; kept open for the process.
static STOP_EVENT: OnceLock<isize> = OnceLock::new();

unsafe extern "system" fn on_control(event: u32) -> BOOL {
    if event == CTRL_C_EVENT || event == CTRL_BREAK_EVENT {
        STOP.store(true, Ordering::Relaxed);
        TRUE
    } else {
        FALSE
    }
}

/// Ctrl+C and Ctrl+Break ask the viewer to end its session cleanly instead of killing it.
pub fn catch_ctrl_c() {
    // SAFETY: registers a handler that only touches an atomic.
    let _ = unsafe { SetConsoleCtrlHandler(Some(on_control), true) };
}

/// A named event that, once set, asks the viewer to end its session cleanly as Ctrl+C does: how the
/// `monaka` window, which has no console to send Ctrl+C through, stops the viewer.
pub fn watch_stop_event(name: &str) -> Result<(), String> {
    // SAFETY: opens an existing event for waiting; the handle stays open for the process.
    let event = unsafe { OpenEventW(SYNCHRONIZATION_SYNCHRONIZE, false, &HSTRING::from(name)) }.map_err(|e| format!("--stop-event {name}: {e}"))?;
    let _ = STOP_EVENT.set(event.0 as isize);
    Ok(())
}

pub fn stop_requested() -> bool {
    // SAFETY: a zero-timeout wait on the event handle opened above, never closed.
    let event_set = || STOP_EVENT.get().is_some_and(|&h| unsafe { WaitForSingleObject(HANDLE(h as *mut _), 0) } == WAIT_OBJECT_0);
    STOP.load(Ordering::Relaxed) || event_set()
}

pub fn request_stop() {
    STOP.store(true, Ordering::Relaxed);
}

/// Points the OpenXR loader at a runtime manifest for this process only.
pub fn set_runtime_override(manifest: &Path) {
    // SAFETY: called from main before any other thread exists and before the loader is loaded.
    unsafe { std::env::set_var("XR_RUNTIME_JSON", manifest) };
}
