//! Probes added while the game runs: a hook at any address, with no signature and no rebuild. Each
//! probe's detour is a small stub made at runtime that saves the registers a call may carry
//! arguments in, hands them to the recorder, puts them back and goes on into the game's code; so
//! any function (any signature, any convention of the Windows x64 kind) can be followed by its
//! address alone. Each call is recorded ([`Call`]): its thread, the frame it fell in ([`present`]
//! counts frames), its four integer and four float arguments, two stack arguments, its caller, and
//! optionally a snapshot of an object one of its arguments points to ([`Snapshot`]). A snapshot
//! marked and compared later ([`mark`], [`diff`]) lists the fields an action changed.
//!
//! A probe may follow returns too ([`Spec::returns`]): the call's return address is swapped for a
//! second stub of the probe's, which records what it returned (rax, xmm0) and how long it took,
//! and goes back to the caller. While such a probe is in, a stack walked through the call (a C++
//! exception thrown across it, a profiler) meets the stub's address instead of the caller's: a
//! return probe is for ordinary functions, briefly.
//!
//! What a probe tells, per frame and per thread: how often a function runs, from where, in what
//! order with the others and the present ([`timeline`]). A research tool: probes are few (up to
//! [`PROBES`] a run, never reused, so a thread late in a removed probe's stub never reaches
//! another's code) and stay until removed; a removed probe's hook is out, its stub kept.
//!
//! Probing code the recorder itself runs (the C runtime's locks, `GetCurrentThreadId`) would
//! recurse: such a probe is the caller's mistake to avoid.

use crate::{Hooks, Original};
use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::*};
use std::time::Instant;
use windows::Win32::System::Diagnostics::Debug::FlushInstructionCache;
use windows::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READ, PAGE_PROTECTION_FLAGS, PAGE_READWRITE, VirtualAlloc, VirtualProtect};
use windows::Win32::System::Threading::{GetCurrentProcess, GetCurrentThreadId};

#[cfg(test)]
mod tests;

/// Probes a run may add.
pub const PROBES: usize = 64;
/// The largest snapshot (bytes).
pub const SNAPSHOT_MAX: usize = 4096;
/// Events kept (calls and presents, the newest), recent calls kept per probe, and calls kept per
/// probe per frame (a function called thousands of times a frame is counted, not all kept).
const EVENTS: usize = 1 << 16;
const RECENT: usize = 64;
const KEPT_PER_FRAME: u32 = 256;
/// Threads and callers told apart per probe.
const THREADS: usize = 16;
const CALLERS: usize = 16;

type Entry = unsafe extern "system" fn();

static ORIGINALS: [Original<Entry>; PROBES] = [const { Original::new() }; PROBES];
/// Where each probe's stub goes on (its hook's trampoline), read by the stub.
static TRAMPOLINES: [AtomicUsize; PROBES] = [const { AtomicUsize::new(0) }; PROBES];
/// Whether each probe records (a stub may still run a moment after its probe is removed).
static ACTIVE: [AtomicBool; PROBES] = [const { AtomicBool::new(false) }; PROBES];
static NEXT: AtomicUsize = AtomicUsize::new(0);
static FRAME: AtomicU64 = AtomicU64::new(0);
static STATE: Mutex<State> = Mutex::new(State { probes: Vec::new(), events: VecDeque::new() });
static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
/// Each probe's filter ([`Spec::only`]), read before the state is locked.
static SPECS_ONLY: [Mutex<Option<(Register, u64)>>; PROBES] = [const { Mutex::new(None) }; PROBES];
/// Each probe's return stub, where it follows returns (0: it does not).
static RETURN_STUBS: [AtomicUsize; PROBES] = [const { AtomicUsize::new(0) }; PROBES];
static SERIAL: AtomicU64 = AtomicU64::new(0);
/// Calls not yet returned from, by the stack slot their return address was in: where they really
/// return to, their probe, their serial, when they began. Never dropped while the call is out (the
/// return stub needs it to go back), so a return stub keeps working after its probe is removed.
static PENDING: Mutex<Option<std::collections::HashMap<usize, Out>>> = Mutex::new(None);

