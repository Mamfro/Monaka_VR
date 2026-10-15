//! Finding the code that writes a value: hardware data breakpoints (the debug registers, up to
//! four addresses of 4 bytes) on every thread of the process, and a vectored handler that
//! records each write's instruction and a snapshot of its stack, then lets the thread go on. A
//! research tool: arm it briefly, read [`hits`], disarm. Threads started after arming are not
//! watched. Nothing is allocated in the handler.

use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering::*};
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Diagnostics::Debug::{
    AddVectoredExceptionHandler, CONTEXT, EXCEPTION_POINTERS, GetThreadContext, RemoveVectoredExceptionHandler,
    SetThreadContext,
};
use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next};
use windows::Win32::System::Threading::{
    GetCurrentProcessId, GetCurrentThreadId, OpenThread, ResumeThread, SuspendThread, THREAD_GET_CONTEXT, THREAD_SET_CONTEXT, THREAD_SUSPEND_RESUME,
};

pub const SLOTS: usize = 4;
/// Stack qwords kept per hit, from the stack pointer after the write.
pub const STACK: usize = 24;
const MAX_HITS: usize = 64;
const EXCEPTION_SINGLE_STEP: u32 = 0x8000_0004;
const EXCEPTION_CONTINUE_EXECUTION: i32 = -1;
const EXCEPTION_CONTINUE_SEARCH: i32 = 0;

/// One recorded write: which watched address (slot), the instruction after the write, the
/// thread, and the stack.
#[derive(Clone, Copy, Debug)]
pub struct Hit {
    pub slot: usize,
    pub rip: usize,
    pub thread: u32,
    pub stack: [usize; STACK],
}

struct Store {
    count: AtomicU32,
    slots: [AtomicUsize; MAX_HITS],
    rips: [AtomicUsize; MAX_HITS],
    threads: [AtomicU32; MAX_HITS],
    stacks: [[AtomicUsize; STACK]; MAX_HITS],
}

#[allow(clippy::declare_interior_mutable_const)]
const ZERO: AtomicUsize = AtomicUsize::new(0);
#[allow(clippy::declare_interior_mutable_const)]
const ZERO32: AtomicU32 = AtomicU32::new(0);
#[allow(clippy::declare_interior_mutable_const)]
const ZERO_STACK: [AtomicUsize; STACK] = [ZERO; STACK];

static STORE: Store = Store { count: ZERO32, slots: [ZERO; MAX_HITS], rips: [ZERO; MAX_HITS], threads: [ZERO32; MAX_HITS], stacks: [ZERO_STACK; MAX_HITS] };
static HANDLER: AtomicUsize = AtomicUsize::new(0);
/// Hits per slot to record before that slot's breakpoint is taken off the thread that hit it.
static PER_SLOT: AtomicU32 = AtomicU32::new(4);
/// Instructions (the address after the access) whose hits are not recorded: code already known
/// to touch the value every frame, so a rarer access is not crowded out.
static IGNORED: [AtomicUsize; 16] = [const { AtomicUsize::new(0) }; 16];

/// Hits at these instruction addresses (as [`Hit::rip`] reports them) are skipped from the next
/// [`arm`] on; up to 16.
pub fn ignore(rips: &[usize]) {
    for (cell, rip) in IGNORED.iter().zip(rips.iter().copied().chain(std::iter::repeat(0))) {
        cell.store(rip, Relaxed);
    }
}
static SLOT_HITS: [AtomicU32; SLOTS] = [ZERO32; SLOTS];

