//! Games that read their input through Microsoft's GameInput, which reads the devices directly, so
//! neither DirectInput hooks ([`crate::mouse`]) nor injected OS input (`SendInput`) reach them.
//! Kingdom Come: Deliverance II reads its pads this way (XInput is asked for capabilities, never
//! polled); its mouse is CryEngine's DirectInput one.
//!
//! GameInput hands the game readings (`IGameInputReading`). The methods are found on objects of
//! our own (the original, v0, `IGameInput` from `GameInput.dll`'s `GameInputCreate`, ordinal 1, as
//! the game makes it): one class of each serves the game's objects and ours, and a reading's slots
//! are checked by its answers before anything is hooked.
//!
//! - [`install`]: our own mouse movement. A mouse reading's state (`GetMouseState`) carries the
//!   position as counts accumulated since the device came; counts queued with [`add`] go into the
//!   position of every reading from then on, so the game sees them once, as movement.
//! - [`install_pad`] and [`stand_in`]: the VR controllers as a gamepad of the game's own, a
//!   stand-in device handed to the game's device callback.
//! - [`install_probe`]: how a game reads through GameInput (its reading and callback calls, the
//!   kinds it asks for, its gamepad states), logged and left unchanged.

use monaka_hook::module::Module;
use monaka_hook::{Hooks, InFlight, Original};
use monaka_producer::{Rejection, log};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering::*};

/// `GameInputKind`: keyboard, mouse, gamepad.
const KIND_KEYBOARD: u32 = 0x10;
const KIND_MOUSE: u32 = 0x20;
const KIND_GAMEPAD: u32 = 0x40000;
/// `IUnknown::Release`.
const SLOT_RELEASE: usize = 2;
/// `IGameInput` slots: `GetCurrentReading(kind, device, &reading)`, `GetNextReading`,
/// `RegisterReadingCallback`, `RegisterDeviceCallback`.
const SLOT_GET_CURRENT_READING: usize = 4;
const SLOT_GET_NEXT_READING: usize = 5;
const SLOT_REGISTER_READING_CALLBACK: usize = 8;
const SLOT_REGISTER_DEVICE_CALLBACK: usize = 9;
/// `IGameInputReading` slots: `GetInputKind()`, `GetKeyCount()`, `GetMouseState(&state)`,
/// `GetGamepadState(&state)`.
const SLOT_GET_INPUT_KIND: usize = 3;
const SLOT_GET_KEY_COUNT: usize = 14;
const SLOT_GET_MOUSE_STATE: usize = 16;
const SLOT_GET_GAMEPAD_STATE: usize = 22;

/// `GameInputMouseState` (v0): buttons, then the position and the wheels, accumulated counts.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct MouseState {
    buttons: u32,
    position_x: i64,
    position_y: i64,
    wheel_x: i64,
    wheel_y: i64,
}

type CreateFn = unsafe extern "system" fn(*mut *mut c_void) -> i32;
type GetCurrentReadingFn = unsafe extern "system" fn(*mut c_void, u32, *mut c_void, *mut *mut c_void) -> i32;
type GetNextReadingFn = unsafe extern "system" fn(*mut c_void, *mut c_void, u32, *mut c_void, *mut *mut c_void) -> i32;
type RegisterReadingFn = unsafe extern "system" fn(*mut c_void, *mut c_void, u32, f32, *mut c_void, *mut c_void, *mut u64) -> i32;
type RegisterDeviceFn = unsafe extern "system" fn(*mut c_void, *mut c_void, u32, u32, u32, *mut c_void, *mut c_void, *mut u64) -> i32;
type ReleaseFn = unsafe extern "system" fn(*mut c_void) -> u32;
type GetInputKindFn = unsafe extern "system" fn(*mut c_void) -> u32;
type GetMouseStateFn = unsafe extern "system" fn(*mut c_void, *mut MouseState) -> bool;
type GetKeyCountFn = unsafe extern "system" fn(*mut c_void) -> u32;
/// `GetGamepadState(&GameInputGamepadState)`: buttons, then six floats.
type GetGamepadStateFn = unsafe extern "system" fn(*mut c_void, *mut [u32; 8]) -> bool;

