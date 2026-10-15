//! A mouse of our own on top of the real one: counts queued with [`add`] go into what the game
//! reads from its DirectInput mouse, so the game turns as if the mouse had moved (head aim by
//! mouse in games whose look angles are out of reach). Both ways a game reads a DirectInput mouse
//! are covered: the immediate state (`GetDeviceState` with a `DIMOUSESTATE`/`DIMOUSESTATE2`) and
//! buffered events (`GetDeviceData`, an X and a Y event appended when the game's buffer has room).
//! Whole counts go out; the fraction stays queued.
//!
//! Two sources are kept apart: [`add`] (a head's turn, whose share the producer must know and
//! takes back at the end) and [`add_other`] (the player's own turning fed this way, a stick
//! converted to mouse so the game sees one look device); [`delivered`] and [`pending`] are the
//! first's.

use crate::probe::{CreateFn, GetDeviceDataFn, GetDeviceStateFn, IID_DIRECTINPUT8_A, IID_DIRECTINPUT8_W, di_methods};
use monaka_hook::module::Module;
use monaka_hook::{Hooks, InFlight, Original, mem};
use monaka_producer::{Rejection, log};
use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering::*};
use monaka_core::sync::Locked;

/// Set while counts may go out.
pub static ACTIVE: AtomicBool = AtomicBool::new(false);
/// Counts of [`add`] handed to the game so far (x right, y down, as DirectInput reports them).
static DELIVERED: [AtomicI64; 2] = [AtomicI64::new(0), AtomicI64::new(0)];
/// Counts of [`add_other`] handed to the game so far.
static DELIVERED_OTHER: [AtomicI64; 2] = [AtomicI64::new(0), AtomicI64::new(0)];
/// Queued counts per source ([`add`], [`add_other`]) and axis.
static PENDING: Mutex<[[f64; 2]; 2]> = Mutex::new([[0.0; 2]; 2]);
static STATE_READS: AtomicU64 = AtomicU64::new(0);
static DATA_READS: AtomicU64 = AtomicU64::new(0);
static DATA_FULL: AtomicU64 = AtomicU64::new(0);
/// Successful reads of a mouse (either way) while active: the game is reading its mouse (a game
/// reads it only while its window has focus).
static MOUSE_READS: AtomicU64 = AtomicU64::new(0);
/// Devices seen in `GetDeviceData`, and whether each is a mouse.
static DEVICES: Mutex<Vec<(usize, bool)>> = Mutex::new(Vec::new());

/// `DIMOUSESTATE` and `DIMOUSESTATE2` sizes (the device kind of a state read: a keyboard's is 256).
const MOUSE_STATE_SIZES: [u32; 2] = [16, 20];
/// `DIMOFS_X`, `DIMOFS_Y`; `DIGDD_PEEK`; `DI8DEVTYPE_MOUSE`.
const OFFSET_X: u32 = 0;
const OFFSET_Y: u32 = 4;
const PEEK: u32 = 1;
const DEVICE_TYPE_MOUSE: u32 = 0x12;
const SLOT_GET_CAPABILITIES: usize = 3;
type GetCapabilitiesFn = unsafe extern "system" fn(*mut c_void, *mut DevCaps) -> i32;

/// `DIDEVCAPS`.
#[repr(C)]
#[derive(Default)]
struct DevCaps {
    size: u32,
    flags: u32,
    device_type: u32,
    rest: [u32; 8],
}

/// `DIDEVICEOBJECTDATA` (its last field is pointer-sized: 24 bytes on x64, 20 on x86).
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ObjectData {
    offset: u32,
    data: u32,
    timestamp: u32,
    sequence: u32,
    app_data: usize,
}

// SAFETY: integers only, every bit pattern valid; no padding (4 x u32 then a usize).
unsafe impl mem::Plain for ObjectData {}

/// How many DirectInput method sets are hooked (the wide and the ANSI interface, when they differ).
const SETS: usize = 2;
/// Each set's originals, by its number.
static STATE: [Original<GetDeviceStateFn>; SETS] = [const { Original::new() }; SETS];
static DATA: [Original<GetDeviceDataFn>; SETS] = [const { Original::new() }; SETS];

/// Queues `dx`, `dy` counts (fractions carry over).
pub fn add(dx: f64, dy: f64) {
    queue(0, dx, dy);
}

/// Queues counts that are not part of [`delivered`]: the player's own turning.
pub fn add_other(dx: f64, dy: f64) {
    queue(1, dx, dy);
}

fn queue(source: usize, dx: f64, dy: f64) {
    if !(dx.is_finite() && dy.is_finite()) {
        return;
    }
    let mut pending = PENDING.locked();
    pending[source][0] += dx;
    pending[source][1] += dy;
}

/// Counts handed to the game so far.
pub fn delivered() -> [i64; 2] {
    [DELIVERED[0].load(Acquire), DELIVERED[1].load(Acquire)]
}

