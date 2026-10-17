//! The game's direct queue, for a D3D12 game whose engine keeps it out of reach. D3D12's
//! `ExecuteCommandLists` is hooked, found on a throwaway queue of the process's device (hooked in
//! D3D12's code, which every queue of the process shares, or in the queue class's vtable), and
//! each direct queue it sees is noted with the thread that last executed work on it. At a present,
//! the game's queue ([`game_queue`]) is the one the swapchain itself holds
//! ([`crate::swapchain_queue`]); only where that cannot be read is it the queue the presenting
//! thread used, which cannot tell apart two queues one thread works on.

use monaka_hook::{Hooks, InFlight, Original, thread_id};
use monaka_producer::{Rejection, log, log_first};
use monaka_core::sync::Locked;
use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering::*};
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
use windows::Win32::Graphics::Direct3D12::{D3D12_COMMAND_LIST_TYPE_DIRECT, D3D12_COMMAND_QUEUE_DESC, D3D12CreateDevice, ID3D12CommandQueue, ID3D12Device};
use windows::Win32::Graphics::Dxgi::IDXGISwapChain3;
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
/// When each was last executed on (a running count), so of two queues one thread works on the
/// later is known.
static LAST: [AtomicU64; SEEN] = [const { AtomicU64::new(0) }; SEEN];
static COUNT: AtomicU64 = AtomicU64::new(0);
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
    // Not fatal: without it the presenting thread's queue is used.
    let _ = crate::swapchain_queue::learn();
    Ok(())
}

unsafe extern "system" fn execute(queue: *mut c_void, count: u32, lists: *const *mut c_void) {
    let flight = InFlight::enter();
    note(queue as usize);
    // SAFETY: the original, with the game's own arguments.
    unsafe { EXECUTE.get(&flight)(queue, count, lists) }
}

/// Notes that the current thread executed work on `queue`.
fn note(queue: usize) {
    let thread = thread_id();
    if let Some(i) = QUEUES.iter().position(|q| q.load(Relaxed) == queue) {
        THREADS[i].store(thread, Relaxed);
        LAST[i].store(COUNT.fetch_add(1, Relaxed), Relaxed);
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
                LAST[i].store(COUNT.fetch_add(1, Relaxed), Relaxed);
                log!("queue catcher: direct queue {queue:#x} on thread {thread}");
            }
            return;
        }
    }
    log_first(1, format_args!("queue catcher: more than {SEEN} queues; {queue:#x} is not noted"));
}

/// A game's own way to its direct queue (a present loop's `queue`).
pub type OwnQueue = fn() -> Option<ID3D12CommandQueue>;

/// The game's own way to its direct queue, set by its present loop ([`use_own`]).
static OWN: Mutex<Option<OwnQueue>> = Mutex::new(None);

/// Sets the game's own way to its direct queue (a present loop's `queue`): where the game executes
/// through a queue its swapchain does not hold (Streamline's proxy), only it answers [`game_queue`].
pub fn use_own(own: Option<OwnQueue>) {
    *OWN.locked() = own;
}

/// The game's direct queue, the one place every feature asks: the game's own way where it has one
/// ([`use_own`]); else, asked on the thread that presents `swap`, the queue the swapchain presents
/// on; where that cannot be read, the direct queue that thread executed work on last, else the
/// only direct queue seen.
pub fn game_queue(swap: Option<&IDXGISwapChain3>) -> Option<ID3D12CommandQueue> {
    if let Some(own) = *OWN.locked() {
        return own();
    }
    // SAFETY: the swapchain being presented, alive for the call.
    if let Some(queue) = swap.and_then(|swap| unsafe { crate::swapchain_queue::of(swap.as_raw()) }) {
        log_first(1, format_args!("queue catcher: the swapchain presents on {:#x}", queue.as_raw() as usize));
        return Some(queue);
    }
    log_first(1, format_args!("queue catcher: the swapchain's queue could not be read; using the presenting thread's"));
    let thread = thread_id();
    let queues: Vec<(usize, u32, u64)> =
        (0..SEEN).map(|i| (QUEUES[i].load(Relaxed), THREADS[i].load(Relaxed), LAST[i].load(Relaxed))).filter(|(q, ..)| *q != 0).collect();
    let queue = match queues.iter().filter(|(_, t, _)| *t == thread).max_by_key(|(.., last)| *last) {
        Some(&(q, ..)) => q,
        None if queues.len() == 1 => queues[0].0,
        None => {
            log_first(3, format_args!("queue catcher: presenting thread {thread} executed on none of {queues:x?}"));
            return None;
        }
    };
    let raw = queue as *mut c_void;
    // SAFETY: a direct queue the game executed work on; it lives as long as the game's renderer.
    unsafe { ID3D12CommandQueue::from_raw_borrowed(&raw) }.cloned()
}
