//! Which of a module's functions run: a breakpoint (`int3`) on the first byte of every function the
//! module's exception directory lists, each taken out when hit (or after a few hits, its budget,
//! re-armed in between by a single step), every hit counted. What runs only while the player does
//! something is that something's code: arm, let the idle frames hit what they hit, [`mark`], act,
//! and read what ran since ([`since`]). Re-arming those ([`rearm`]) and doing something else
//! narrows them to what did not run then ([`not_hit`]). With a [`value`] set, each hit also checks
//! the four argument registers for it: the functions handed an object.
//!
//! A research tool, and a heavy one: the first moments after arming are slow (each first call
//! traps), and code that checks its own bytes (anti-tamper) will see the breakpoints. A function
//! whose first byte is already a jump (a hook, a thunk) or a breakpoint is left out, and so is the
//! module the handler itself is in.

use crate::module::Module;
use crate::threads::Frozen;
use std::sync::Mutex;
use std::sync::atomic::{AtomicPtr, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering::*};
use windows::Win32::System::Diagnostics::Debug::{AddVectoredExceptionHandler, EXCEPTION_POINTERS, FlushInstructionCache};
use windows::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, PAGE_PROTECTION_FLAGS, VirtualProtect};
use windows::Win32::System::Threading::GetCurrentProcess;

#[cfg(test)]
mod tests;

const EXCEPTION_BREAKPOINT: u32 = 0x8000_0003;
const EXCEPTION_SINGLE_STEP: u32 = 0x8000_0004;
const EXCEPTION_CONTINUE_EXECUTION: i32 = -1;
const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
const TRAP_FLAG: u32 = 0x100;
const INT3: u8 = 0xCC;
/// Threads that may be stepping past a function's first instruction at once.
const STEPPING: usize = 256;

/// A function's state: not armed, armed (its first byte a breakpoint).
const IDLE: u8 = 0;
const ARMED: u8 = 1;

/// The functions followed: their starts (sorted), first bytes, states and counts. Made once per
/// arming of a module and never freed (the handler may read it at any moment).
struct Table {
    starts: Vec<usize>,
    originals: Vec<u8>,
    states: Vec<AtomicU8>,
    hits: Vec<AtomicU32>,
    /// The order of each one's first hit (0: never).
    first: Vec<AtomicU64>,
    /// Hits counted at the last mark.
    marked: Vec<AtomicU32>,
    /// Which argument registers held the value (bit 0 rcx .. bit 3 r9).
    matched: Vec<AtomicU8>,
    /// The sections made writable, with their protections before.
    sections: Vec<(usize, usize, PAGE_PROTECTION_FLAGS)>,
}

static TABLE: AtomicPtr<Table> = AtomicPtr::new(std::ptr::null_mut());
static HANDLER: AtomicUsize = AtomicUsize::new(0);
static BUDGET: AtomicU32 = AtomicU32::new(1);
static ORDER: AtomicU64 = AtomicU64::new(1);
static VALUE: AtomicU64 = AtomicU64::new(0);
/// Whether the armed functions' pages are writable (from arming to disarming).
static WRITABLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Threads stepping past a function's first instruction, with the function to arm again after.
static STEPS: [(AtomicU32, AtomicUsize); STEPPING] = [const { (AtomicU32::new(0), AtomicUsize::new(0)) }; STEPPING];
/// Commands take turns.
static TURN: Mutex<()> = Mutex::new(());

fn table() -> Option<&'static Table> {
    // SAFETY: a table once stored is never freed.
    unsafe { TABLE.load(Acquire).as_ref() }
}

/// One function that ran: its start, how many hits were counted, the order of its first hit, and
/// which argument registers held the value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    pub start: usize,
    pub hits: u32,
    pub order: u64,
    pub matched: u8,
}

/// The starts of every function `module` lists, as the exception directory has them.
pub fn functions(module: &Module) -> Result<Vec<usize>, String> {
    let code = module.kept_code().ok_or("the module's code is unreadable")?;
    let mut starts: Vec<usize> = code.all_pieces().filter(|(start, _, primary)| start == primary).map(|(start, _, _)| module.base() + start as usize).collect();
    starts.sort_unstable();
    starts.dedup();
    Ok(starts)
}

