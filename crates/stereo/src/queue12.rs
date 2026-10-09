//! The game's direct queue, for a D3D12 game whose engine keeps it out of reach. D3D12's
//! `ExecuteCommandLists` is hooked, found on a throwaway queue of the process's device (hooked in
//! D3D12's code, which every queue of the process shares, or in the queue class's vtable), and
//! each direct queue it sees is noted with the thread that last executed work on it. At a present,
//! the queue the presenting thread used last is the game's ([`game_queue`]): an engine finishes a
//! frame's work on the swapchain's queue from the thread that then presents it.

use monaka_hook::{Hooks, InFlight, Original, thread_id};
use monaka_producer::{Rejection, log, log_first};
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering::*};
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
use windows::Win32::Graphics::Direct3D12::{D3D12_COMMAND_LIST_TYPE_DIRECT, D3D12_COMMAND_QUEUE_DESC, D3D12CreateDevice, ID3D12CommandQueue, ID3D12Device};
use windows::core::Interface;

/// `ID3D12CommandQueue::ExecuteCommandLists`.
const SLOT_EXECUTE: usize = 10;
/// `ExecuteCommandLists(this, count, lists)`.
pub type ExecuteFn = unsafe extern "system" fn(*mut c_void, u32, *const *mut c_void);
static EXECUTE: Original<ExecuteFn> = Original::new();

/// The direct queues seen (raw pointers; the game's own live as long as it does) and the thread
/// that last executed work on each.
const SEEN: usize = 8;
static QUEUES: [AtomicUsize; SEEN] = [const { AtomicUsize::new(0) }; SEEN];
static THREADS: [AtomicU32; SEEN] = [const { AtomicU32::new(0) }; SEEN];
/// Queues seen that are not direct (compute, copy), so their type is asked once.
static OTHERS: [AtomicUsize; SEEN] = [const { AtomicUsize::new(0) }; SEEN];

/// Prepares the hook. Makes a queue on the device of the default adapter (D3D12 hands back the
/// game's own device when it has one there) and lets it go once hooked.
///
/// # Safety
/// Called at start, before the hooks are enabled.
pub unsafe fn hook(hooks: &mut Hooks) -> Result<(), Rejection> {
    let failed = |what: &str, e: windows::core::Error| Rejection::unsupported(format!("queue catcher: {what}: {e}"));
    let mut device: Option<ID3D12Device> = None;
    // SAFETY: plain D3D12 creation calls; the queue is released when this returns, the hook targets
    // D3D12's ExecuteCommandLists, whose type is `ExecuteFn`.
    unsafe {
        D3D12CreateDevice(None, D3D_FEATURE_LEVEL_11_0, &mut device).map_err(|e| failed("device", e))?;
        let device = device.ok_or_else(|| Rejection::unsupported("queue catcher: no device"))?;
        let queue: ID3D12CommandQueue = device.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC { Type: D3D12_COMMAND_LIST_TYPE_DIRECT, ..Default::default() }).map_err(|e| failed("queue", e))?;
        hooks.method_noted(&EXECUTE, "ExecuteCommandLists", queue.as_raw(), SLOT_EXECUTE, execute as ExecuteFn, |why| {
            log!("queue catcher: ExecuteCommandLists: {why}; hooking its vtable slot instead");
        })?;
    }
    Ok(())
}

unsafe extern "system" fn execute(queue: *mut c_void, count: u32, lists: *const *mut c_void) {
    let _flight = InFlight::enter();
    note(queue as usize);
    // SAFETY: the original, with the game's own arguments.
    unsafe { EXECUTE.get()(queue, count, lists) }
}

/// Notes that the current thread executed work on `queue`.
fn note(queue: usize) {
    let thread = thread_id();
    if let Some(i) = QUEUES.iter().position(|q| q.load(Relaxed) == queue) {
        THREADS[i].store(thread, Relaxed);
        return;
    }
    if queue == 0 || OTHERS.iter().any(|q| q.load(Relaxed) == queue) {
        return;
    }
    let raw = queue as *mut c_void;
    // SAFETY: the queue ExecuteCommandLists was called on, alive for the call; only its descriptor
    // is read.
    let kind = unsafe { ID3D12CommandQueue::from_raw_borrowed(&raw).map(|q| q.GetDesc().Type) };
    let table = if kind == Some(D3D12_COMMAND_LIST_TYPE_DIRECT) { &QUEUES } else { &OTHERS };
    for (i, cell) in table.iter().enumerate() {
        if cell.compare_exchange(0, queue, Relaxed, Relaxed).is_ok() {
            if kind == Some(D3D12_COMMAND_LIST_TYPE_DIRECT) {
                THREADS[i].store(thread, Relaxed);
                log!("queue catcher: direct queue {queue:#x} on thread {thread}");
            }
            return;
        }
    }
    log_first!(1, "queue catcher: more than {SEEN} queues; {queue:#x} is not noted");
}

/// The game's direct queue, asked on the thread that presents: the one that thread executed work
/// on last, else the only direct queue seen.
pub fn game_queue() -> Option<ID3D12CommandQueue> {
    let thread = thread_id();
    let queues: Vec<(usize, u32)> = QUEUES.iter().zip(&THREADS).map(|(q, t)| (q.load(Relaxed), t.load(Relaxed))).filter(|(q, _)| *q != 0).collect();
    let queue = match queues.iter().find(|(_, t)| *t == thread) {
        Some(&(q, _)) => q,
        None if queues.len() == 1 => queues[0].0,
        None => {
            log_first!(3, "queue catcher: presenting thread {thread} executed on none of {queues:x?}");
            return None;
        }
    };
    let raw = queue as *mut c_void;
    // SAFETY: a direct queue the game executed work on; it lives as long as the game's renderer.
    unsafe { ID3D12CommandQueue::from_raw_borrowed(&raw) }.cloned()
}
