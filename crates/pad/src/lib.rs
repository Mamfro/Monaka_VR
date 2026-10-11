//! The VR controllers as the game's gamepad: the viewer writes them as one Xbox-layout pad into
//! the hand block ([`monaka_core::pad`]); this hands that pad to the game as XInput pad 0, merged
//! with a real pad when one is plugged in. Every control and menu then works as with a real pad,
//! with the game's own prompts, and nothing is mapped per game.
//!
//! Three XInput calls are caught: reading the pad (`XInputGetState`, or `XInputGetStateEx`, #100,
//! as SDL reads it) and asking what is plugged in (`XInputGetCapabilities`, which reports a pad in
//! slot 0 while the VR pad is live and no real one is: the game sees one plugged in, and it
//! unplugs when the viewer stops). They are caught at both ends ([`install`]), because callers
//! reach XInput two ways: through the XInput DLLs' own code (libraries that look the functions up
//! themselves, as SDL does), and through the game modules' import tables, which the Steam overlay
//! points at its own XInput hooks, so those calls never reach the DLLs' code (in Dying Light
//! the engine's reads go engine, overlay, XInput, driver). A read that passes
//! both is merged once.

pub mod gameinput;
pub mod mouse;
pub mod probe;

use monaka_channel::ChannelName;
use monaka_channel::hands::HandChannel;
use monaka_core::pad::{Gamepad, dither, merge};
use monaka_hook::module::Module;
use monaka_hook::{Hooks, InFlight, Original};
use monaka_producer::{Rejection, log};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::*};

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

const ERROR_SUCCESS: u32 = 0;
/// `XINPUT_DEVTYPE_GAMEPAD` and `XINPUT_DEVSUBTYPE_GAMEPAD`.
const GAMEPAD: u8 = 1;
/// XInput's exports by number (`xinput1_3.dll`, `xinput1_4.dll`).
const GET_STATE_ORDINAL: u16 = 2;
const GET_CAPABILITIES_ORDINAL: u16 = 4;
/// How long a stick holds each of its two jittered values.
const DITHER_MS: u64 = 20;
/// A pad the viewer has not refreshed for this long is gone (the viewer stopped or lost focus).
const PAD_MAX_AGE_MS: u64 = 250;

type GetStateFn = unsafe extern "system" fn(u32, *mut XInputState) -> u32;
type GetCapabilitiesFn = unsafe extern "system" fn(u32, u32, *mut XInputCapabilities) -> u32;

static ACTIVE: AtomicBool = AtomicBool::new(false);
/// Diagnostic: hand out the VR pad alone, the real pad's input dropped (does the game take the
/// real pad through these reads at all?).
static MASK_REAL: AtomicBool = AtomicBool::new(false);

/// Diagnostic switch for [`MASK_REAL`].
pub fn mask_real(on: bool) {
    MASK_REAL.store(on, Relaxed);
}

/// Set while the right stick goes to the producer instead of the game ([`take_right_stick`]).
static TAKE_RIGHT: AtomicBool = AtomicBool::new(false);
/// Set while only the right stick's vertical axis is taken ([`take_right_stick_y`]).
static TAKE_RIGHT_Y: AtomicBool = AtomicBool::new(false);
/// The right stick taken at the last read (x and y as `i16` bits), and the tick of that read.
static RIGHT_STICK: AtomicU32 = AtomicU32::new(0);
static RIGHT_TICK: AtomicU64 = AtomicU64::new(0);
/// The left trigger handed to the game at that read (0..=255).
static LEFT_TRIGGER: AtomicU32 = AtomicU32::new(0);
/// The left trigger's gate ([`gate_left_trigger`]): on, and the state of the press (idle, held
/// back since a tick, let through).
static GATE_LEFT: AtomicBool = AtomicBool::new(false);
static LEFT_PRESS: AtomicU32 = AtomicU32::new(PRESS_IDLE);
static LEFT_PRESS_TICK: AtomicU64 = AtomicU64::new(0);
const PRESS_IDLE: u32 = 0;
const PRESS_HELD: u32 = 1;
const PRESS_OPEN: u32 = 2;
/// XInput's own trigger threshold: a game acts on a pull past it.
const TRIGGER_THRESHOLD: u8 = 30;
/// The longest a press is held back (ms), whatever the producer says.
const GATE_MAX_MS: u64 = 250;

