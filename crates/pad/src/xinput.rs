//! XInput's calls caught ([`install`]): in the code of each system XInput DLL loaded and in the
//! import tables of the game's own modules, each hooked place with a set of its own
//! ([`ExportHooks`]). Each read of pad 0 hands the game the VR pad merged with a real one
//! ([`read`]); asking what is in slot 0 reports a pad while the VR one is live ([`describe`]).

use crate::presses::pressed;
use crate::stick::{TAKE_RIGHT, take_right, takes_y, walk_turned};
use crate::presses::{flick, rewritten};
use crate::vr_pad::{ACTIVE, CHANNEL, MASK_REAL, open_block, vr_pad};
use monaka_channel::ChannelName;
use monaka_core::pad::{dither, merge};
use monaka_contract::gamepad::Gamepad;
use monaka_hook::module::Module;
use monaka_hook::{Hooks, InFlight, Original};
use monaka_producer::{Rejection, log};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering::*};

/// XInput's `XINPUT_STATE`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct XInputState {
    pub packet: u32,
    pub gamepad: Gamepad,
}

/// XInput's `XINPUT_CAPABILITIES`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct XInputCapabilities {
    pub kind: u8,
    pub sub_type: u8,
    pub flags: u16,
    pub gamepad: Gamepad,
    pub vibration: [u16; 2],
}

pub(super) const ERROR_SUCCESS: u32 = 0;
/// `XINPUT_DEVTYPE_GAMEPAD` and `XINPUT_DEVSUBTYPE_GAMEPAD`.
pub(super) const GAMEPAD: u8 = 1;
/// XInput's exports by number (`xinput1_3.dll`, `xinput1_4.dll`).
pub(super) const GET_STATE_ORDINAL: u16 = 2;
pub(super) const GET_CAPABILITIES_ORDINAL: u16 = 4;
/// How long a stick holds each of its two jittered values.
pub(super) const DITHER_MS: u64 = 20;
/// A pad the viewer has not refreshed for this long is gone (the viewer stopped or lost focus).
pub(super) const PAD_MAX_AGE_MS: u64 = 250;

pub(super) type GetStateFn = unsafe extern "system" fn(u32, *mut XInputState) -> u32;
pub(super) type GetCapabilitiesFn = unsafe extern "system" fn(u32, u32, *mut XInputCapabilities) -> u32;

pub(super) const GET_STATE_EX_ORDINAL: u16 = 100;

/// The hooks at one place (an XInput DLL's code, or one module's imports from one XInput DLL):
/// its three functions' originals and our detours for them.
pub(super) struct ExportHooks {
    state: &'static Original<GetStateFn>,
    state_ex: &'static Original<GetStateFn>,
    capabilities: &'static Original<GetCapabilitiesFn>,
    detours: (GetStateFn, GetStateFn, GetCapabilitiesFn),
}

/// How many places can be hooked.
const SET_COUNT: usize = 6;

/// Each set's originals, by its number.
static STATE: [Original<GetStateFn>; SET_COUNT] = [const { Original::new() }; SET_COUNT];
static STATE_EX: [Original<GetStateFn>; SET_COUNT] = [const { Original::new() }; SET_COUNT];
static CAPABILITIES: [Original<GetCapabilitiesFn>; SET_COUNT] = [const { Original::new() }; SET_COUNT];

/// Set `N`'s detour for `XInputGetState`.
unsafe extern "system" fn state<const N: usize>(user: u32, state: *mut XInputState) -> u32 {
    // SAFETY: the game's own call, forwarded with its arguments.
    unsafe { read(&STATE[N], user, state) }
}

/// Set `N`'s detour for `XInputGetStateEx`.
unsafe extern "system" fn state_ex<const N: usize>(user: u32, state: *mut XInputState) -> u32 {
    // SAFETY: as above.
    unsafe { read(&STATE_EX[N], user, state) }
}

/// Set `N`'s detour for `XInputGetCapabilities`.
unsafe extern "system" fn capabilities<const N: usize>(user: u32, flags: u32, out: *mut XInputCapabilities) -> u32 {
    // SAFETY: as above.
    unsafe { describe(&CAPABILITIES[N], user, flags, out) }
}

