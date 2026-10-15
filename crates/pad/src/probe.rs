//! Which input APIs a game reads, and from which of its modules: DirectInput device reads,
//! Raw Input records and HID report parsing, counted by caller and kind (read-only). For finding
//! the API a game takes a real pad from, before virtualising that API.

use monaka_hook::module::Module;
use windows::Win32::System::Diagnostics::Debug::RtlCaptureStackBackTrace;
use monaka_hook::{Hooks, InFlight, Original};
use monaka_producer::log;
use std::ffi::c_void;
use std::sync::Mutex;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::core::GUID;

static TALLY: Mutex<Vec<(String, u64)>> = Mutex::new(Vec::new());

/// Counts one call of kind `what` from the caller of the hooked API; logs the first of each.
fn note(what: impl FnOnce(&str) -> String) {
    thread_local! {
        static INSIDE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    // A hooked call made while noting one (file and driver calls are everywhere) is not noted.
    if INSIDE.with(|i| i.replace(true)) {
        return;
    }
    note_once(what);
    INSIDE.with(|i| i.set(false));
}

/// The callers on the stack outside this module, nearest first: their module names, Windows'
/// own plumbing (kernel32, kernelbase, ntdll) skipped, up to four, joined by " < ".
fn callers() -> (usize, String) {
    let own = Module::containing(callers as fn() -> (usize, String) as usize);
    let mut frames = [std::ptr::null_mut(); 32];
    // SAFETY: fills our buffer with return addresses.
    let captured = unsafe { RtlCaptureStackBackTrace(0, &mut frames, None) } as usize;
    let mut names: Vec<String> = Vec::new();
    let mut first = 0;
    for &frame in &frames[..captured.min(frames.len())] {
        let frame = frame as usize;
        if own.as_ref().is_some_and(|m| m.contains(frame)) {
            continue;
        }
        let name = Module::containing(frame).and_then(|m| m.path()).and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_else(|| "?".into());
        let plumbing = ["kernel32.dll", "kernelbase.dll", "ntdll.dll"].iter().any(|p| name.eq_ignore_ascii_case(p));
        if plumbing || names.last() == Some(&name) {
            continue;
        }
        if first == 0 {
            first = frame;
        }
        names.push(name);
        if names.len() == 4 {
            break;
        }
    }
    (first, if names.is_empty() { "?".into() } else { names.join(" < ") })
}

fn note_once(what: impl FnOnce(&str) -> String) {
    let (site, module) = callers();
    let key = what(&module);
    let Ok(mut tally) = TALLY.lock() else { return };
    if let Some((_, n)) = tally.iter_mut().find(|(k, _)| *k == key) {
        *n += 1;
    } else if tally.len() < 64 {
        log!("input probe: first {key} (from {})", Module::describe(site));
        tally.push((key, 1));
    }
}

pub fn report() {
    let Ok(tally) = TALLY.lock() else { return };
    log!("input probe: {} kinds of input reads seen", tally.len());
    for (key, n) in tally.iter() {
        log!("  {n:>8}x {key}");
    }
}

// ---- DirectInput: the device methods, found from a device of our own (every device type of the
// ---- ANSI and the wide interface runs the same code in dinput8.dll).

pub(crate) type GetDeviceStateFn = unsafe extern "system" fn(*mut c_void, u32, *mut c_void) -> i32;
pub(crate) type GetDeviceDataFn = unsafe extern "system" fn(*mut c_void, u32, *mut c_void, *mut u32, u32) -> i32;
type PollFn = unsafe extern "system" fn(*mut c_void) -> i32;
pub(crate) type CreateFn = unsafe extern "system" fn(*mut c_void, u32, *const GUID, *mut *mut c_void, *mut c_void) -> i32;
type CreateDeviceFn = unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void, *mut c_void) -> i32;
type ReleaseFn = unsafe extern "system" fn(*mut c_void) -> u32;

pub(crate) const IID_DIRECTINPUT8_A: GUID = GUID::from_u128(0xbf798030_483a_4da2_aa99_5d64ed369700);
pub(crate) const IID_DIRECTINPUT8_W: GUID = GUID::from_u128(0xbf798031_483a_4da2_aa99_5d64ed369700);
const GUID_SYS_MOUSE: GUID = GUID::from_u128(0x6f1d2b60_d5a0_11cf_bfc7_444553540000);
/// IDirectInputDevice8 slots: GetDeviceState, GetDeviceData, Poll.
const SLOT_GET_STATE: usize = 9;
const SLOT_GET_DATA: usize = 10;
const SLOT_POLL: usize = 25;