/// Arms every function of `module` with `budget` hits each; how many were armed.
pub fn arm(module: &Module, budget: u32) -> Result<usize, String> {
    let handler = handler as unsafe extern "system" fn(*mut EXCEPTION_POINTERS) -> i32 as usize;
    if module.contains(handler) {
        return Err("the module the handler is in cannot be followed".into());
    }
    arm_functions(functions(module)?, budget)
}

/// Arms the functions starting at `starts` (sorted, inside executable sections) with `budget`
/// hits each; how many were armed (those starting with a jump or a breakpoint are left out).
pub fn arm_functions(starts: Vec<usize>, budget: u32) -> Result<usize, String> {
    let _turn = TURN.lock().map_err(|_| "poisoned")?;
    if table().is_some_and(|t| t.states.iter().any(|s| s.load(Relaxed) == ARMED)) {
        return Err("functions are armed already: disarm first".into());
    }
    install_handler()?;
    let mut originals = Vec::with_capacity(starts.len());
    let mut kept = Vec::with_capacity(starts.len());
    for start in starts {
        let Some(byte) = crate::mem::read::<u8>(start) else { continue };
        let next = crate::mem::read::<u8>(start + 1).unwrap_or(0);
        // A jump (a hook's, a thunk's) or a breakpoint already there is left alone.
        if matches!(byte, INT3 | 0xE9 | 0xEB) || (byte == 0xFF && next == 0x25) {
            continue;
        }
        kept.push(start);
        originals.push(byte);
    }
    let sections = writable(&kept)?;
    let count = kept.len();
    let table = Box::leak(Box::new(Table {
        states: (0..count).map(|_| AtomicU8::new(ARMED)).collect(),
        hits: (0..count).map(|_| AtomicU32::new(0)).collect(),
        first: (0..count).map(|_| AtomicU64::new(0)).collect(),
        marked: (0..count).map(|_| AtomicU32::new(0)).collect(),
        matched: (0..count).map(|_| AtomicU8::new(0)).collect(),
        starts: kept,
        originals,
        sections,
    }));
    BUDGET.store(budget.max(1), Relaxed);
    TABLE.store(table, Release);
    WRITABLE.store(true, Release);
    let indices: Vec<usize> = (0..count).collect();
    write_all(table, &indices, true);
    Ok(count)
}

/// Makes the pages holding `starts` writable (whole runs of them at once); what they were.
fn writable(starts: &[usize]) -> Result<Vec<(usize, usize, PAGE_PROTECTION_FLAGS)>, String> {
    let mut runs: Vec<(usize, usize)> = Vec::new();
    for &start in starts {
        let page = start & !0xfff;
        match runs.last_mut() {
            Some((from, to)) if page <= *to + 0x10000 && page >= *from => *to = (*to).max(page + 0x1000),
            _ => runs.push((page, page + 0x1000)),
        }
    }
    let mut changed = Vec::with_capacity(runs.len());
    for (from, to) in runs {
        let mut previous = PAGE_PROTECTION_FLAGS::default();
        // SAFETY: pages of the game's own code, made writable while armed; put back at disarm.
        unsafe { VirtualProtect(from as *const _, to - from, PAGE_EXECUTE_READWRITE, &mut previous) }.map_err(|e| format!("the code at {from:#x} cannot be made writable: {e}"))?;
        changed.push((from, to - from, previous));
    }
    Ok(changed)
}

/// Writes a breakpoint (`arm`) or the first byte back at the functions `indices`, with the other
/// threads frozen (nothing allocated meanwhile).
fn write_all(table: &Table, indices: &[usize], arm: bool) {
    let frozen = Frozen::others();
    for &k in indices {
        let at = table.starts[k];
        // SAFETY: the first byte of a function in pages made writable at arming.
        unsafe { std::ptr::write_volatile(at as *mut u8, if arm { INT3 } else { table.originals[k] }) };
        table.states[k].store(if arm { ARMED } else { IDLE }, Release);
    }
    // SAFETY: flushes the whole range written (our own process).
    unsafe {
        if let (Some(first), Some(last)) = (indices.first(), indices.last()) {
            let (from, to) = (table.starts[*first], table.starts[*last] + 1);
            let _ = FlushInstructionCache(GetCurrentProcess(), Some(from as *const _), to.saturating_sub(from));
        }
    }
    drop(frozen);
}