const fn set<const N: usize>() -> ExportHooks {
    ExportHooks { state: &STATE[N], state_ex: &STATE_EX[N], capabilities: &CAPABILITIES[N], detours: (state::<N>, state_ex::<N>, capabilities::<N>) }
}

/// One set per hooked place; more places than sets are logged and left alone.
pub(super) const SETS: [ExportHooks; SET_COUNT] = [set::<0>(), set::<1>(), set::<2>(), set::<3>(), set::<4>(), set::<5>()];
/// The XInput DLLs whose code is hooked when loaded (system copies, by full path: a game folder may
/// hold a proxy of the same name).
pub(super) const XINPUT_DLLS: [&str; 3] = ["XInput1_4.dll", "XInput1_3.dll", "XInput9_1_0.dll"];

/// [`install`] as an optional piece of a run, logged either way: without it VR runs, the
/// controllers' buttons just do nothing. Whether the pad went in (then [`announce`] it once the
/// hooks are enabled).
pub fn install_optional(hooks: &mut Hooks, channel: &ChannelName) -> bool {
    match install(hooks, channel) {
        Ok(count) => {
            log!("controller pad: the VR controllers are the game's pad 0 ({count} places hooked)");
            true
        }
        Err(why) => {
            log!("controller pad off: {}", why.reason);
            false
        }
    }
}

/// Prepares the pad hooks (enabled with the producer's other hooks) and opens the hand block
/// beside `channel`: in the code of each system XInput DLL loaded, and in the import table of each
/// of the game's own modules (those in the host executable's folder tree) that imports XInput.
/// Returns how many places were hooked; a place that cannot be hooked is logged and skipped.
pub fn install(hooks: &mut Hooks, channel: &ChannelName) -> Result<usize, Rejection> {
    open_block(channel)?;
    let mut sets = SETS.iter();
    let mut hooked = 0;
    let system = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    for dll in XINPUT_DLLS {
        let Some(module) = Module::find(&format!(r"{system}\System32\{dll}")) else { continue };
        let Some(set) = sets.next() else { break };
        match hook_dll(hooks, &module, set) {
            Ok(()) => {
                hooked += 1;
                log!("controller pad: hooked {dll}'s own code");
            }
            Err(why) => log!("controller pad: {dll}'s code not hooked: {}", why.reason),
        }
    }
    let host = Module::loaded().into_iter().next().and_then(|m| m.path()).and_then(|p| p.parent().map(|d| d.to_path_buf()));
    let own = Module::containing(install as fn(&mut Hooks, &ChannelName) -> Result<usize, Rejection> as usize);
    for module in Module::loaded() {
        if Some(module) == own || !module.path().zip(host.as_ref()).is_some_and(|(path, folder)| path.starts_with(folder)) {
            continue;
        }
        for dll in module.imported_dlls().into_iter().filter(|d| d.to_ascii_lowercase().starts_with("xinput")) {
            let Some(set) = sets.next() else {
                log!("controller pad: out of hook sets; {} {dll} left alone", Module::describe(module.base()));
                continue;
            };
            match hook_imports(hooks, &module, &dll, set) {
                Ok(count) if count > 0 => {
                    hooked += 1;
                    log!("controller pad: hooked {count} {dll} imports of {}", Module::describe(module.base()));
                }
                Ok(_) => {}
                Err(why) => log!("controller pad: {} {dll} imports not hooked: {}", Module::describe(module.base()), why.reason),
            }
        }
    }
    if hooked == 0 {
        return Err(Rejection::unsupported("controller pad: no XInput caller could be hooked"));
    }
    ACTIVE.store(true, Release);
    Ok(hooked)
}