/// Holds each new left trigger press back from the game until the producer lets it through
/// ([`open_left_trigger`], at most [`GATE_MAX_MS`]): a producer that turns the game's aim on the
/// press (a left-hand weapon aimed by its controller) lets the shot go once the aim is there.
pub fn gate_left_trigger(on: bool) {
    GATE_LEFT.store(on, Release);
    if !on {
        LEFT_PRESS.store(PRESS_IDLE, Release);
    }
}

/// Lets a held-back left trigger press through to the game (until the trigger is let go).
pub fn open_left_trigger() {
    let _ = LEFT_PRESS.compare_exchange(PRESS_HELD, PRESS_OPEN, AcqRel, Acquire);
}

/// Whether a left trigger press is being held back from the game now.
pub fn left_trigger_held_back() -> bool {
    LEFT_PRESS.load(Acquire) == PRESS_HELD
}

/// Follows the left trigger's press through the gate; whether the game must see it released.
fn gate_left(raw: u8) -> bool {
    let now = monaka_channel::tick();
    if raw < TRIGGER_THRESHOLD {
        LEFT_PRESS.store(PRESS_IDLE, Release);
        return false;
    }
    match LEFT_PRESS.load(Acquire) {
        PRESS_IDLE if GATE_LEFT.load(Acquire) => {
            LEFT_PRESS.store(PRESS_HELD, Release);
            LEFT_PRESS_TICK.store(now, Release);
            true
        }
        PRESS_IDLE => {
            LEFT_PRESS.store(PRESS_OPEN, Release);
            false
        }
        PRESS_HELD if now.saturating_sub(LEFT_PRESS_TICK.load(Acquire)) >= GATE_MAX_MS => {
            LEFT_PRESS.store(PRESS_OPEN, Release);
            false
        }
        PRESS_HELD => true,
        _ => false,
    }
}

/// Takes the right stick (VR and real pads merged) away from the game: the game reads it centred
/// and the producer reads it with [`right_stick`]. For a producer that feeds the player's look in
/// another way (as mouse input, so the game sees one look device: a game that also gets mouse
/// counts from head aim weakens its stick look while they come).
pub fn take_right_stick(on: bool) {
    TAKE_RIGHT.store(on, Release);
}

/// Takes only the right stick's vertical axis from the game (it reads it centred), for a producer
/// that leaves the game its stick turning but not its stick pitch.
pub fn take_right_stick_y(on: bool) {
    TAKE_RIGHT_Y.store(on, Release);
}

/// The right stick (VR and real pads merged, with a producer's [`press`]) at the game's last pad
/// read, taken or not, x right and y up in -1..=1, if that read was at most `max_age_ms` ago.
pub fn right_stick(max_age_ms: u64) -> Option<[f32; 2]> {
    let tick = RIGHT_TICK.load(Acquire);
    if tick == 0 || monaka_channel::tick().saturating_sub(tick) > max_age_ms {
        return None;
    }
    let bits = RIGHT_STICK.load(Acquire);
    let axis = |v: u16| (v as i16 as f32 / 32767.0).clamp(-1.0, 1.0);
    Some([axis(bits as u16), axis((bits >> 16) as u16)])
}

/// The left trigger (VR and real pads merged) at the game's last pad read, 0..=1, if that read
/// was at most `max_age_ms` ago: a producer aiming a left-hand action while it is held.
pub fn left_trigger(max_age_ms: u64) -> Option<f32> {
    let tick = RIGHT_TICK.load(Acquire);
    if tick == 0 || monaka_channel::tick().saturating_sub(tick) > max_age_ms {
        return None;
    }
    Some(LEFT_TRIGGER.load(Acquire) as f32 / 255.0)
}

/// Keeps `pad`'s right stick (and left trigger) for the producer ([`right_stick`],
/// [`left_trigger`]); with [`TAKE_RIGHT`] also centres the stick for the game.
fn take_right(pad: &mut Gamepad) {
    LEFT_TRIGGER.store(u32::from(pad.left_trigger), Release);
    if gate_left(pad.left_trigger) {
        pad.left_trigger = 0;
    }
    RIGHT_STICK.store(pad.thumb_rx as u16 as u32 | (pad.thumb_ry as u16 as u32) << 16, Release);
    RIGHT_TICK.store(monaka_channel::tick().max(1), Release);
    if TAKE_RIGHT.load(Acquire) {
        pad.thumb_rx = 0;
        pad.thumb_ry = 0;
    } else if TAKE_RIGHT_Y.load(Acquire) {
        pad.thumb_ry = 0;
    }
}
static CHANNEL: Mutex<Option<HandChannel>> = Mutex::new(None);