unsafe extern "system" fn vectored(info: *mut EXCEPTION_POINTERS) -> i32 {
    // SAFETY: the system passes valid records for the exception being dispatched.
    let (record, context) = unsafe { (&*(*info).ExceptionRecord, &mut *(*info).ContextRecord) };
    if record.ExceptionCode.0 as u32 != EXCEPTION_SINGLE_STEP || context.Dr6 & 0xf == 0 {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    for (slot, slot_hits) in SLOT_HITS.iter().enumerate() {
        if context.Dr6 & (1 << slot) == 0 {
            continue;
        }
        let rip = crate::arch::instruction_pointer(context);
        if IGNORED.iter().any(|cell| cell.load(Relaxed) == rip) {
            continue;
        }
        let seen = slot_hits.fetch_add(1, Relaxed);
        if seen < PER_SLOT.load(Relaxed) {
            let i = STORE.count.fetch_add(1, Relaxed) as usize;
            if i < MAX_HITS {
                STORE.slots[i].store(slot, Relaxed);
                STORE.rips[i].store(rip, Relaxed);
                STORE.threads[i].store(crate::thread_id(), Relaxed);
                for (k, cell) in STORE.stacks[i].iter().enumerate() {
                    let at = crate::arch::stack_pointer(context) + size_of::<usize>() * k;
                    cell.store(crate::mem::read::<usize>(at).unwrap_or(0), Relaxed);
                }
            }
        } else {
            // Enough from this slot: off on this thread (the others stop at their next hit).
            context.Dr7 &= !(0b11 << (2 * slot));
        }
    }
    context.Dr6 = 0;
    EXCEPTION_CONTINUE_EXECUTION
}

/// `GetThreadContext` wants the record 16-byte aligned; the binding's type alone is not.
#[repr(C, align(16))]
struct Aligned(CONTEXT);

/// Debug register 7 for write watches of 4 bytes on the first `count` slots.
fn dr7_for(count: usize) -> u64 {
    dr7_with(count, 0b1101)
}

/// Debug register 7 for watches of 4 bytes on the first `count` slots: length and access bits per
/// slot (`0b1101` writes, `0b1111` reads and writes).
fn dr7_with(count: usize, bits: u64) -> u64 {
    (0..count).fold(0u64, |dr7, slot| dr7 | 1 << (2 * slot) | bits << (16 + 4 * slot))
}

/// Sets the debug registers of every other thread of the process; returns how many took them.
fn set_all(addresses: [usize; SLOTS], dr7: u64) -> usize {
    let mut ids = Vec::with_capacity(256);
    // SAFETY: Toolhelp enumeration of this process's threads; each is suspended only while its
    // debug registers are written, and nothing is allocated meanwhile.
    unsafe {
        let (process, me) = (GetCurrentProcessId(), GetCurrentThreadId());
        if let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) {
            let mut entry = THREADENTRY32 { dwSize: size_of::<THREADENTRY32>() as u32, ..Default::default() };
            let mut more = Thread32First(snapshot, &mut entry).is_ok();
            while more {
                if entry.th32OwnerProcessID == process && entry.th32ThreadID != me {
                    ids.push(entry.th32ThreadID);
                }
                more = Thread32Next(snapshot, &mut entry).is_ok();
            }
            let _ = CloseHandle(snapshot);
        }
        let mut done = 0;
        for id in ids {
            let Ok(thread) = OpenThread(THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_SET_CONTEXT, false, id) else { continue };
            if SuspendThread(thread) != u32::MAX {
                let mut aligned = Aligned(CONTEXT { ContextFlags: crate::arch::DEBUG_REGISTERS, ..Default::default() });
                let context = &mut aligned.0;
                if GetThreadContext(thread, context).is_ok() {
                    crate::arch::set_breakpoints(context, addresses);
                    context.Dr6 = 0;
                    crate::arch::set_dr7(context, dr7 as usize);
                    if SetThreadContext(thread, context).is_ok() {
                        done += 1;
                    }
                }
                ResumeThread(thread);
            }
            let _ = CloseHandle(thread);
        }
        done
    }
}

/// Watches writes to up to four 4-byte-aligned addresses, recording up to `per_slot` writes of
/// each. Returns the number of threads armed.
pub fn arm(addresses: &[usize], per_slot: u32) -> Result<usize, String> {
    arm_with(addresses, per_slot, false)
}

/// [`arm`] for reads as well as writes (the code that uses a value, not only what sets it).
pub fn arm_accesses(addresses: &[usize], per_slot: u32) -> Result<usize, String> {
    arm_with(addresses, per_slot, true)
}

fn arm_with(addresses: &[usize], per_slot: u32, reads: bool) -> Result<usize, String> {
    if addresses.is_empty() || addresses.len() > SLOTS {
        return Err(format!("1 to {SLOTS} addresses, not {}", addresses.len()));
    }
    if let Some(a) = addresses.iter().find(|a| *a % 4 != 0) {
        return Err(format!("{a:#x} is not 4-byte aligned"));
    }
    STORE.count.store(0, Relaxed);
    SLOT_HITS.iter().for_each(|n| n.store(0, Relaxed));
    PER_SLOT.store(per_slot, Relaxed);
    if HANDLER.load(Acquire) == 0 {
        // SAFETY: the handler only touches the exception's own records and static atomics; it
        // stays valid while this DLL is loaded (a producer is never unloaded).
        let handle = unsafe { AddVectoredExceptionHandler(1, Some(vectored)) };
        if handle.is_null() {
            return Err("could not add the exception handler".into());
        }
        HANDLER.store(handle as usize, Release);
    }
    let mut slots = [0usize; SLOTS];
    slots[..addresses.len()].copy_from_slice(addresses);
    Ok(set_all(slots, if reads { dr7_with(addresses.len(), 0b1111) } else { dr7_for(addresses.len()) }))
}

/// Clears the debug registers of every thread and removes the handler.
pub fn disarm() -> usize {
    let threads = set_all([0; SLOTS], 0);
    let handle = HANDLER.swap(0, AcqRel);
    if handle != 0 {
        // SAFETY: removes the handler `arm` added.
        unsafe { RemoveVectoredExceptionHandler(handle as *const c_void) };
    }
    threads
}

/// The writes recorded so far.
pub fn hits() -> Vec<Hit> {
    let count = (STORE.count.load(Acquire) as usize).min(MAX_HITS);
    (0..count)
        .map(|i| Hit {
            slot: STORE.slots[i].load(Relaxed),
            rip: STORE.rips[i].load(Relaxed),
            thread: STORE.threads[i].load(Relaxed),
            stack: std::array::from_fn(|k| STORE.stacks[i][k].load(Relaxed)),
        })
        .collect()
}

#[cfg(test)]
mod tests;