/// Takes every breakpoint out and puts the code's protection back; how many were still in.
pub fn disarm() -> usize {
    let Ok(_turn) = TURN.lock() else { return 0 };
    let Some(table) = table() else { return 0 };
    let armed: Vec<usize> = (0..table.starts.len()).filter(|k| table.states[*k].load(Acquire) == ARMED).collect();
    write_all(table, &armed, false);
    VALUE.store(0, Relaxed);
    WRITABLE.store(false, Release);
    for (from, length, previous) in &table.sections {
        let mut ignored = PAGE_PROTECTION_FLAGS::default();
        // SAFETY: the protection the pages had before arming.
        let _ = unsafe { VirtualProtect(*from as *const _, *length, *previous, &mut ignored) };
    }
    armed.len()
}

/// Takes the breakpoint at `start` out, if one is in (before a probe's hook goes there).
pub fn release_at(start: usize) {
    let Ok(_turn) = TURN.lock() else { return };
    if let Some(table) = table()
        && let Ok(k) = table.starts.binary_search(&start)
        && table.states[k].load(Acquire) == ARMED
        && WRITABLE.load(Acquire)
    {
        write_all(table, &[k], false);
    }
}

/// Whether any breakpoint is in.
pub fn armed() -> bool {
    table().is_some_and(|t| t.states.iter().any(|s| s.load(Relaxed) == ARMED))
}

/// From now on, hits also check the argument registers for `value` (None: no more).
pub fn value(value: Option<u64>) {
    VALUE.store(value.unwrap_or(0), Relaxed);
}

/// Counts the hits so far as the mark's: [`since`] lists what runs after it.
pub fn mark() {
    if let Some(table) = table() {
        for (marked, hits) in table.marked.iter().zip(&table.hits) {
            marked.store(hits.load(Relaxed), Relaxed);
        }
    }
}

fn hit(table: &Table, k: usize) -> Hit {
    Hit { start: table.starts[k], hits: table.hits[k].load(Relaxed), order: table.first[k].load(Relaxed), matched: table.matched[k].load(Relaxed) }
}

/// Every function hit since arming, in the order of their first hits.
pub fn hits() -> Vec<Hit> {
    let Some(table) = table() else { return Vec::new() };
    let mut out: Vec<Hit> = (0..table.starts.len()).filter(|k| table.hits[*k].load(Relaxed) > 0).map(|k| hit(table, k)).collect();
    out.sort_by_key(|h| h.order);
    out
}

/// The functions that ran after the mark and not before it, in the order they first ran.
pub fn since() -> Vec<Hit> {
    let Some(table) = table() else { return Vec::new() };
    let mut out: Vec<Hit> = (0..table.starts.len()).filter(|k| table.marked[*k].load(Relaxed) == 0 && table.hits[*k].load(Relaxed) > 0).map(|k| hit(table, k)).collect();
    out.sort_by_key(|h| h.order);
    out
}

/// The functions whose argument registers held the value.
pub fn matches() -> Vec<Hit> {
    let Some(table) = table() else { return Vec::new() };
    (0..table.starts.len()).filter(|k| table.matched[*k].load(Relaxed) != 0).map(|k| hit(table, k)).collect()
}

/// Arms again only the functions starting at `starts` (with their counts cleared), the others
/// left out; how many.
pub fn rearm(starts: &[usize]) -> Result<usize, String> {
    let _turn = TURN.lock().map_err(|_| "poisoned")?;
    let table = table().ok_or("nothing was armed")?;
    if !WRITABLE.load(Acquire) {
        return Err("disarmed: arm again".into());
    }
    let mut indices: Vec<usize> = starts.iter().filter_map(|s| table.starts.binary_search(s).ok()).collect();
    indices.sort_unstable();
    indices.dedup();
    let armed: Vec<usize> = (0..table.starts.len()).filter(|k| table.states[*k].load(Acquire) == ARMED).collect();
    write_all(table, &armed, false);
    for k in 0..table.starts.len() {
        table.hits[k].store(0, Relaxed);
        table.marked[k].store(0, Relaxed);
        table.first[k].store(0, Relaxed);
        table.matched[k].store(0, Relaxed);
    }
    write_all(table, &indices, true);
    Ok(indices.len())
}