/// Hooks `module`'s imports of the three calls from `dll`, by name or by number, whichever it
/// uses. Returns how many it imports.
pub(super) fn hook_imports(hooks: &mut Hooks, module: &Module, dll: &str, set: &ExportHooks) -> Result<usize, Rejection> {
    let (state_detour, state_ex_detour, capabilities_detour) = set.detours;
    let mut count = 0;
    // SAFETY: XInput's exports have these documented signatures (GetStateEx is GetState's).
    unsafe {
        if module.import_slot(dll, "XInputGetState").is_some() {
            hooks.import(set.state, "XInputGetState", module, dll, "XInputGetState", state_detour)?;
            count += 1;
        } else if module.import_ordinal_slot(dll, GET_STATE_ORDINAL).is_some() {
            hooks.import_ordinal(set.state, "XInputGetState", module, dll, GET_STATE_ORDINAL, state_detour)?;
            count += 1;
        }
        if module.import_ordinal_slot(dll, GET_STATE_EX_ORDINAL).is_some() {
            hooks.import_ordinal(set.state_ex, "XInputGetStateEx", module, dll, GET_STATE_EX_ORDINAL, state_ex_detour)?;
            count += 1;
        }
        if module.import_slot(dll, "XInputGetCapabilities").is_some() {
            hooks.import(set.capabilities, "XInputGetCapabilities", module, dll, "XInputGetCapabilities", capabilities_detour)?;
            count += 1;
        } else if module.import_ordinal_slot(dll, GET_CAPABILITIES_ORDINAL).is_some() {
            hooks.import_ordinal(set.capabilities, "XInputGetCapabilities", module, dll, GET_CAPABILITIES_ORDINAL, capabilities_detour)?;
            count += 1;
        }
    }
    Ok(count)
}

pub(super) fn hook_dll(hooks: &mut Hooks, xinput: &Module, set: &ExportHooks) -> Result<(), Rejection> {
    let state = xinput.export("XInputGetState").ok_or_else(|| Rejection::unsupported("no XInputGetState export"))?;
    let state_ex = xinput.export_ordinal(GET_STATE_EX_ORDINAL).filter(|&ex| ex != state);
    let capabilities = xinput.export("XInputGetCapabilities").ok_or_else(|| Rejection::unsupported("no XInputGetCapabilities export"))?;
    let (state_detour, state_ex_detour, capabilities_detour) = set.detours;
    // SAFETY: XInput's exports have these documented signatures (GetStateEx is GetState's).
    unsafe {
        // The Steam overlay hooks these in every game it runs in: chain after it.
        hooks.inline_chained(set.state, "XInputGetState", state, state_detour)?;
        if let Some(state_ex) = state_ex {
            hooks.inline_chained(set.state_ex, "XInputGetStateEx", state_ex, state_ex_detour)?;
        }
        hooks.inline_chained(set.capabilities, "XInputGetCapabilities", capabilities, capabilities_detour)?;
    }
    Ok(())
}

pub(super) struct Counters {
    checks: AtomicU64,
    reported: AtomicU64,
    polls: AtomicU64,
    merged: AtomicU64,
    stood_in: AtomicU64,
    input: AtomicU64,
    /// Reads where the VR pad's left stick, and the left stick handed to the game, were pushed
    /// past a quarter of their range.
    vr_left_stick: AtomicU64,
    out_left_stick: AtomicU64,
}
pub(super) static COUNTERS: Counters = Counters {
    checks: AtomicU64::new(0),
    reported: AtomicU64::new(0),
    polls: AtomicU64::new(0),
    merged: AtomicU64::new(0),
    stood_in: AtomicU64::new(0),
    input: AtomicU64::new(0),
    vr_left_stick: AtomicU64::new(0),
    out_left_stick: AtomicU64::new(0),
};

/// The packet number for a changed pad: past the last one handed out, the real pad's own, and
/// `floor` ([`packet_floor`]). Games keep the last number they saw and take only a newer state
/// (The New Order: a state whose number is not above it is dropped), and a real pad they read
/// before the hooks went in (or before it was switched off) has already shown them its own
/// numbers, often in the hundreds.
pub(super) fn next_packet(last: u32, real: u32, floor: u32) -> u32 {
    last.wrapping_add(1).max(real.wrapping_add(1)).max(floor).max(1)
}

/// A number above any packet number a real pad can have reached since the PC started: half the
/// milliseconds since then (a pad's number goes up once per changed state, at most once a read,
/// a few milliseconds apart), kept under 2^31 so games that compare numbers as signed differences
/// see it as newer.
pub(super) fn packet_floor() -> u32 {
    ((monaka_channel::tick() / 2) % (1 << 31)) as u32
}

/// Whether a left stick is pushed past a quarter of its range.
pub(super) fn left_stick_pushed(pad: &Gamepad) -> bool {
    pad.thumb_lx.unsigned_abs() > 8192 || pad.thumb_ly.unsigned_abs() > 8192
}