/// A call not yet returned from: its caller, its probe, its serial, when it began.
type Out = (usize, usize, u64, u64);

/// An integer argument register (the first four arguments).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Register {
    Rcx,
    Rdx,
    R8,
    R9,
}

impl Register {
    pub fn parse(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "rcx" | "a0" | "this" => Some(Register::Rcx),
            "rdx" | "a1" => Some(Register::Rdx),
            "r8" | "a2" => Some(Register::R8),
            "r9" | "a3" => Some(Register::R9),
            _ => None,
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// What to copy at each call: `length` bytes from the address in `register`, plus `offset`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub register: Register,
    pub offset: isize,
    pub length: usize,
}

/// A probe to add: where, what to call it, what to copy at each call, and the calls it keeps:
/// all, or only those whose register holds a value (one object of the many a function is called
/// on, so its snapshots are of that object alone).
#[derive(Clone, Debug)]
pub struct Spec {
    pub address: usize,
    pub name: String,
    pub snapshot: Option<Snapshot>,
    pub only: Option<(Register, u64)>,
    /// Records each call's return too.
    pub returns: bool,
}

/// What a call returned: rax, xmm0 (as a float), and how long it took (microseconds).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Returned {
    pub rax: u64,
    pub xmm0: f32,
    pub took: u64,
}

/// One call: the probe, its thread, the frame it fell in, when (microseconds from the first
/// probe), its integer arguments (rcx, rdx, r8, r9), its float arguments (xmm0-3, as floats),
/// the first two stack arguments, and the address it returns to.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Call {
    /// Counts every call recorded in the run.
    pub serial: u64,
    pub probe: usize,
    pub thread: u32,
    pub frame: u64,
    pub time: u64,
    pub args: [u64; 4],
    pub floats: [f32; 4],
    pub stack: [u64; 2],
    pub caller: usize,
    /// Its return, for a probe following returns, once it returned.
    pub returned: Option<Returned>,
}

/// A recorded moment: a call, or a present (a frame's end).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    Call(Call),
    Present { frame: u64, time: u64, thread: u32 },
}

struct Probe {
    spec: Spec,
    /// Kept after removal: a thread may still be in its trampoline.
    hooks: Hooks,
    enabled: bool,
    count: u64,
    since: u64,
    /// The frame of the last call and the calls kept in it.
    frame: (u64, u32),
    threads: Vec<(u32, u64)>,
    callers: Vec<(usize, u64)>,
    recent: VecDeque<Call>,
    latest: Vec<u8>,
    marked: Option<Vec<u8>>,
}

struct State {
    probes: Vec<Option<Probe>>,
    events: VecDeque<Event>,
}

/// What [`probes`] tells of one probe.
#[derive(Clone, Debug)]
pub struct Summary {
    pub id: usize,
    pub name: String,
    pub address: usize,
    pub enabled: bool,
    pub count: u64,
    /// Frames since it was added.
    pub frames: u64,
    /// Threads and callers with their call counts, the busiest first.
    pub threads: Vec<(u32, u64)>,
    pub callers: Vec<(usize, u64)>,
}

fn now() -> u64 {
    START.get_or_init(Instant::now).elapsed().as_micros() as u64
}

