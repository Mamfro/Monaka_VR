//! Reloading the game packs when asked from outside the window: `monaka.exe --reload-packs` (after
//! a pack is rebuilt) sets a named event the open window watches ([`Watch`]), then exits without a
//! window of its own.

use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{CreateEventW, EVENT_MODIFY_STATE, OpenEventW, SetEvent, WaitForSingleObject};
use windows::core::HSTRING;

/// The argument that asks the open window to reload its packs.
pub const ARG: &str = "--reload-packs";
const EVENT: &str = r"Local\monaka-reload-packs";

/// The open window's end of the event.
pub struct Watch(HANDLE);

impl Watch {
    /// The event, made (or joined, with another window open) for this window; None when it cannot
    /// be: the button still reloads.
    pub fn new() -> Option<Watch> {
        // SAFETY: an auto-reset event, initially clear, owned by the returned value.
        unsafe { CreateEventW(None, false, false, &HSTRING::from(EVENT)) }.ok().map(Watch)
    }

    /// Whether a reload was asked for since the last look (the look clears it).
    pub fn asked(&self) -> bool {
        // SAFETY: a zero wait on the event this value owns.
        let waited = unsafe { WaitForSingleObject(self.0, 0) };
        waited == WAIT_OBJECT_0
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        // SAFETY: closes the handle this value owns.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// Asks the open window to reload its packs; whether one was open to ask.
pub fn ask() -> bool {
    // SAFETY: opens the event by name to set it, and closes it.
    unsafe {
        let Ok(event) = OpenEventW(EVENT_MODIFY_STATE, false, &HSTRING::from(EVENT)) else { return false };
        let set = SetEvent(event).is_ok();
        let _ = CloseHandle(event);
        set
    }
}