/// The last pad handed out and its packet number (XInput's change counter: games skip a state
/// whose number they have seen).
pub(super) static LAST: Mutex<(Gamepad, u32)> = Mutex::new((Gamepad { buttons: 0, left_trigger: 0, right_trigger: 0, thumb_lx: 0, thumb_ly: 0, thumb_rx: 0, thumb_ry: 0 }, 0));

thread_local! {
    /// Set while a read is being answered on this thread: an XInput read made from inside another
    /// one (GetState through GetStateEx, or a proxy through the system DLL) is merged only once.
    static READING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// # Safety
/// `original` is the hooked read the game called, with the game's arguments.
unsafe fn read(original: &Original<GetStateFn>, user: u32, state: *mut XInputState) -> u32 {
    let flight = InFlight::enter();
    let outer = !READING.with(|r| r.replace(true));
    // SAFETY: forwards the game's own call.
    let result = unsafe { original.get(&flight)(user, state) };
    if outer {
        READING.with(|r| r.set(false));
    }
    if !outer || user != 0 || state.is_null() {
        return result;
    }
    note_reader();
    COUNTERS.polls.fetch_add(1, Relaxed);
    // A producer's own press counts even while the viewer's pad is not live.
    let pressed = pressed();
    let Some(vr) = vr_pad().map(|vr| merge(vr, pressed)).or((pressed != Gamepad::default() && ACTIVE.load(Acquire)).then_some(pressed)) else {
        if result == ERROR_SUCCESS && ACTIVE.load(Acquire) {
            // A real pad alone: its right stick is taken all the same.
            // SAFETY: the game's out-structure, filled by XInput.
            unsafe {
                take_right(&mut (*state).gamepad);
                walk_turned(&mut (*state).gamepad);
                rewritten(&mut (*state).gamepad);
            }
        }
        return result;
    };
    let connected = result == ERROR_SUCCESS;
    // SAFETY: the game's out-structure, filled by XInput when connected.
    let (mut real, real_packet) = if connected { unsafe { ((*state).gamepad, (*state).packet) } } else { (Gamepad::default(), 0) };
    let seen = real;
    if MASK_REAL.load(Relaxed) {
        real = Gamepad::default();
    }
    // The jitter changes with time, not per read: a library may read the pad many times in one
    // frame (SDL re-reads it for every event the game takes), and a pad that changes on every
    // read hands the game a new event each time, forever, and the game stalls in its event loop.
    let merged = merge(real, vr);
    let mut pad = dither(merged, vr, (monaka_channel::tick() / DITHER_MS) % 2 == 1);
    // The undithered stick to the producer; taken, a centred one to the game.
    let mut kept = merged;
    take_right(&mut kept);
    if TAKE_RIGHT.load(Acquire) {
        pad.thumb_rx = 0;
        pad.thumb_ry = 0;
    } else if takes_y() {
        pad.thumb_ry = 0;
    }
    // The producer's own flick goes through the take.
    if let Some([x, y]) = flick() {
        (pad.thumb_rx, pad.thumb_ry) = (x, y);
    }
    if kept.left_trigger == 0 {
        // Held back by the gate.
        pad.left_trigger = 0;
    }
    walk_turned(&mut pad);
    rewritten(&mut pad);
    let packet = {
        let Ok(mut last) = LAST.lock() else { return result };
        if last.0 != pad || last.1 == 0 {
            last.0 = pad;
            last.1 = next_packet(last.1, real_packet, packet_floor());
        }
        last.1
    };
    trace_read(real_packet, &seen, packet, &pad);
    // SAFETY: as above; written whole.
    unsafe { *state = XInputState { packet, gamepad: pad } };
    (if connected { &COUNTERS.merged } else { &COUNTERS.stood_in }).fetch_add(1, Relaxed);
    if left_stick_pushed(&vr) {
        COUNTERS.vr_left_stick.fetch_add(1, Relaxed);
    }
    if left_stick_pushed(&pad) {
        COUNTERS.out_left_stick.fetch_add(1, Relaxed);
    }
    if vr != Gamepad::default() && COUNTERS.input.fetch_add(1, Relaxed) == 0 {
        log!("controller pad: first VR input handed to the game: {vr:?} (a real pad {})", if connected { "is connected too" } else { "is not connected" });
    }
    ERROR_SUCCESS
}

/// # Safety
/// `original` is the hooked capabilities call the game made, with the game's arguments.
unsafe fn describe(original: &Original<GetCapabilitiesFn>, user: u32, flags: u32, capabilities: *mut XInputCapabilities) -> u32 {
    let flight = InFlight::enter();
    // SAFETY: forwards the game's own call.
    let result = unsafe { original.get(&flight)(user, flags, capabilities) };
    if user != 0 || capabilities.is_null() {
        return result;
    }
    let check = COUNTERS.checks.fetch_add(1, Relaxed);
    let vr = vr_pad();
    if check < 3 {
        let block = CHANNEL.lock().ok().and_then(|mut c| c.as_mut().map(|c| c.read_pad(u64::MAX)));
        log!("controller pad: slot 0 asked (flags {flags}): XInput says {result}, VR pad {}, block holds {block:?}", if vr.is_some() { "live" } else { "absent" });
    }
    if result == ERROR_SUCCESS || vr.is_none() {
        return result;
    }
    if COUNTERS.reported.fetch_add(1, Relaxed) == 0 {
        log!("controller pad: reported a pad in slot 0 (no real one there)");
    }
    // No real pad: describe a full Xbox pad, which the VR pad stands in for.
    let full = Gamepad { buttons: 0xf3ff, left_trigger: 255, right_trigger: 255, thumb_lx: -64, thumb_ly: -64, thumb_rx: -64, thumb_ry: -64 };
    // SAFETY: the game's out-structure, written whole.
    unsafe { *capabilities = XInputCapabilities { kind: GAMEPAD, sub_type: GAMEPAD, flags: 0, gamepad: full, vibration: [0xffff; 2] } };
    ERROR_SUCCESS
}

/// The first reads whose real pad buttons changed, and the first few reads (what goes in, what
/// goes out, and the change counters on both sides).
pub(super) fn trace_read(real_packet: u32, real: &Gamepad, packet: u32, pad: &Gamepad) {
    static LAST_REAL: Mutex<(u16, u64)> = Mutex::new((u16::MAX, 0));
    let Ok(mut last) = LAST_REAL.lock() else { return };
    last.1 += 1;
    if (last.0 != real.buttons || last.1 <= 3) && last.1 < 100_000 {
        static LOGGED: AtomicU64 = AtomicU64::new(0);
        if LOGGED.fetch_add(1, Relaxed) < 20 {
            log!("controller pad: read {}: real packet {real_packet} buttons {:#06x} -> out packet {packet} buttons {:#06x} ly {}", last.1, real.buttons, pad.buttons, pad.thumb_ly);
        }
        last.0 = real.buttons;
    }
}

/// Who reads pad 0 (the first few distinct calling modules, with counts at the end of the run).
pub(super) static READERS: Mutex<Vec<(String, u64)>> = Mutex::new(Vec::new());

pub(super) fn note_reader() {
    let site = monaka_hook::probe::caller();
    let module = Module::containing(site).and_then(|m| m.path()).and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_else(|| format!("{site:#x}"));
    let Ok(mut readers) = READERS.lock() else { return };
    if let Some((_, n)) = readers.iter_mut().find(|(m, _)| *m == module) {
        *n += 1;
    } else if readers.len() < 8 {
        log!("controller pad: pad 0 read by {} ({module})", Module::describe(site));
        readers.push((module, 1));
    }
}

/// End-of-run tally.
pub fn report() {
    let c = &COUNTERS;
    log!(
        "controller pad: slot 0 asked about {} times (reported the VR pad {}); {} polls of pad 0; merged with a real pad {}, stood in for one {}, with VR input {}; left stick pushed: VR {}, handed to the game {}",
        c.checks.load(Relaxed),
        c.reported.load(Relaxed),
        c.polls.load(Relaxed),
        c.merged.load(Relaxed),
        c.stood_in.load(Relaxed),
        c.input.load(Relaxed),
        c.vr_left_stick.load(Relaxed),
        c.out_left_stick.load(Relaxed)
    );
    if let Ok(readers) = READERS.lock() {
        for (module, n) in readers.iter() {
            log!("  read {n}x by {module}");
        }
    }
}

#[cfg(test)]
mod tests;