/// Adds a probe at `spec.address`; its id. An address whose first instructions cannot be moved
/// (the hook engine's decoder) is refused.
pub fn add(spec: Spec) -> Result<usize, String> {
    let flight = crate::InFlight::enter();
    if spec.snapshot.is_some_and(|s| s.length > SNAPSHOT_MAX) {
        return Err(format!("a snapshot is at most {SNAPSHOT_MAX} bytes"));
    }
    if STATE.lock().map_err(|_| "poisoned")?.probes.iter().flatten().any(|p| p.enabled && p.spec.address == spec.address) {
        return Err(format!("{:#x} is probed already", spec.address));
    }
    let id = NEXT.fetch_add(1, Relaxed);
    if id >= PROBES {
        return Err(format!("all {PROBES} probes of this run are used; stop and start the run for more"));
    }
    let stub = stub(id)?;
    if spec.returns {
        RETURN_STUBS[id].store(return_stub(id)?, Release);
    }
    let mut hooks = Hooks::default();
    // SAFETY: the stub is a detour of any signature: it keeps every argument register and the
    // stack as they were, and goes on to the trampoline the hook makes.
    unsafe {
        let detour: Entry = std::mem::transmute::<usize, Entry>(stub);
        hooks.inline_decoded(&ORIGINALS[id], &spec.name, spec.address, detour).map_err(|e| e.to_string())?;
    }
    TRAMPOLINES[id].store(ORIGINALS[id].get(&flight) as usize, Release);
    if let Ok(mut only) = SPECS_ONLY[id].lock() {
        *only = spec.only;
    }
    {
        let mut state = STATE.lock().map_err(|_| "poisoned")?;
        while state.probes.len() <= id {
            state.probes.push(None);
        }
        let since = FRAME.load(Relaxed);
        state.probes[id] = Some(Probe {
            spec,
            hooks: Hooks::default(),
            enabled: true,
            count: 0,
            since,
            frame: (since, 0),
            threads: Vec::new(),
            callers: Vec::new(),
            recent: VecDeque::new(),
            latest: Vec::new(),
            marked: None,
        });
    }
    ACTIVE[id].store(true, Release);
    if let Err(e) = hooks.enable() {
        ACTIVE[id].store(false, Release);
        let _ = hooks.disable();
        if let Ok(mut state) = STATE.lock() {
            state.probes[id] = None;
        }
        return Err(e.to_string());
    }
    if let Ok(mut state) = STATE.lock()
        && let Some(Some(probe)) = state.probes.get_mut(id)
    {
        probe.hooks = hooks;
    }
    Ok(id)
}

/// Takes probe `id`'s hook out.
pub fn remove(id: usize) -> Result<(), String> {
    ACTIVE.get(id).ok_or("no such probe")?.store(false, Release);
    let mut state = STATE.lock().map_err(|_| "poisoned")?;
    let probe = state.probes.get_mut(id).and_then(Option::as_mut).ok_or("no such probe")?;
    if probe.enabled {
        probe.hooks.disable().map_err(|e| e.to_string())?;
        probe.enabled = false;
    }
    Ok(())
}

/// Takes every probe's hook out (at a stop).
pub fn remove_all() {
    let ids: Vec<usize> = STATE.lock().map(|s| s.probes.iter().enumerate().filter(|(_, p)| p.as_ref().is_some_and(|p| p.enabled)).map(|(id, _)| id).collect()).unwrap_or_default();
    for id in ids {
        let _ = remove(id);
    }
}

/// A frame ended (the present loop calls it at each present).
pub fn present() {
    let frame = FRAME.fetch_add(1, Relaxed) + 1;
    if !ACTIVE.iter().any(|a| a.load(Relaxed)) {
        return;
    }
    // SAFETY: a plain query on our own thread.
    let thread = unsafe { GetCurrentThreadId() };
    if let Ok(mut state) = STATE.lock() {
        push(&mut state.events, Event::Present { frame, time: now(), thread });
    }
}

fn push(events: &mut VecDeque<Event>, event: Event) {
    if events.len() == EVENTS {
        events.pop_front();
    }
    events.push_back(event);
}

/// The registers the stub saved, as it laid them out: xmm0-5 (16 bytes each), then r11, r10, r9,
/// r8, rdx, rcx, rax, then the call's own stack (its return address first).
const SAVED_R9: usize = 0x70;
const SAVED_R8: usize = 0x78;
const SAVED_RDX: usize = 0x80;
const SAVED_RCX: usize = 0x88;
const SAVED_RETURN: usize = 0x98;
/// The first stack argument: past the return address and the four registers' home space.
const SAVED_STACK_ARGS: usize = SAVED_RETURN + 0x28;