/// Controls the producer presses itself, on top of the controllers', and the tick
/// (`monaka_channel::tick`) they let go at.
static PRESSED: Mutex<(Gamepad, u64)> = Mutex::new((Gamepad { buttons: 0, left_trigger: 0, right_trigger: 0, thumb_lx: 0, thumb_ly: 0, thumb_rx: 0, thumb_ry: 0 }, 0));

/// Presses `pad`'s controls on the VR pad for `for_ms` (a producer's own input: a melee swing's
/// attack), merged like a real pad's. Replaces an earlier press still held.
pub fn press(pad: Gamepad, for_ms: u64) {
    if let Ok(mut pressed) = PRESSED.lock() {
        *pressed = (pad, monaka_channel::tick() + for_ms);
    }
}

/// Buttons the producer holds down for as long as it says ([`hold_buttons`]).
static HELD_BUTTONS: AtomicU32 = AtomicU32::new(0);

/// Holds `buttons` (XInput button bits) down on the VR pad while `on`, on top of the controllers'
/// and of a [`press`] (a guard pose held: Dishonored's block), until called again with `on` false.
pub fn hold_buttons(buttons: u16, on: bool) {
    if on {
        HELD_BUTTONS.fetch_or(u32::from(buttons), Relaxed);
    } else {
        HELD_BUTTONS.fetch_and(!u32::from(buttons), Relaxed);
    }
}

/// A right-stick flick the producer makes ([`flick_right_stick`]): the stick and the tick it lets
/// go at.
static FLICK: Mutex<([i16; 2], u64)> = Mutex::new(([0; 2], 0));

/// Pushes the right stick to `stick` (x right, y up, -1..=1) for `for_ms`, through a take
/// ([`take_right_stick`], [`take_right_stick_y`]): the producer moves the game's stick on purpose
/// (Kingdom Come picks a sword's attack direction with it), where the controllers' own stick stays
/// taken. Replaces an earlier flick still held.
pub fn flick_right_stick(stick: [f32; 2], for_ms: u64) {
    let axis = |v: f32| (v.clamp(-1.0, 1.0) * 32767.0) as i16;
    if let Ok(mut flick) = FLICK.lock() {
        *flick = ([axis(stick[0]), axis(stick[1])], monaka_channel::tick() + for_ms);
    }
}

/// The flick held now, if any.
fn flick() -> Option<[i16; 2]> {
    FLICK.lock().ok().filter(|f| monaka_channel::tick() < f.1).map(|f| f.0)
}

/// The producer's rewrite of the pad the game reads ([`rewrite`]).
static REWRITE: Mutex<Option<fn(&mut Gamepad)>> = Mutex::new(None);

/// Has `rewrite` change every pad handed to the game, last (after the takes, flicks and gates):
/// a producer that turns the sticks from the game camera's heading into the player's. `None`
/// stops it.
pub fn rewrite(rewrite: Option<fn(&mut Gamepad)>) {
    *REWRITE.lock().unwrap_or_else(|e| e.into_inner()) = rewrite;
}

fn rewritten(pad: &mut Gamepad) {
    if let Some(rewrite) = *REWRITE.lock().unwrap_or_else(|e| e.into_inner()) {
        rewrite(pad);
    }
}

/// What [`press`], [`hold_buttons`] and [`flick_right_stick`] hold now.
fn pressed() -> Gamepad {
    let mut pad = PRESSED.lock().ok().filter(|p| monaka_channel::tick() < p.1).map(|p| p.0).unwrap_or_default();
    pad.buttons |= HELD_BUTTONS.load(Relaxed) as u16;
    if let Some([x, y]) = flick() {
        (pad.thumb_rx, pad.thumb_ry) = (x, y);
    }
    pad
}

const GET_STATE_EX_ORDINAL: u16 = 100;