/// The functions armed and not hit since.
pub fn not_hit() -> Vec<usize> {
    let Some(table) = table() else { return Vec::new() };
    (0..table.starts.len()).filter(|k| table.states[*k].load(Relaxed) == ARMED && table.hits[*k].load(Relaxed) == 0).map(|k| table.starts[k]).collect()
}

fn install_handler() -> Result<(), String> {
    if HANDLER.load(Acquire) != 0 {
        return Ok(());
    }
    // SAFETY: the handler touches only the table (never freed) and static atomics; it stays
    // valid while this DLL is loaded (a producer is never unloaded).
    let handle = unsafe { AddVectoredExceptionHandler(1, Some(handler)) };
    if handle.is_null() {
        return Err("could not add the exception handler".into());
    }
    HANDLER.store(handle as usize, Release);
    Ok(())
}

/// A breakpoint of ours: counted, its byte put back, the thread sent on (stepping one instruction
/// first to arm it again while its budget lasts). A single step of ours: the breakpoint back in.
unsafe extern "system" fn handler(info: *mut EXCEPTION_POINTERS) -> i32 {
    // SAFETY: the system passes valid records for the exception being dispatched.
    let (record, context) = unsafe { (&*(*info).ExceptionRecord, &mut *(*info).ContextRecord) };
    let code = record.ExceptionCode.0 as u32;
    let Some(table) = table() else { return EXCEPTION_CONTINUE_SEARCH };
    let thread = crate::thread_id();
    if code == EXCEPTION_SINGLE_STEP {
        let Some(step) = STEPS.iter().find(|(t, _)| t.load(Acquire) == thread) else { return EXCEPTION_CONTINUE_SEARCH };
        let k = step.1.load(Acquire);
        if table.states[k].load(Acquire) == ARMED {
            // SAFETY: the function's first byte, in pages writable while armed.
            unsafe { std::ptr::write_volatile(table.starts[k] as *mut u8, INT3) };
        }
        step.0.store(0, Release);
        context.EFlags &= !TRAP_FLAG;
        return EXCEPTION_CONTINUE_EXECUTION;
    }
    if code != EXCEPTION_BREAKPOINT {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let at = record.ExceptionAddress as usize;
    let Ok(k) = table.starts.binary_search(&at) else { return EXCEPTION_CONTINUE_SEARCH };
    if table.states[k].load(Acquire) != ARMED {
        // Another thread took it out a moment ago: run the function as it is.
        context.Rip = at as u64;
        return EXCEPTION_CONTINUE_EXECUTION;
    }
    // SAFETY: as above: the first byte back, so the function runs as it was.
    unsafe { std::ptr::write_volatile(at as *mut u8, table.originals[k]) };
    let hits = table.hits[k].fetch_add(1, Relaxed) + 1;
    if hits == 1 {
        table.first[k].store(ORDER.fetch_add(1, Relaxed), Relaxed);
    }
    let value = VALUE.load(Relaxed);
    if value != 0 {
        let held = [context.Rcx, context.Rdx, context.R8, context.R9];
        let bits = held.iter().enumerate().filter(|(_, r)| **r == value).fold(0u8, |b, (i, _)| b | 1 << i);
        if bits != 0 {
            table.matched[k].fetch_or(bits, Relaxed);
        }
    }
    context.Rip = at as u64;
    // Under its budget: one step past its first instruction, then the breakpoint back in.
    let free = (hits < BUDGET.load(Relaxed)).then(|| STEPS.iter().find(|(t, _)| t.compare_exchange(0, thread, AcqRel, Relaxed).is_ok())).flatten();
    match free {
        Some(step) => {
            step.1.store(k, Release);
            context.EFlags |= TRAP_FLAG;
        }
        None => table.states[k].store(IDLE, Release),
    }
    EXCEPTION_CONTINUE_EXECUTION
}