/// Called by probe `id`'s stub with the registers it saved.
unsafe extern "system" fn record(id: usize, saved: *const u8) {
    if !ACTIVE.get(id).is_some_and(|a| a.load(Acquire)) {
        return;
    }
    // SAFETY: the stub's own saved area on this thread's stack, laid out as above.
    let word = |at: usize| unsafe { std::ptr::read_unaligned(saved.add(at).cast::<u64>()) };
    let args = [word(SAVED_RCX), word(SAVED_RDX), word(SAVED_R8), word(SAVED_R9)];
    if let Some(Some((register, value))) = SPECS_ONLY.get(id).map(|o| o.lock().ok().and_then(|o| *o))
        && args[register.index()] != value
    {
        return;
    }
    // SAFETY: as above: xmm0-3's low lanes.
    let floats = [0, 1, 2, 3].map(|k| unsafe { std::ptr::read_unaligned(saved.add(k * 16).cast::<f32>()) });
    // The caller's stack beyond its home space: read as memory that may not be there.
    let stack_at = saved as usize + SAVED_STACK_ARGS;
    let stack = [crate::mem::read::<u64>(stack_at).unwrap_or(0), crate::mem::read::<u64>(stack_at + 8).unwrap_or(0)];
    // SAFETY: a plain query on our own thread.
    let thread = unsafe { GetCurrentThreadId() };
    let serial = SERIAL.fetch_add(1, Relaxed);
    let call = Call { serial, probe: id, thread, frame: FRAME.load(Relaxed), time: now(), args, floats, stack, caller: word(SAVED_RETURN) as usize, returned: None };
    let return_stub = RETURN_STUBS[id].load(Acquire);
    if return_stub != 0 {
        let slot = saved as usize + SAVED_RETURN;
        let mut pending = PENDING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        pending.get_or_insert_default().insert(slot, (call.caller, id, serial, call.time));
        // SAFETY: the call's own return address slot on this thread's stack: the call now returns
        // into the probe's return stub, which goes on to the caller.
        unsafe { std::ptr::write_unaligned(slot as *mut u64, return_stub as u64) };
    }
    let Ok(mut state) = STATE.lock() else { return };
    let State { probes, events } = &mut *state;
    let Some(Some(probe)) = probes.get_mut(id) else { return };
    probe.count += 1;
    count(&mut probe.threads, thread, THREADS);
    count(&mut probe.callers, call.caller, CALLERS);
    if probe.frame.0 != call.frame {
        probe.frame = (call.frame, 0);
    }
    if let Some(s) = probe.spec.snapshot {
        let from = (args[s.register.index()] as usize).wrapping_add_signed(s.offset);
        probe.latest.resize(s.length, 0);
        if !crate::mem::read_bytes(from, &mut probe.latest) {
            probe.latest.clear();
        }
    }
    if probe.frame.1 < KEPT_PER_FRAME {
        probe.frame.1 += 1;
        if probe.recent.len() == RECENT {
            probe.recent.pop_front();
        }
        probe.recent.push_back(call);
        push(events, Event::Call(call));
    }
}

/// The return stub's saved registers: xmm0, xmm1, then r11, r10, r9, r8, rdx, rcx, rax, then the
/// slot the caller's address goes in (where the return address was).
const RETURNED_XMM0: usize = 0x00;
const RETURNED_RAX: usize = 0x50;
const RETURNED_SLOT: usize = 0x58;