/// How many DirectInput method sets are watched (the wide and the ANSI interface, when they differ).
const DI_SETS: usize = 2;
/// Each set's originals, by its number.
static DI_STATE: [Original<GetDeviceStateFn>; DI_SETS] = [const { Original::new() }; DI_SETS];
static DI_DATA: [Original<GetDeviceDataFn>; DI_SETS] = [const { Original::new() }; DI_SETS];
static DI_POLL: [Original<PollFn>; DI_SETS] = [const { Original::new() }; DI_SETS];

/// Set `N`'s watch on `GetDeviceState`.
unsafe extern "system" fn di_state<const N: usize>(device: *mut c_void, size: u32, out: *mut c_void) -> i32 {
    let flight = InFlight::enter();
    // SAFETY: forwards the game's own call.
    let result = unsafe { DI_STATE[N].get(&flight)(device, size, out) };
    // The state size tells the device kind: 16/20 mouse, 256 keyboard, 80/272 joystick.
    note(|m| format!("DirectInput GetDeviceState size {size} by {m}"));
    result
}

/// Set `N`'s watch on `GetDeviceData`.
unsafe extern "system" fn di_data<const N: usize>(device: *mut c_void, size: u32, out: *mut c_void, count: *mut u32, flags: u32) -> i32 {
    let flight = InFlight::enter();
    // SAFETY: forwards the game's own call.
    let result = unsafe { DI_DATA[N].get(&flight)(device, size, out, count, flags) };
    // SAFETY: the game's in-out count, read after the call filled it.
    let got = if count.is_null() { 0 } else { unsafe { *count } };
    note(|m| format!("DirectInput GetDeviceData (device {device:p}{}) by {m}", if got > 0 { ", with data" } else { "" }));
    result
}

/// Set `N`'s watch on `Poll`.
unsafe extern "system" fn di_poll<const N: usize>(device: *mut c_void) -> i32 {
    let flight = InFlight::enter();
    note(|m| format!("DirectInput Poll (device {device:p}) by {m}"));
    // SAFETY: forwards the game's own call.
    unsafe { DI_POLL[N].get(&flight)(device) }
}

/// The device method addresses of DirectInput 8 for interface `iid` (a mouse device of our own,
/// released at once).
pub(crate) fn di_methods(create: CreateFn, iid: &GUID) -> Option<[usize; 3]> {
    // SAFETY: the game's executable module handle.
    let instance = unsafe { GetModuleHandleW(None) }.ok()?;
    let mut input: *mut c_void = std::ptr::null_mut();
    // SAFETY: DirectInput8Create(hinst, version, riid, out, outer).
    if unsafe { create(instance.0, 0x0800, iid, &mut input, std::ptr::null_mut()) } < 0 || input.is_null() {
        return None;
    }
    let call = |object: *mut c_void, slot: usize| -> Option<usize> { monaka_hook::mem::read::<usize>(monaka_hook::mem::read::<usize>(object as usize)? + slot * size_of::<usize>()) };
    let mut methods = None;
    let mut device: *mut c_void = std::ptr::null_mut();
    if let Some(create_device) = call(input, 3) {
        // SAFETY: IDirectInput8::CreateDevice(this, guid, out, outer).
        let made = unsafe { std::mem::transmute::<usize, CreateDeviceFn>(create_device)(input, &GUID_SYS_MOUSE, &mut device, std::ptr::null_mut()) };
        if made >= 0 && !device.is_null() {
            methods = (|| Some([call(device, SLOT_GET_STATE)?, call(device, SLOT_GET_DATA)?, call(device, SLOT_POLL)?]))();
            if let Some(release) = call(device, 2) {
                // SAFETY: releases our device.
                unsafe { std::mem::transmute::<usize, ReleaseFn>(release)(device) };
            }
        }
    }
    if let Some(release) = call(input, 2) {
        // SAFETY: releases our DirectInput object.
        unsafe { std::mem::transmute::<usize, ReleaseFn>(release)(input) };
    }
    methods
}

// ---- Raw Input and HID.

type GetRawInputDataFn = unsafe extern "system" fn(*mut c_void, u32, *mut c_void, *mut u32, u32) -> u32;
type GetRawInputBufferFn = unsafe extern "system" fn(*mut c_void, *mut u32, u32) -> u32;
type Args8 = unsafe extern "system" fn(u64, u64, u64, u64, u64, u64, u64, u64) -> u64;
type Args6 = unsafe extern "system" fn(u64, u64, u64, u64, u64, u64) -> u64;
type Args3 = unsafe extern "system" fn(u64, u64, u64) -> u64;