/// The hooks at one place (an XInput DLL's code, or one module's imports from one XInput DLL):
/// its three functions' originals and our detours for them.
struct ExportHooks {
    state: &'static Original<GetStateFn>,
    state_ex: &'static Original<GetStateFn>,
    capabilities: &'static Original<GetCapabilitiesFn>,
    detours: (GetStateFn, GetStateFn, GetCapabilitiesFn),
}

macro_rules! export_hooks {
    ($name:ident, $state:ident, $state_ex:ident, $capabilities:ident) => {
        static $state: Original<GetStateFn> = Original::new();
        static $state_ex: Original<GetStateFn> = Original::new();
        static $capabilities: Original<GetCapabilitiesFn> = Original::new();
        const $name: ExportHooks = {
            unsafe extern "system" fn state(user: u32, state: *mut XInputState) -> u32 {
                // SAFETY: the game's own call, forwarded with its arguments.
                unsafe { read(&$state, user, state) }
            }
            unsafe extern "system" fn state_ex(user: u32, state: *mut XInputState) -> u32 {
                // SAFETY: as above.
                unsafe { read(&$state_ex, user, state) }
            }
            unsafe extern "system" fn capabilities(user: u32, flags: u32, out: *mut XInputCapabilities) -> u32 {
                // SAFETY: as above.
                unsafe { describe(&$capabilities, user, flags, out) }
            }
            ExportHooks { state: &$state, state_ex: &$state_ex, capabilities: &$capabilities, detours: (state, state_ex, capabilities) }
        };
    };
}

export_hooks!(SET_1, STATE_1, STATE_EX_1, CAPABILITIES_1);
export_hooks!(SET_2, STATE_2, STATE_EX_2, CAPABILITIES_2);
export_hooks!(SET_3, STATE_3, STATE_EX_3, CAPABILITIES_3);
export_hooks!(SET_4, STATE_4, STATE_EX_4, CAPABILITIES_4);
export_hooks!(SET_5, STATE_5, STATE_EX_5, CAPABILITIES_5);
export_hooks!(SET_6, STATE_6, STATE_EX_6, CAPABILITIES_6);
/// One set per hooked place; more places than sets are logged and left alone.
const SETS: [ExportHooks; 6] = [SET_1, SET_2, SET_3, SET_4, SET_5, SET_6];
/// The XInput DLLs whose code is hooked when loaded (system copies, by full path: a game folder may
/// hold a proxy of the same name).
const XINPUT_DLLS: [&str; 3] = ["XInput1_4.dll", "XInput1_3.dll", "XInput9_1_0.dll"];