/// Called by probe `id`'s return stub: the call's return recorded, and where it goes back to put
/// in the slot the stub returns through. Runs even for a removed probe (its calls still return).
unsafe extern "system" fn returned(id: usize, saved: *mut u8) {
    let slot = saved as usize + RETURNED_SLOT;
    let entry = PENDING.lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_mut().and_then(|p| p.remove(&slot));
    // Every call through the stub was entered with its slot noted: the entry is there.
    let Some((caller, probe, serial, began)) = entry else { return };
    // SAFETY: the stub's saved area on this thread's stack: the slot it returns through.
    unsafe { std::ptr::write_unaligned(slot as *mut u64, caller as u64) };
    if probe != id {
        return;
    }
    // SAFETY: as above: rax and xmm0's low lane as the call left them.
    let (rax, xmm0) = unsafe { (std::ptr::read_unaligned(saved.add(RETURNED_RAX).cast::<u64>()), std::ptr::read_unaligned(saved.add(RETURNED_XMM0).cast::<f32>())) };
    let result = Returned { rax, xmm0, took: now().saturating_sub(began) };
    if let Ok(mut state) = STATE.lock()
        && let Some(Some(p)) = state.probes.get_mut(id)
        && let Some(call) = p.recent.iter_mut().rev().find(|c| c.serial == serial)
    {
        call.returned = Some(result);
    }
}

/// One more of `key` in a short tally (the first `most` kept).
fn count<K: PartialEq + Copy>(tally: &mut Vec<(K, u64)>, key: K, most: usize) {
    if let Some((_, n)) = tally.iter_mut().find(|(k, _)| *k == key) {
        *n += 1;
    } else if tally.len() < most {
        tally.push((key, 1));
    }
}

/// Every probe of the run.
pub fn probes() -> Vec<Summary> {
    let Ok(state) = STATE.lock() else { return Vec::new() };
    let frame = FRAME.load(Relaxed);
    state
        .probes
        .iter()
        .enumerate()
        .filter_map(|(id, p)| p.as_ref().map(|p| (id, p)))
        .map(|(id, p)| {
            let mut threads = p.threads.clone();
            threads.sort_by_key(|t| std::cmp::Reverse(t.1));
            let mut callers = p.callers.clone();
            callers.sort_by_key(|c| std::cmp::Reverse(c.1));
            Summary { id, name: p.spec.name.clone(), address: p.spec.address, enabled: p.enabled, count: p.count, frames: frame.saturating_sub(p.since), threads, callers }
        })
        .collect()
}

/// Probe `id`'s latest calls, at most `most`, oldest first.
pub fn calls(id: usize, most: usize) -> Vec<Call> {
    let Ok(state) = STATE.lock() else { return Vec::new() };
    let Some(Some(probe)) = state.probes.get(id) else { return Vec::new() };
    probe.recent.iter().skip(probe.recent.len().saturating_sub(most)).copied().collect()
}

/// What happened in the last `frames` frames, in order.
pub fn timeline(frames: u64) -> Vec<Event> {
    let Ok(state) = STATE.lock() else { return Vec::new() };
    let from = FRAME.load(Relaxed).saturating_sub(frames);
    state
        .events
        .iter()
        .filter(|e| match e {
            Event::Call(c) => c.frame >= from,
            Event::Present { frame, .. } => *frame > from,
        })
        .copied()
        .collect()
}

/// Keeps probe `id`'s latest snapshot as the one later snapshots are compared with.
pub fn mark(id: usize) -> Result<usize, String> {
    let mut state = STATE.lock().map_err(|_| "poisoned")?;
    let probe = state.probes.get_mut(id).and_then(Option::as_mut).ok_or("no such probe")?;
    if probe.spec.snapshot.is_none() {
        return Err("the probe takes no snapshot".into());
    }
    if probe.latest.is_empty() {
        return Err("no snapshot yet (has it been called?)".into());
    }
    probe.marked = Some(probe.latest.clone());
    Ok(probe.latest.len())
}

/// Probe `id`'s latest snapshot.
pub fn snapshot(id: usize) -> Option<Vec<u8>> {
    let state = STATE.lock().ok()?;
    state.probes.get(id)?.as_ref().map(|p| p.latest.clone()).filter(|l| !l.is_empty())
}

/// A 4-byte field that changed: its offset, what it held, what it holds.
pub type Changed = (usize, [u8; 4], [u8; 4]);