/// The method in `slot` of the COM object `object`.
///
/// # Safety
/// `object` is a live COM object with at least `slot + 1` methods.
unsafe fn method(object: *mut c_void, slot: usize) -> usize {
    // SAFETY: the caller's guarantee: the object starts with its vtable pointer.
    unsafe { *(*(object as *const *const usize)).add(slot) }
}

/// Whether `reading` (of `kind`) answers as a reading of that kind does: the slots are where the
/// v0 interface has them.
///
/// # Safety
/// `reading` is a live `IGameInputReading`.
unsafe fn answers(reading: *mut c_void, kind: u32) -> Result<(), String> {
    // SAFETY: the caller's guarantee; each call fills a local of its own.
    unsafe {
        let has = std::mem::transmute::<usize, GetInputKindFn>(method(reading, SLOT_GET_INPUT_KIND))(reading);
        if has & kind == 0 {
            return Err(format!("asked for kind {kind:#x}, the reading has {has:#x}"));
        }
        let ok = match kind {
            KIND_MOUSE => std::mem::transmute::<usize, GetMouseStateFn>(method(reading, SLOT_GET_MOUSE_STATE))(reading, &mut MouseState::default()),
            KIND_GAMEPAD => std::mem::transmute::<usize, GetGamepadStateFn>(method(reading, SLOT_GET_GAMEPAD_STATE))(reading, &mut [0; 8]),
            _ => std::mem::transmute::<usize, GetKeyCountFn>(method(reading, SLOT_GET_KEY_COUNT))(reading) < 256,
        };
        if ok { Ok(()) } else { Err(format!("a kind {kind:#x} reading refused its own state")) }
    }
}

/// GameInput's methods, found on objects of our own.
#[derive(Clone, Copy, Debug)]
struct Methods {
    get_current: usize,
    get_next: usize,
    register_reading: usize,
    register_device: usize,
    mouse_state: usize,
    gamepad_state: usize,
}