fn open_block(channel: &ChannelName) -> Result<(), Rejection> {
    let block = HandChannel::open(channel).map_err(|e| Rejection::not_ready(format!("controller pad: hand block: {e}")))?;
    *CHANNEL.lock().unwrap_or_else(|e| e.into_inner()) = Some(block);
    Ok(())
}

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
fn hook_imports(hooks: &mut Hooks, module: &Module, dll: &str, set: &ExportHooks) -> Result<usize, Rejection> {
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

fn hook_dll(hooks: &mut Hooks, xinput: &Module, set: &ExportHooks) -> Result<(), Rejection> {
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

/// A HID device interface arriving, for [`announce`]. Static: the posted message points at it.
static ARRIVAL: windows::Win32::UI::WindowsAndMessaging::DEV_BROADCAST_DEVICEINTERFACE_W = windows::Win32::UI::WindowsAndMessaging::DEV_BROADCAST_DEVICEINTERFACE_W {
    dbcc_size: size_of::<windows::Win32::UI::WindowsAndMessaging::DEV_BROADCAST_DEVICEINTERFACE_W>() as u32,
    dbcc_devicetype: 5, // DBT_DEVTYP_DEVICEINTERFACE
    dbcc_reserved: 0,
    // GUID_DEVINTERFACE_HID
    dbcc_classguid: windows::core::GUID::from_u128(0x4d1e55b2_f16f_11cf_88cb_001111000030),
    dbcc_name: [0],
};

/// The thread [`announce`] starts, joined at [`stop`].
static ANNOUNCER: Mutex<Option<std::thread::JoinHandle<()>>> = Mutex::new(None);

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
    *ANNOUNCER.lock().unwrap_or_else(|e| e.into_inner()) = Some(watcher);
}

/// Tells the game's windows a device came with `WM_DEVICECHANGE`: posts "device nodes changed" and
/// sends a HID interface arrival. The arrival carries a pointer, so Windows refuses to post it
/// (a posted arrival reaches no window): it is sent, and waited for.
fn post_arrival() {
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
    if let Ok(mut flick) = FLICK.lock() {
        flick.1 = 0;
    }
    if let Some(watcher) = ANNOUNCER.lock().unwrap_or_else(|e| e.into_inner()).take() {
        let _ = watcher.join();
    }
}

/// Closes the hand block after the hooks are off.
pub fn release() {
    *CHANNEL.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// The VR pad, while the viewer keeps it fresh.
fn vr_pad() -> Option<Gamepad> {
    if !ACTIVE.load(Acquire) {
        return None;
    }
    CHANNEL.lock().ok()?.as_mut()?.read_pad(PAD_MAX_AGE_MS)
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
    (TAKE_RIGHT.load(Acquire), TAKE_RIGHT_Y.load(Acquire), flick())
}

struct Counters {
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
static COUNTERS: Counters = Counters {
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
fn next_packet(last: u32, real: u32, floor: u32) -> u32 {
    last.wrapping_add(1).max(real.wrapping_add(1)).max(floor).max(1)
}

/// A number above any packet number a real pad can have reached since the PC started: half the
/// milliseconds since then (a pad's number goes up once per changed state, at most once a read,
/// a few milliseconds apart), kept under 2^31 so games that compare numbers as signed differences
/// see it as newer.
fn packet_floor() -> u32 {
    ((monaka_channel::tick() / 2) % (1 << 31)) as u32
}

/// Whether a left stick is pushed past a quarter of its range.
fn left_stick_pushed(pad: &Gamepad) -> bool {
    pad.thumb_lx.unsigned_abs() > 8192 || pad.thumb_ly.unsigned_abs() > 8192
}

/// The last pad handed out and its packet number (XInput's change counter: games skip a state
/// whose number they have seen).
static LAST: Mutex<(Gamepad, u32)> = Mutex::new((Gamepad { buttons: 0, left_trigger: 0, right_trigger: 0, thumb_lx: 0, thumb_ly: 0, thumb_rx: 0, thumb_ry: 0 }, 0));

thread_local! {
    /// Set while a read is being answered on this thread: an XInput read made from inside another
    /// one (GetState through GetStateEx, or a proxy through the system DLL) is merged only once.
    static READING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// # Safety
/// `original` is the hooked read the game called, with the game's arguments.
unsafe fn read(original: &Original<GetStateFn>, user: u32, state: *mut XInputState) -> u32 {
    let _flight = InFlight::enter();
    let outer = !READING.with(|r| r.replace(true));
    // SAFETY: forwards the game's own call.
    let result = unsafe { original.get()(user, state) };
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
    } else if TAKE_RIGHT_Y.load(Acquire) {
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
    let _flight = InFlight::enter();
    // SAFETY: forwards the game's own call.
    let result = unsafe { original.get()(user, flags, capabilities) };
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
fn trace_read(real_packet: u32, real: &Gamepad, packet: u32, pad: &Gamepad) {
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
static READERS: Mutex<Vec<(String, u64)>> = Mutex::new(Vec::new());

fn note_reader() {
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
mod tests {
    use super::*;

    #[test]
    fn layouts_are_xinputs() {
        assert_eq!(size_of::<XInputState>(), 16);
        assert_eq!(size_of::<XInputCapabilities>(), 20);
        assert_eq!(std::mem::offset_of!(XInputCapabilities, vibration), 16);
    }

    #[test]
    fn packets_pass_the_real_pads_own_and_the_floor() {
        assert_eq!(next_packet(0, 0, 0), 1);
        assert_eq!(next_packet(5, 0, 0), 6, "no real pad: counts on");
        assert_eq!(next_packet(1, 461, 0), 462, "a real pad already seen at 461: above it");
        assert_eq!(next_packet(462, 461, 0), 463);
        assert_eq!(next_packet(0, 0, 900_000), 900_000, "a pad switched off since: above the floor");
        assert_eq!(next_packet(900_000, 0, 900_000), 900_001);
        assert_eq!(next_packet(u32::MAX, 0, 0), 1, "wraps, never 0");
        assert!(packet_floor() < 1 << 31);
    }
}