/// The 4-byte fields of probe `id`'s latest snapshot that differ from the marked one.
pub fn diff(id: usize) -> Result<Vec<Changed>, String> {
    let state = STATE.lock().map_err(|_| "poisoned")?;
    let probe = state.probes.get(id).and_then(Option::as_ref).ok_or("no such probe")?;
    let marked = probe.marked.as_ref().ok_or("nothing marked: mark it first")?;
    Ok(changed(marked, &probe.latest))
}

/// The 4-byte words that differ between `before` and `after`.
pub fn changed(before: &[u8], after: &[u8]) -> Vec<Changed> {
    before
        .as_chunks::<4>()
        .0
        .iter()
        .zip(after.as_chunks::<4>().0)
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .map(|(k, (a, b))| (k * 4, *a, *b))
        .collect()
}

/// The stub for probe `id`, in a page of its own (never freed).
fn stub(id: usize) -> Result<usize, String> {
    place(&stub_code(id as u64, record as unsafe extern "system" fn(usize, *const u8) as usize as u64, &TRAMPOLINES[id] as *const AtomicUsize as u64))
}

/// `code` in an executable page of its own (never freed).
fn place(code: &[u8]) -> Result<usize, String> {
    // SAFETY: a page of our own, written, then made executable and read-only.
    unsafe {
        let page = VirtualAlloc(None, 4096, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE) as usize;
        if page == 0 || code.len() > 4096 {
            return Err("no memory for the probe's stub".into());
        }
        std::ptr::copy_nonoverlapping(code.as_ptr(), page as *mut u8, code.len());
        let mut previous = PAGE_PROTECTION_FLAGS::default();
        VirtualProtect(page as *const _, 4096, PAGE_EXECUTE_READ, &mut previous).map_err(|e| format!("the stub's page: {e}"))?;
        let _ = FlushInstructionCache(GetCurrentProcess(), Some(page as *const _), code.len());
        Ok(page)
    }
}

/// Probe `id`'s return stub, in a page of its own (never freed).
fn return_stub(id: usize) -> Result<usize, String> {
    place(&return_code(id as u64, returned as unsafe extern "system" fn(usize, *mut u8) as usize as u64))
}

/// The return stub's machine code: entered by the probed call's `ret` (its slot, where the return
/// address was, now just above the stack), it makes that slot again, saves rax, rcx, rdx, r8-r11,
/// xmm0 and xmm1, calls `returned(id, saved)` (which puts the caller's address in the slot),
/// restores them and returns through the slot.
pub fn return_code(id: u64, recorder: u64) -> Vec<u8> {
    let mut code: Vec<u8> = vec![
        0x48, 0x83, 0xEC, 0x08, // sub rsp, 8 (the slot)
        0x50, // push rax
        0x51, // push rcx
        0x52, // push rdx
        0x41, 0x50, // push r8
        0x41, 0x51, // push r9
        0x41, 0x52, // push r10
        0x41, 0x53, // push r11
        0x48, 0x83, 0xEC, 0x20, // sub rsp, 0x20
        0xF3, 0x0F, 0x7F, 0x04, 0x24, // movdqu [rsp], xmm0
        0xF3, 0x0F, 0x7F, 0x4C, 0x24, 0x10, // movdqu [rsp+0x10], xmm1
        0x48, 0xB9, // mov rcx, imm64
    ];
    code.extend(id.to_le_bytes());
    code.extend([0x48, 0x89, 0xE2]); // mov rdx, rsp
    code.extend([0x48, 0x83, 0xEC, 0x20]); // sub rsp, 0x20
    code.extend([0x48, 0xB8]); // mov rax, imm64
    code.extend(recorder.to_le_bytes());
    code.extend([
        0xFF, 0xD0, // call rax
        0x48, 0x83, 0xC4, 0x20, // add rsp, 0x20
        0xF3, 0x0F, 0x6F, 0x04, 0x24, // movdqu xmm0, [rsp]
        0xF3, 0x0F, 0x6F, 0x4C, 0x24, 0x10, // movdqu xmm1, [rsp+0x10]
        0x48, 0x83, 0xC4, 0x20, // add rsp, 0x20
        0x41, 0x5B, // pop r11
        0x41, 0x5A, // pop r10
        0x41, 0x59, // pop r9
        0x41, 0x58, // pop r8
        0x5A, // pop rdx
        0x59, // pop rcx
        0x58, // pop rax
        0xC3, // ret (through the slot, to the caller)
    ]);
    code
}