fn find() -> Result<Methods, Rejection> {
    let create = Module::find("GameInput.dll")
        .and_then(|m| m.export("GameInputCreate"))
        .ok_or_else(|| Rejection::unsupported("GameInput.dll is not loaded (the game reads no GameInput)"))?;
    let mut input: *mut c_void = std::ptr::null_mut();
    // SAFETY: the export's documented signature; it hands back an `IGameInput` we release below.
    let hr = unsafe { std::mem::transmute::<usize, CreateFn>(create)(&mut input) };
    if hr < 0 || input.is_null() {
        return Err(Rejection::unsupported(format!("GameInputCreate failed ({hr:#x})")));
    }
    // SAFETY: our own `IGameInput`: its slots, read from its table.
    let (get_current, get_next, register_reading, register_device) = unsafe {
        (method(input, SLOT_GET_CURRENT_READING), method(input, SLOT_GET_NEXT_READING), method(input, SLOT_REGISTER_READING_CALLBACK), method(input, SLOT_REGISTER_DEVICE_CALLBACK))
    };
    // Any reading that answers as its kind should gives the readings' methods (a fresh `IGameInput`
    // has a mouse reading only once the mouse moves; a pad's comes at once). Asked for up to 2 s.
    // SAFETY: `IGameInput` slot 4 of our own object, `GetCurrentReading`.
    let get_reading = unsafe { std::mem::transmute::<usize, GetCurrentReadingFn>(get_current) };
    let mut found = Err(String::from("no reading of a mouse, gamepad or keyboard"));
    'search: for _ in 0..20 {
        for kind in [KIND_MOUSE, KIND_GAMEPAD, KIND_KEYBOARD] {
            let mut reading: *mut c_void = std::ptr::null_mut();
            // SAFETY: `IGameInput::GetCurrentReading` on our own object; any device of the kind.
            let hr = unsafe { get_reading(input, kind, std::ptr::null_mut(), &mut reading) };
            if hr < 0 || reading.is_null() {
                continue;
            }
            // SAFETY: the reading GameInput handed us: asked, its getters read, then released once.
            let (answer, getters) = unsafe {
                let answer = answers(reading, kind);
                let getters = (method(reading, SLOT_GET_MOUSE_STATE), method(reading, SLOT_GET_GAMEPAD_STATE));
                std::mem::transmute::<usize, ReleaseFn>(method(reading, SLOT_RELEASE))(reading);
                (answer, getters)
            };
            found = answer.map(|()| {
                log!("GameInput: a reading of kind {kind:#x} answers as one; the mouse and gamepad state getters at {:#x}, {:#x}", getters.0, getters.1);
                getters
            });
            break 'search;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    // SAFETY: our IGameInput, released once.
    unsafe { std::mem::transmute::<usize, ReleaseFn>(method(input, SLOT_RELEASE))(input) };
    let (mouse_state, gamepad_state) = found.map_err(|why| Rejection::unsupported(format!("GameInput: {why}")))?;
    Ok(Methods { get_current, get_next, register_reading, register_device, mouse_state, gamepad_state })
}

// --- Our own mouse movement ---------------------------------------------------------------------

static GET_MOUSE_STATE: Original<GetMouseStateFn> = Original::new();
/// Set while counts may go out.
pub static ACTIVE: AtomicBool = AtomicBool::new(false);
/// Counts handed to the game so far (x right, y down): added to every reading's position.
static OFFSET: [AtomicI64; 2] = [AtomicI64::new(0), AtomicI64::new(0)];
static READS: AtomicU64 = AtomicU64::new(0);

/// Queues `dx`, `dy` mouse counts (x right, y down): the game sees them as movement at its next
/// mouse reading.
pub fn add(dx: i64, dy: i64) {
    OFFSET[0].fetch_add(dx, Relaxed);
    OFFSET[1].fetch_add(dy, Relaxed);
}

/// Hooks GameInput's mouse readings (enabled with the producer's other hooks).
pub fn install(hooks: &mut Hooks) -> Result<(), Rejection> {
    let methods = find()?;
    // SAFETY: `IGameInputReading::GetMouseState`, checked on a reading of our own.
    unsafe { hooks.inline_chained(&GET_MOUSE_STATE, "GameInput GetMouseState", methods.mouse_state, get_mouse_state as GetMouseStateFn)? };
    Ok(())
}

unsafe extern "system" fn get_mouse_state(reading: *mut c_void, state: *mut MouseState) -> bool {
    let flight = InFlight::enter();
    // SAFETY: forwards the game's own call.
    let ok = unsafe { GET_MOUSE_STATE.get(&flight)(reading, state) };
    if ok && !state.is_null() && ACTIVE.load(Acquire) {
        READS.fetch_add(1, Relaxed);
        // SAFETY: the game's out-structure, filled by GameInput.
        unsafe {
            (*state).position_x += OFFSET[0].load(Relaxed);
            (*state).position_y += OFFSET[1].load(Relaxed);
        }
    }
    ok
}

/// What went out.
pub fn summary() -> String {
    format!("GameInput mouse: {} readings carried ours, ({}, {}) counts handed out", READS.load(Relaxed), OFFSET[0].load(Relaxed), OFFSET[1].load(Relaxed))
}

// --- The VR controllers as the game's gamepad: a stand-in device --------------------------------

// A game reads a pad through GameInput only as a device GameInput reported to it, so with no
// real pad plugged in there is nothing to read. The stand-in is a device of our own: an object
// with `IGameInputDevice`'s methods (an Xbox One pad, connected; the rest refuse or do nothing),
// which a producer hands to the game's own device callback as connected ([`stand_in`]), and
// whose readings, asked for through `IGameInput::GetCurrentReading`, are an object of our own
// with `IGameInputReading`'s methods carrying the VR pad ([`crate::install`] opens it) and the
// producer's presses and flick, the right stick under the takes. A real pad plugged in goes on
// working beside it.
//
// Before the producer stops, the game must be told the stand-in went ([`stand_in`] again, as
// disconnected), so it never calls the objects once this DLL is gone.

static GET_CURRENT_READING: Original<GetCurrentReadingFn> = Original::new();
static PAD_READS: AtomicU64 = AtomicU64::new(0);
static PAD_INPUT: AtomicU64 = AtomicU64::new(0);
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A COM object: its vtable.
#[repr(C)]
struct Object {
    vtable: *const usize,
}

/// The stand-in device and its reading, built at [`install_pad`].
static DEVICE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
static READING: std::sync::OnceLock<usize> = std::sync::OnceLock::new();

/// `GameInputKindGamepad` and the controller kinds a pad also reports.
const KIND_CONTROLLER_GAMEPAD: u32 = 0x0000_000e | KIND_GAMEPAD;
/// `GameInputDeviceStatus`: connected, input enabled.
const STATUS_CONNECTED: u32 = 0x1 | 0x2;
const E_NOINTERFACE: i32 = 0x8000_4002_u32 as i32;
const E_NOTIMPL: i32 = 0x8000_4001_u32 as i32;

/// `GameInputDeviceInfo` (v0) as far as games read it: vendor and product at +4, the family at
/// +0x60 (Xbox One: 1), the supported input at +0x68, and the gamepad's info at +0x100 (a game may
/// take a device for a gamepad only when it has one: KCD2 makes its gamepad buttons and sticks only
/// then). The rest stays zero.
#[repr(C, align(8))]
struct DeviceInfo {
    head: [u8; 0x100],
    gamepad: *const GamepadInfo,
    tail: [u8; 0xf8],
}

// SAFETY: read-only, pointing at another static.
unsafe impl Sync for DeviceInfo {}

/// `GameInputGamepadInfo` (v0): a label for each button, all `GameInputLabelNone`.
#[repr(C)]
struct GamepadInfo([u32; 16]);

static GAMEPAD_INFO: GamepadInfo = GamepadInfo([0; 16]);

static INFO: DeviceInfo = DeviceInfo { head: INFO_HEAD, gamepad: &GAMEPAD_INFO, tail: [0; 0xf8] };

const INFO_HEAD: [u8; 0x100] = {
    let mut bytes = [0u8; 0x100];
    // infoSize
    bytes[0] = 0x00;
    bytes[1] = 0x02;
    // vendor 0x045e (Microsoft), product 0x02ea (Xbox One S pad)
    bytes[4] = 0x5e;
    bytes[5] = 0x04;
    bytes[6] = 0xea;
    bytes[7] = 0x02;
    // deviceId (32 bytes from +0x20): "monaka stand-in pad"
    let id = b"monaka stand-in gamepad 00000001";
    let mut i = 0;
    while i < 32 {
        bytes[0x20 + i] = id[i];
        bytes[0x40 + i] = id[i];
        i += 1;
    }
    // deviceFamily: Xbox One
    bytes[0x60] = 1;
    // supportedInput: controller + gamepad
    let kinds = KIND_CONTROLLER_GAMEPAD.to_le_bytes();
    bytes[0x68] = kinds[0];
    bytes[0x69] = kinds[1];
    bytes[0x6a] = kinds[2];
    bytes[0x6b] = kinds[3];
    bytes
};

unsafe extern "system" fn query_interface(_this: *mut c_void, _iid: *const c_void, out: *mut *mut c_void) -> i32 {
    let _flight = InFlight::enter();
    if !out.is_null() {
        // SAFETY: the caller's out-pointer.
        unsafe { *out = std::ptr::null_mut() };
    }
    E_NOINTERFACE
}
/// AddRef and Release: the objects are static and live as long as this DLL.
unsafe extern "system" fn add_ref(_this: *mut c_void) -> u32 {
    let _flight = InFlight::enter();
    1
}
/// Returns 0: `S_OK`, `false`, a count of none, a void call done.
unsafe extern "system" fn nothing(_this: *mut c_void) -> usize {
    let _flight = InFlight::enter();
    0
}
unsafe extern "system" fn not_implemented(_this: *mut c_void) -> i32 {
    let _flight = InFlight::enter();
    E_NOTIMPL
}
unsafe extern "system" fn device_info(_this: *mut c_void) -> *const DeviceInfo {
    let _flight = InFlight::enter();
    &INFO
}
unsafe extern "system" fn device_status(_this: *mut c_void) -> u32 {
    let _flight = InFlight::enter();
    STATUS_CONNECTED
}
/// `GetBatteryState(&state)`: unknown (all zero).
unsafe extern "system" fn battery(_this: *mut c_void, out: *mut [u8; 32]) {
    let _flight = InFlight::enter();
    if !out.is_null() {
        // SAFETY: the caller's out-structure (at most 32 bytes in v0).
        unsafe { *out = [0; 32] };
    }
}
unsafe extern "system" fn reading_kind(_this: *mut c_void) -> u32 {
    let _flight = InFlight::enter();
    KIND_GAMEPAD
}
unsafe extern "system" fn reading_sequence(_this: *mut c_void, _kind: u32) -> u64 {
    let _flight = InFlight::enter();
    SEQUENCE.load(Relaxed)
}
/// Microseconds, as GameInput counts them.
unsafe extern "system" fn reading_timestamp(_this: *mut c_void) -> u64 {
    let _flight = InFlight::enter();
    monaka_channel::tick() * 1000
}
unsafe extern "system" fn reading_device(_this: *mut c_void, out: *mut *mut c_void) {
    let _flight = InFlight::enter();
    if !out.is_null() {
        // SAFETY: the caller's out-pointer; the device is static.
        unsafe { *out = DEVICE.get().copied().unwrap_or(0) as *mut c_void };
    }
}
/// `GetRawReport(&report)`: none.
unsafe extern "system" fn reading_raw(_this: *mut c_void, out: *mut *mut c_void) -> bool {
    let _flight = InFlight::enter();
    if !out.is_null() {
        // SAFETY: the caller's out-pointer.
        unsafe { *out = std::ptr::null_mut() };
    }
    false
}

/// XInput's button bits (the VR pad's) and GameInput's for the same buttons.
const BUTTONS: [(u16, u32); 14] = [
    (0x0010, 0x0001), // start: menu
    (0x0020, 0x0002), // back: view
    (0x1000, 0x0004), // A
    (0x2000, 0x0008), // B
    (0x4000, 0x0010), // X
    (0x8000, 0x0020), // Y
    (0x0001, 0x0040), // d-pad up
    (0x0002, 0x0080), // d-pad down
    (0x0004, 0x0100), // d-pad left
    (0x0008, 0x0200), // d-pad right
    (0x0100, 0x0400), // left shoulder
    (0x0200, 0x0800), // right shoulder
    (0x0040, 0x1000), // left stick press
    (0x0080, 0x2000), // right stick press
];

/// `GetGamepadState(&state)`: the VR pad as `GameInputGamepadState` (buttons, then the triggers
/// 0..1 and the sticks -1..1, y up, as floats).
unsafe extern "system" fn reading_gamepad(_this: *mut c_void, out: *mut [u32; 8]) -> bool {
    let _flight = InFlight::enter();
    if out.is_null() {
        return false;
    }
    PAD_READS.fetch_add(1, Relaxed);
    let pad = crate::virtual_pad().unwrap_or_default();
    if pad != monaka_contract::gamepad::Gamepad::default() {
        PAD_INPUT.fetch_add(1, Relaxed);
    }
    let axis = |v: i16| f32::from(v) / 32767.0;
    let mut state = [0u32; 8];
    state[0] = BUTTONS.iter().filter(|(x, _)| pad.buttons & x != 0).fold(0, |bits, (_, g)| bits | g);
    let mut values = [f32::from(pad.left_trigger) / 255.0, f32::from(pad.right_trigger) / 255.0, axis(pad.thumb_lx), axis(pad.thumb_ly), axis(pad.thumb_rx), axis(pad.thumb_ry)];
    let (take, take_y, flick) = crate::right_stick_rules();
    if take {
        (values[4], values[5]) = (0.0, 0.0);
    } else if take_y {
        values[5] = 0.0;
    }
    if let Some([x, y]) = flick {
        (values[4], values[5]) = (axis(x), axis(y));
    }
    for (k, v) in values.into_iter().enumerate() {
        state[1 + k] = v.to_bits();
    }
    // SAFETY: the caller's out-structure, written whole.
    unsafe { *out = state };
    true
}

/// An object of `methods.len()` methods, leaked (it lives as long as this DLL).
fn object(methods: Vec<usize>) -> usize {
    let vtable = Box::leak(methods.into_boxed_slice()).as_ptr();
    Box::leak(Box::new(Object { vtable })) as *mut Object as usize
}

/// Hooks `IGameInput::GetCurrentReading` to answer for the stand-in device, and builds the device
/// and its reading (enabled with the producer's other hooks). Hand the device to the game with
/// [`stand_in`].
pub fn install_pad(hooks: &mut Hooks) -> Result<(), Rejection> {
    let methods = find()?;
    let nothing = nothing as unsafe extern "system" fn(*mut c_void) -> usize as usize;
    let not_implemented = not_implemented as unsafe extern "system" fn(*mut c_void) -> i32 as usize;
    let unknown = [
        query_interface as unsafe extern "system" fn(*mut c_void, *const c_void, *mut *mut c_void) -> i32 as usize,
        add_ref as unsafe extern "system" fn(*mut c_void) -> u32 as usize,
        add_ref as unsafe extern "system" fn(*mut c_void) -> u32 as usize,
    ];
    // `IGameInputDevice` (v0): GetDeviceInfo, GetDeviceStatus, GetBatteryState, then force
    // feedback, haptics, rumble, synchronisation, power and raw access: refused or nothing.
    let mut device = unknown.to_vec();
    device.push(device_info as unsafe extern "system" fn(*mut c_void) -> *const DeviceInfo as usize);
    device.push(device_status as unsafe extern "system" fn(*mut c_void) -> u32 as usize);
    device.push(battery as unsafe extern "system" fn(*mut c_void, *mut [u8; 32]) as usize);
    device.push(not_implemented); // CreateForceFeedbackEffect
    device.extend([nothing; 2]); // IsForceFeedbackMotorPoweredOn, SetForceFeedbackMotorGain
    device.push(not_implemented); // SetHapticMotorState
    device.extend([nothing; 4]); // SetRumbleState, SetInputSynchronizationState, SendInputSynchronizationHint, PowerOff
    device.extend([not_implemented; 7]); // CreateRawDeviceReport .. AcquireExclusiveRawDeviceAccess
    device.push(nothing); // ReleaseExclusiveRawDeviceAccess
    // A few more of nothing, for a caller of a later interface's slot.
    device.extend([nothing; 8]);
    // `IGameInputReading` (v0): GetInputKind, GetSequenceNumber, GetTimestamp, GetDevice,
    // GetRawReport, the controller axes, buttons and switches, keys, mouse, touch, motion, arcade
    // stick, flight stick, gamepad, racing wheel, UI navigation.
    let mut reading = unknown.to_vec();
    reading.push(reading_kind as unsafe extern "system" fn(*mut c_void) -> u32 as usize);
    reading.push(reading_sequence as unsafe extern "system" fn(*mut c_void, u32) -> u64 as usize);
    reading.push(reading_timestamp as unsafe extern "system" fn(*mut c_void) -> u64 as usize);
    reading.push(reading_device as unsafe extern "system" fn(*mut c_void, *mut *mut c_void) as usize);
    reading.push(reading_raw as unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> bool as usize);
    reading.extend([nothing; 11]); // axis, button and switch counts and states, keys, mouse, touch
    reading.extend([nothing; 3]); // motion, arcade stick, flight stick
    reading.push(reading_gamepad as unsafe extern "system" fn(*mut c_void, *mut [u32; 8]) -> bool as usize);
    reading.extend([nothing; 2]); // racing wheel, UI navigation
    reading.extend([nothing; 8]);
    let _ = DEVICE.set(object(device));
    let _ = READING.set(object(reading));
    // SAFETY: `IGameInput::GetCurrentReading`, found on our own object; the detour forwards every
    // call but the stand-in's.
    unsafe { hooks.inline_chained(&GET_CURRENT_READING, "GameInput GetCurrentReading", methods.get_current, get_current_reading as GetCurrentReadingFn)? };
    log!("controller pad: a GameInput stand-in gamepad ready");
    Ok(())
}

unsafe extern "system" fn get_current_reading(this: *mut c_void, kind: u32, device: *mut c_void, out: *mut *mut c_void) -> i32 {
    let flight = InFlight::enter();
    if let (Some(&ours), Some(&reading)) = (DEVICE.get(), READING.get())
        && device as usize == ours
        && kind & KIND_GAMEPAD != 0
        && !out.is_null()
    {
        SEQUENCE.fetch_add(1, Relaxed);
        // SAFETY: the caller's out-pointer; the reading is static.
        unsafe { *out = reading as *mut c_void };
        return 0;
    }
    // SAFETY: forwards the game's own call.
    unsafe { GET_CURRENT_READING.get(&flight)(this, kind, device, out) }
}

/// The game's device callback: `(token, context, device, timestamp, status, previous status)`.
pub type DeviceCallbackFn = unsafe extern "system" fn(u64, *mut c_void, *mut c_void, u64, u32, u32);

/// Tells the game, through its own device callback (`callback` with its `context`), that the
/// stand-in gamepad came (`connected`) or went. Whether there was a stand-in to tell of.
///
/// # Safety
/// `callback` must be the game's `GameInputDeviceCallback` registered with `context`.
pub unsafe fn stand_in(callback: DeviceCallbackFn, context: *mut c_void, connected: bool) -> bool {
    let Some(&device) = DEVICE.get() else { return false };
    let (now, before) = if connected { (STATUS_CONNECTED, 0) } else { (0, STATUS_CONNECTED) };
    // SAFETY: the caller's guarantee; the device is static.
    unsafe { callback(0, context, device as *mut c_void, monaka_channel::tick() * 1000, now, before) };
    log!("controller pad: the GameInput stand-in gamepad {}", if connected { "connected" } else { "disconnected" });
    true
}

/// What the stand-in did.
pub fn pad_summary() -> String {
    format!("GameInput stand-in gamepad: {} states read, {} with VR input", PAD_READS.load(Relaxed), PAD_INPUT.load(Relaxed))
}

// --- The probe: how a game reads through GameInput ---------------------------------------------

static PROBE_CURRENT: Original<GetCurrentReadingFn> = Original::new();
static PROBE_NEXT: Original<GetNextReadingFn> = Original::new();
static PROBE_REGISTER_READING: Original<RegisterReadingFn> = Original::new();
static PROBE_REGISTER_DEVICE: Original<RegisterDeviceFn> = Original::new();
static PROBE_GAMEPAD: Original<GetGamepadStateFn> = Original::new();
/// Calls seen: current readings, next readings, reading callbacks, device callbacks, gamepad states.
static PROBE_CALLS: [AtomicU64; 5] = [const { AtomicU64::new(0) }; 5];
/// The kinds asked for (or-ed), and the readings found, by the current and next reading calls.
static PROBE_KINDS: AtomicU64 = AtomicU64::new(0);
static PROBE_FOUND: AtomicU64 = AtomicU64::new(0);

/// Hooks GameInput's reading and callback calls and the gamepad state getter to log how the game
/// reads its input (calls, kinds, results), changing nothing.
pub fn install_probe(hooks: &mut Hooks) -> Result<(), Rejection> {
    let m = find()?;
    // SAFETY: each is the `IGameInput` or `IGameInputReading` method of its slot, found on our own
    // objects (the reading's checked by its answers); the detours forward every call unchanged.
    unsafe {
        hooks.inline_chained(&PROBE_CURRENT, "GameInput GetCurrentReading", m.get_current, probe_current as GetCurrentReadingFn)?;
        hooks.inline_chained(&PROBE_NEXT, "GameInput GetNextReading", m.get_next, probe_next as GetNextReadingFn)?;
        hooks.inline_chained(&PROBE_REGISTER_READING, "GameInput RegisterReadingCallback", m.register_reading, probe_register_reading as RegisterReadingFn)?;
        hooks.inline_chained(&PROBE_REGISTER_DEVICE, "GameInput RegisterDeviceCallback", m.register_device, probe_register_device as RegisterDeviceFn)?;
        hooks.inline_chained(&PROBE_GAMEPAD, "GameInput GetGamepadState", m.gamepad_state, probe_gamepad as GetGamepadStateFn)?;
    }
    log!("GameInput probe: reading, callback and gamepad state calls hooked");
    Ok(())
}

/// Counts a call of kind `slot`, logging the first few.
fn note(slot: usize, line: impl FnOnce() -> String) {
    if PROBE_CALLS[slot].fetch_add(1, Relaxed) < 6 {
        log!("GameInput probe: {}", line());
    }
}

unsafe extern "system" fn probe_current(this: *mut c_void, kind: u32, device: *mut c_void, out: *mut *mut c_void) -> i32 {
    let flight = InFlight::enter();
    // SAFETY: forwards the game's own call.
    let hr = unsafe { PROBE_CURRENT.get(&flight)(this, kind, device, out) };
    PROBE_KINDS.fetch_or(u64::from(kind), Relaxed);
    if hr >= 0 {
        PROBE_FOUND.fetch_add(1, Relaxed);
    }
    note(0, || format!("GetCurrentReading(kind {kind:#x}, device {device:?}) -> {hr:#x}"));
    hr
}

unsafe extern "system" fn probe_next(this: *mut c_void, reference: *mut c_void, kind: u32, device: *mut c_void, out: *mut *mut c_void) -> i32 {
    let flight = InFlight::enter();
    // SAFETY: forwards the game's own call.
    let hr = unsafe { PROBE_NEXT.get(&flight)(this, reference, kind, device, out) };
    PROBE_KINDS.fetch_or(u64::from(kind), Relaxed);
    if hr >= 0 {
        PROBE_FOUND.fetch_add(1, Relaxed);
    }
    note(1, || format!("GetNextReading(kind {kind:#x}, device {device:?}) -> {hr:#x}"));
    hr
}

unsafe extern "system" fn probe_register_reading(this: *mut c_void, device: *mut c_void, kind: u32, threshold: f32, context: *mut c_void, callback: *mut c_void, token: *mut u64) -> i32 {
    let flight = InFlight::enter();
    // SAFETY: forwards the game's own call.
    let hr = unsafe { PROBE_REGISTER_READING.get(&flight)(this, device, kind, threshold, context, callback, token) };
    note(2, || format!("RegisterReadingCallback(kind {kind:#x}, device {device:?}, callback {}) -> {hr:#x}", Module::describe(callback as usize)));
    hr
}

unsafe extern "system" fn probe_register_device(this: *mut c_void, device: *mut c_void, kind: u32, status: u32, enumeration: u32, context: *mut c_void, callback: *mut c_void, token: *mut u64) -> i32 {
    let flight = InFlight::enter();
    // SAFETY: forwards the game's own call.
    let hr = unsafe { PROBE_REGISTER_DEVICE.get(&flight)(this, device, kind, status, enumeration, context, callback, token) };
    note(3, || format!("RegisterDeviceCallback(kind {kind:#x}, status filter {status:#x}, enumeration {enumeration}, callback {}) -> {hr:#x}", Module::describe(callback as usize)));
    hr
}

unsafe extern "system" fn probe_gamepad(reading: *mut c_void, state: *mut [u32; 8]) -> bool {
    let flight = InFlight::enter();
    // SAFETY: forwards the game's own call.
    let ok = unsafe { PROBE_GAMEPAD.get(&flight)(reading, state) };
    // SAFETY: the game's out-structure, filled when `ok`.
    let seen = if ok && !state.is_null() { Some(unsafe { *state }) } else { None };
    note(4, || {
        let floats = seen.map(|s| s[1..7].iter().map(|&b| f32::from_bits(b)).collect::<Vec<_>>());
        format!("GetGamepadState -> {ok}: buttons {:x?}, triggers and sticks {floats:.2?}", seen.map(|s| s[0]))
    });
    ok
}

/// What the probe saw.
pub fn probe_report() {
    let calls: Vec<u64> = PROBE_CALLS.iter().map(|c| c.load(Relaxed)).collect();
    log!(
        "GameInput probe: {} current and {} next readings asked for (kinds {:#x}, {} found), {} reading and {} device callbacks registered, {} gamepad states read",
        calls[0],
        calls[1],
        PROBE_KINDS.load(Relaxed),
        PROBE_FOUND.load(Relaxed),
        calls[2],
        calls[3],
        calls[4]
    );
}