static RAW_DATA: Original<GetRawInputDataFn> = Original::new();
static RAW_BUFFER: Original<GetRawInputBufferFn> = Original::new();
static HID_USAGES: Original<Args8> = Original::new();
static HID_VALUE: Original<Args8> = Original::new();
static HID_DATA: Original<Args6> = Original::new();
static HID_REPORT: Original<Args3> = Original::new();
static IO_CONTROL: Original<Args8> = Original::new();
static READ_FILE: Original<Args6> = Original::new();
const RID_INPUT: u32 = 0x1000_0003;

unsafe extern "system" fn raw_data(handle: *mut c_void, command: u32, data: *mut c_void, size: *mut u32, header: u32) -> u32 {
    let flight = InFlight::enter();
    // SAFETY: forwards the game's own call.
    let result = unsafe { RAW_DATA.get(&flight)(handle, command, data, size, header) };
    if command == RID_INPUT && !data.is_null() && result != u32::MAX && result >= 4 {
        // SAFETY: RAWINPUTHEADER.dwType, just filled: 0 mouse, 1 keyboard, 2 HID.
        let kind = unsafe { *(data as *const u32) };
        note(|m| format!("Raw Input record type {kind} by {m}"));
    }
    result
}

unsafe extern "system" fn raw_buffer(data: *mut c_void, size: *mut u32, header: u32) -> u32 {
    let flight = InFlight::enter();
    // SAFETY: forwards the game's own call.
    let result = unsafe { RAW_BUFFER.get(&flight)(data, size, header) };
    if !data.is_null() && result != u32::MAX && result > 0 {
        // SAFETY: the first record's RAWINPUTHEADER.dwType.
        let kind = unsafe { *(data as *const u32) };
        note(|m| format!("Raw Input buffer, first record type {kind} by {m}"));
    }
    result
}

unsafe extern "system" fn hid_usages(a: u64, b: u64, c: u64, d: u64, e: u64, f: u64, g: u64, h: u64) -> u64 {
    let flight = InFlight::enter();
    note(|m| format!("HidP_GetUsages by {m}"));
    // SAFETY: forwards the game's own call with its eight arguments.
    unsafe { HID_USAGES.get(&flight)(a, b, c, d, e, f, g, h) }
}

unsafe extern "system" fn hid_value(a: u64, b: u64, c: u64, d: u64, e: u64, f: u64, g: u64, h: u64) -> u64 {
    let flight = InFlight::enter();
    note(|m| format!("HidP_GetUsageValue by {m}"));
    // SAFETY: as above.
    unsafe { HID_VALUE.get(&flight)(a, b, c, d, e, f, g, h) }
}

unsafe extern "system" fn hid_data(a: u64, b: u64, c: u64, d: u64, e: u64, f: u64) -> u64 {
    let flight = InFlight::enter();
    note(|m| format!("HidP_GetData by {m}"));
    // SAFETY: as above, six arguments.
    unsafe { HID_DATA.get(&flight)(a, b, c, d, e, f) }
}

unsafe extern "system" fn hid_report(a: u64, b: u64, c: u64) -> u64 {
    let flight = InFlight::enter();
    note(|m| format!("HidD_GetInputReport by {m}"));
    // SAFETY: as above, three arguments.
    unsafe { HID_REPORT.get(&flight)(a, b, c) }
}

/// Every driver request, by caller and control code: whatever reads a pad (any XInput copy asks
/// the Xbox controller driver, codes 0x8000e0xx) shows up here under its own module.
unsafe extern "system" fn io_control(a: u64, code: u64, c: u64, d: u64, e: u64, f: u64, g: u64, h: u64) -> u64 {
    let flight = InFlight::enter();
    let code = code as u32;
    note(|m| format!("DeviceIoControl {code:#010x} by {m}"));
    // SAFETY: forwards the call with its eight arguments.
    unsafe { IO_CONTROL.get(&flight)(a, code as u64, c, d, e, f, g, h) }
}

/// File reads, by caller (a HID pad read straight from its device file shows up here).
unsafe extern "system" fn read_file(a: u64, b: u64, c: u64, d: u64, e: u64, f: u64) -> u64 {
    let flight = InFlight::enter();
    note(|m| format!("ReadFile by {m}"));
    // SAFETY: as above, five arguments (the sixth is padding).
    unsafe { READ_FILE.get(&flight)(a, b, c, d, e, f) }
}