/// Successful mouse reads so far while active.
pub fn reads() -> u64 {
    MOUSE_READS.load(Acquire)
}

/// Counts of [`add`] queued but not yet read by the game.
pub fn pending() -> [f64; 2] {
    PENDING.locked()[0]
}

pub fn summary() -> String {
    let mice = DEVICES.lock().map(|d| d.iter().filter(|(_, m)| *m).count()).unwrap_or(0);
    format!(
        "fake mouse: {} state reads, {} buffered reads ({} with a full buffer), {} reads of the game's own, {mice} buffered mouse device(s), delivered {:?} (and {:?} of the player's own), pending {:?}",
        STATE_READS.load(Relaxed),
        DATA_READS.load(Relaxed),
        DATA_FULL.load(Relaxed),
        OWN_READS.load(Relaxed),
        delivered(),
        [DELIVERED_OTHER[0].load(Relaxed), DELIVERED_OTHER[1].load(Relaxed)],
        pending()
    )
}

/// The whole counts to hand out now per source, taken from the queue.
fn take() -> [[i32; 2]; 2] {
    let mut pending = PENDING.locked();
    pending.each_mut().map(|source| {
        let out = source.map(|v| v.trunc().clamp(-1.0e6, 1.0e6) as i32);
        for (p, o) in source.iter_mut().zip(out) {
            *p -= o as f64;
        }
        out
    })
}

/// Both sources' counts together, as the game gets them.
fn total(taken: &[[i32; 2]; 2]) -> [i32; 2] {
    [taken[0][0] + taken[1][0], taken[0][1] + taken[1][1]]
}

/// Gives counts that could not go out back to the queue.
fn give_back(taken: [[i32; 2]; 2]) {
    let mut pending = PENDING.locked();
    for (source, counts) in pending.iter_mut().zip(taken) {
        source[0] += counts[0] as f64;
        source[1] += counts[1] as f64;
    }
}

fn delivered_now(taken: [[i32; 2]; 2]) {
    for (cells, counts) in [&DELIVERED, &DELIVERED_OTHER].into_iter().zip(taken) {
        cells[0].fetch_add(counts[0] as i64, AcqRel);
        cells[1].fetch_add(counts[1] as i64, AcqRel);
    }
}

fn is_mouse(device: *mut c_void) -> bool {
    let key = device as usize;
    let mut devices = DEVICES.locked();
    if let Some(&(_, mouse)) = devices.iter().find(|(d, _)| *d == key) {
        return mouse;
    }
    let mut caps = DevCaps { size: size_of::<DevCaps>() as u32, ..Default::default() };
    let method = mem::read::<usize>(key).and_then(|table| mem::read::<usize>(table + SLOT_GET_CAPABILITIES * size_of::<usize>()));
    let mouse = method.is_some_and(|m| {
        // SAFETY: IDirectInputDevice8::GetCapabilities of the device the game is reading.
        let got = unsafe { std::mem::transmute::<usize, GetCapabilitiesFn>(m)(device, &mut caps) };
        got >= 0 && caps.device_type & 0xff == DEVICE_TYPE_MOUSE
    });
    if devices.len() < 64 {
        devices.push((key, mouse));
    }
    mouse
}

/// Successful reads through [`deliver_with`].
static OWN_READS: AtomicU64 = AtomicU64::new(0);

/// For a game that reads its mouse outside DirectInput (Control: Raw Input into its own input
/// object): called at the game's once-a-frame mouse update, after it has taken the frame's
/// movement. The whole counts due now (x right, y down, as DirectInput reports them) go to
/// `deliver`, which adds them to what the game took and says whether it could; counted as a mouse
/// read either way, and given back to the queue when not delivered.
pub fn deliver_with(deliver: impl FnOnce([i32; 2]) -> bool) {
    if !ACTIVE.load(Acquire) {
        return;
    }
    OWN_READS.fetch_add(1, Relaxed);
    MOUSE_READS.fetch_add(1, Release);
    let taken = take();
    let counts = total(&taken);
    if counts == [0, 0] {
        return;
    }
    if deliver(counts) { delivered_now(taken) } else { give_back(taken) }
}

/// After a successful immediate read of a mouse state: our counts added to its X and Y.
fn after_state(size: u32, out: *mut c_void) {
    if !ACTIVE.load(Acquire) || out.is_null() || !MOUSE_STATE_SIZES.contains(&size) {
        return;
    }
    STATE_READS.fetch_add(1, Relaxed);
    MOUSE_READS.fetch_add(1, Release);
    let taken = take();
    let counts = total(&taken);
    if counts == [0, 0] {
        return;
    }
    let (x, y) = (out as usize, out as usize + 4);
    match (mem::read::<i32>(x), mem::read::<i32>(y)) {
        (Some(lx), Some(ly)) if mem::write(x, lx.wrapping_add(counts[0])) && mem::write(y, ly.wrapping_add(counts[1])) => delivered_now(taken),
        _ => give_back(taken),
    }
}