/// The stub's machine code: entered in place of the probed function (its stack as the caller
/// left it), it saves rax, rcx, rdx, r8-r11 and xmm0-5 (16-byte aligned for the call), calls
/// `recorder(id, saved)`, restores them all and jumps to the address held at `trampoline`
/// (through r11, which carries no argument).
pub fn stub_code(id: u64, recorder: u64, trampoline: u64) -> Vec<u8> {
    let mut code: Vec<u8> = vec![
        0x50, // push rax
        0x51, // push rcx
        0x52, // push rdx
        0x41, 0x50, // push r8
        0x41, 0x51, // push r9
        0x41, 0x52, // push r10
        0x41, 0x53, // push r11
        0x48, 0x83, 0xEC, 0x60, // sub rsp, 0x60
        0xF3, 0x0F, 0x7F, 0x04, 0x24, // movdqu [rsp], xmm0
        0xF3, 0x0F, 0x7F, 0x4C, 0x24, 0x10, // movdqu [rsp+0x10], xmm1
        0xF3, 0x0F, 0x7F, 0x54, 0x24, 0x20, // movdqu [rsp+0x20], xmm2
        0xF3, 0x0F, 0x7F, 0x5C, 0x24, 0x30, // movdqu [rsp+0x30], xmm3
        0xF3, 0x0F, 0x7F, 0x64, 0x24, 0x40, // movdqu [rsp+0x40], xmm4
        0xF3, 0x0F, 0x7F, 0x6C, 0x24, 0x50, // movdqu [rsp+0x50], xmm5
        0x48, 0xB9, // mov rcx, imm64
    ];
    code.extend(id.to_le_bytes());
    code.extend([0x48, 0x89, 0xE2]); // mov rdx, rsp
    code.extend([0x48, 0x83, 0xEC, 0x20]); // sub rsp, 0x20
    code.extend([0x48, 0xB8]); // mov rax, imm64
    code.extend(recorder.to_le_bytes());
    code.extend([
        0xFF, 0xD0, // call rax
        0x48, 0x83, 0xC4, 0x20, // add rsp, 0x20
        0xF3, 0x0F, 0x6F, 0x04, 0x24, // movdqu xmm0, [rsp]
        0xF3, 0x0F, 0x6F, 0x4C, 0x24, 0x10, // movdqu xmm1, [rsp+0x10]
        0xF3, 0x0F, 0x6F, 0x54, 0x24, 0x20, // movdqu xmm2, [rsp+0x20]
        0xF3, 0x0F, 0x6F, 0x5C, 0x24, 0x30, // movdqu xmm3, [rsp+0x30]
        0xF3, 0x0F, 0x6F, 0x64, 0x24, 0x40, // movdqu xmm4, [rsp+0x40]
        0xF3, 0x0F, 0x6F, 0x6C, 0x24, 0x50, // movdqu xmm5, [rsp+0x50]
        0x48, 0x83, 0xC4, 0x60, // add rsp, 0x60
        0x41, 0x5B, // pop r11
        0x41, 0x5A, // pop r10
        0x41, 0x59, // pop r9
        0x41, 0x58, // pop r8
        0x5A, // pop rdx
        0x59, // pop rcx
        0x58, // pop rax
        0x49, 0xBB, // mov r11, imm64
    ]);
    code.extend(trampoline.to_le_bytes());
    code.extend([0x41, 0xFF, 0x23]); // jmp qword ptr [r11]
    code
}