/// Prepares the probe's hooks (enabled with the producer's others); each API that is not loaded
/// or cannot be hooked is logged and skipped.
pub fn install(hooks: &mut Hooks) {
    let skip = |what: &str, why: String| log!("input probe: {what} not watched: {why}");
    match Module::find("dinput8.dll").and_then(|m| m.export("DirectInput8Create")) {
        Some(create) => {
            // SAFETY: the export's documented signature.
            let create: CreateFn = unsafe { std::mem::transmute::<usize, CreateFn>(create) };
            let mut seen: Vec<[usize; 3]> = Vec::new();
            for iid in [IID_DIRECTINPUT8_W, IID_DIRECTINPUT8_A] {
                if let Some(methods) = di_methods(create, &iid).filter(|m| !seen.contains(m)) {
                    seen.push(methods);
                }
            }
            let detours: [(GetDeviceStateFn, GetDeviceDataFn, PollFn); DI_SETS] = [(di_state::<0>, di_data::<0>, di_poll::<0>), (di_state::<1>, di_data::<1>, di_poll::<1>)];
            for (i, [state, data, poll]) in seen.into_iter().enumerate().take(DI_SETS) {
                let (state_detour, data_detour, poll_detour) = detours[i];
                // SAFETY: IDirectInputDevice8 methods of these signatures.
                let hooked = unsafe {
                    hooks
                        .inline_chained(&DI_STATE[i], "GetDeviceState", state, state_detour)
                        .and_then(|()| hooks.inline_chained(&DI_DATA[i], "GetDeviceData", data, data_detour))
                        .and_then(|()| hooks.inline_chained(&DI_POLL[i], "Poll", poll, poll_detour))
                };
                if let Err(e) = hooked {
                    skip("DirectInput", e.to_string());
                }
            }
        }
        None => skip("DirectInput", "dinput8.dll is not loaded".into()),
    }
    match Module::find("user32.dll") {
        Some(user32) => {
            if let (Some(data), Some(buffer)) = (user32.export("GetRawInputData"), user32.export("GetRawInputBuffer")) {
                // SAFETY: user32's documented signatures.
                let hooked = unsafe {
                    hooks
                        .inline_chained(&RAW_DATA, "GetRawInputData", data, raw_data as GetRawInputDataFn)
                        .and_then(|()| hooks.inline_chained(&RAW_BUFFER, "GetRawInputBuffer", buffer, raw_buffer as GetRawInputBufferFn))
                };
                if let Err(e) = hooked {
                    skip("Raw Input", e.to_string());
                }
            }
        }
        None => skip("Raw Input", "user32.dll is not loaded".into()),
    }
    match Module::find("kernelbase.dll") {
        Some(base) => {
            if let (Some(io), Some(read)) = (base.export("DeviceIoControl"), base.export("ReadFile")) {
                // SAFETY: kernelbase's documented signatures (integer and pointer arguments only).
                let hooked = unsafe {
                    hooks
                        .inline_chained(&IO_CONTROL, "DeviceIoControl", io, io_control as Args8)
                        .and_then(|()| hooks.inline_chained(&READ_FILE, "ReadFile", read, read_file as Args6))
                };
                if let Err(e) = hooked {
                    skip("driver requests", e.to_string());
                }
            }
        }
        None => skip("driver requests", "kernelbase.dll is not loaded".into()),
    }
    match Module::find("hid.dll") {
        Some(hid) => {
            let exports = (hid.export("HidP_GetUsages"), hid.export("HidP_GetUsageValue"), hid.export("HidP_GetData"), hid.export("HidD_GetInputReport"));
            if let (Some(usages), Some(value), Some(data), Some(report)) = exports {
                // SAFETY: hid.dll's documented signatures (all integer or pointer arguments).
                let hooked = unsafe {
                    hooks
                        .inline_chained(&HID_USAGES, "HidP_GetUsages", usages, hid_usages as Args8)
                        .and_then(|()| hooks.inline_chained(&HID_VALUE, "HidP_GetUsageValue", value, hid_value as Args8))
                        .and_then(|()| hooks.inline_chained(&HID_DATA, "HidP_GetData", data, hid_data as Args6))
                        .and_then(|()| hooks.inline_chained(&HID_REPORT, "HidD_GetInputReport", report, hid_report as Args3))
                };
                if let Err(e) = hooked {
                    skip("HID", e.to_string());
                }
            }
        }
        None => skip("HID", "hid.dll is not loaded".into()),
    }
}