/// After a successful buffered read: our counts appended as an X and a Y event when it is a
/// mouse, the read removes events and the buffer has room.
fn after_data(device: *mut c_void, size: u32, out: *mut c_void, count: *mut u32, capacity: u32, flags: u32) {
    if !ACTIVE.load(Acquire) || out.is_null() || count.is_null() || flags & PEEK != 0 || size as usize != size_of::<ObjectData>() {
        return;
    }
    DATA_READS.fetch_add(1, Relaxed);
    if !is_mouse(device) {
        return;
    }
    MOUSE_READS.fetch_add(1, Release);
    let Some(got) = mem::read::<u32>(count as usize) else { return };
    let taken = take();
    let counts = total(&taken);
    let events: Vec<(u32, i32)> = [(OFFSET_X, counts[0]), (OFFSET_Y, counts[1])].into_iter().filter(|(_, v)| *v != 0).collect();
    if events.is_empty() {
        return;
    }
    if got as usize + events.len() > capacity as usize {
        DATA_FULL.fetch_add(1, Relaxed);
        give_back(taken);
        return;
    }
    let base = out as usize;
    let last = got.checked_sub(1).and_then(|i| mem::read::<ObjectData>(base + i as usize * size_of::<ObjectData>()));
    let (timestamp, mut sequence) = last.map_or((0, 0), |e| (e.timestamp, e.sequence.wrapping_add(1)));
    let mut written = got;
    for (offset, value) in events {
        let event = ObjectData { offset, data: value as u32, timestamp, sequence, app_data: 0 };
        if !mem::write(base + written as usize * size_of::<ObjectData>(), event) {
            break;
        }
        written += 1;
        sequence = sequence.wrapping_add(1);
    }
    if written == got + 2 || (written > got && (counts[0] == 0 || counts[1] == 0)) {
        mem::write(count as usize, written);
        delivered_now(taken);
    } else {
        give_back(taken);
    }
}

/// Set `N`'s detour for `GetDeviceState`.
unsafe extern "system" fn state<const N: usize>(device: *mut c_void, size: u32, out: *mut c_void) -> i32 {
    let flight = InFlight::enter();
    // SAFETY: forwards the game's own call.
    let result = unsafe { STATE[N].get(&flight)(device, size, out) };
    if result >= 0 {
        after_state(size, out);
    }
    result
}

/// Set `N`'s detour for `GetDeviceData`.
unsafe extern "system" fn data<const N: usize>(device: *mut c_void, size: u32, out: *mut c_void, count: *mut u32, flags: u32) -> i32 {
    let flight = InFlight::enter();
    let capacity = if count.is_null() { 0 } else { mem::read::<u32>(count as usize).unwrap_or(0) };
    // SAFETY: forwards the game's own call.
    let result = unsafe { DATA[N].get(&flight)(device, size, out, count, flags) };
    if result >= 0 {
        after_data(device, size, out, count, capacity, flags);
    }
    result
}

/// Prepares the hooks on DirectInput's device reads (enabled with the producer's others; counts
/// go out only while [`ACTIVE`]). Returns how many method sets were hooked (the ANSI and wide
/// interfaces usually share one).
pub fn install(hooks: &mut Hooks) -> Result<usize, Rejection> {
    let create = Module::find("dinput8.dll")
        .and_then(|m| m.export("DirectInput8Create"))
        .ok_or_else(|| Rejection::unsupported("dinput8.dll is not loaded (the game reads no DirectInput mouse)"))?;
    // SAFETY: the export's documented signature.
    let create: CreateFn = unsafe { std::mem::transmute::<usize, CreateFn>(create) };
    let mut seen: Vec<[usize; 3]> = Vec::new();
    for iid in [IID_DIRECTINPUT8_W, IID_DIRECTINPUT8_A] {
        if let Some(methods) = di_methods(create, &iid).filter(|m| !seen.contains(m)) {
            seen.push(methods);
        }
    }
    if seen.is_empty() {
        return Err(Rejection::unsupported("could not make a DirectInput mouse to find its methods"));
    }
    let detours: [(GetDeviceStateFn, GetDeviceDataFn); SETS] = [(state::<0>, data::<0>), (state::<1>, data::<1>)];
    let names = [("GetDeviceState", "GetDeviceData"), ("GetDeviceState (second interface)", "GetDeviceData (second interface)")];
    for (i, [state_at, data_at, _]) in seen.iter().copied().enumerate().take(SETS) {
        // SAFETY: IDirectInputDevice8 methods of these signatures.
        unsafe {
            hooks.inline_chained(&STATE[i], names[i].0, state_at, detours[i].0)?;
            hooks.inline_chained(&DATA[i], names[i].1, data_at, detours[i].1)?;
        }
    }
    log!("fake mouse: DirectInput reads hooked ({} method set(s))", seen.len().min(SETS));
    Ok(seen.len().min(SETS))
}

#[cfg(test)]
mod tests;
