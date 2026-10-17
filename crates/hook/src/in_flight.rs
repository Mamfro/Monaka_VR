//! [`InFlight`]: counts detours in progress so a producer stops only when none is running, and
//! names the ones still running when it cannot (each by where its detour took the guard).

use std::panic::Location;
use std::ptr;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicPtr, AtomicU32, AtomicU64, AtomicUsize, Ordering::*};
use std::time::Instant;

static COUNT: AtomicUsize = AtomicUsize::new(0);

/// Detours named at once; more still count, unnamed.
const SLOTS: usize = 64;
const NO_SLOT: usize = usize::MAX;

/// One detour running: where it took its guard, on which thread, since when (ms from [`base`]).
struct Slot {
    at: AtomicPtr<Location<'static>>,
    thread: AtomicU32,
    since_ms: AtomicU64,
}

static TABLE: [Slot; SLOTS] = [const { Slot { at: AtomicPtr::new(ptr::null_mut()), thread: AtomicU32::new(0), since_ms: AtomicU64::new(0) } }; SLOTS];

fn base() -> Instant {
    static BASE: OnceLock<Instant> = OnceLock::new();
    *BASE.get_or_init(Instant::now)
}

fn now_ms() -> u64 {
    base().elapsed().as_millis() as u64
}

/// Held by every detour for its whole run, so a producer stops only when no detour is running.
///
/// A detour that does nothing after its forward may let go just before it, when the forward can
/// run for seconds (a game's whole frame, which holds a load): trampolines and producers are never
/// freed, so the forward and the return into the detour stay valid after a stop.
pub struct InFlight {
    slot: usize,
}

impl InFlight {
    /// The guard for the calling detour, named by where it is taken.
    #[track_caller]
    pub fn enter() -> Self {
        COUNT.fetch_add(1, AcqRel);
        let at = ptr::from_ref(Location::caller()).cast_mut();
        let thread = crate::thread_id();
        let first = thread as usize % SLOTS;
        for k in 0..SLOTS {
            let slot = (first + k) % SLOTS;
            if TABLE[slot].at.compare_exchange(ptr::null_mut(), at, AcqRel, Relaxed).is_ok() {
                TABLE[slot].thread.store(thread, Release);
                TABLE[slot].since_ms.store(now_ms(), Release);
                return Self { slot };
            }
        }
        Self { slot: NO_SLOT }
    }

    /// Waits until no detour is running; false after `timeout_ms`.
    pub fn wait_idle(timeout_ms: u64) -> bool {
        let start = Instant::now();
        while COUNT.load(Acquire) != 0 {
            if start.elapsed().as_millis() as u64 >= timeout_ms {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        true
    }

    /// The detours running now, for a log: each where it took its guard, its thread and for how
    /// long (`none` when none is).
    pub fn running() -> String {
        let now = now_ms();
        let mut named = Vec::new();
        for slot in &TABLE {
            let at = slot.at.load(Acquire);
            if at.is_null() {
                continue;
            }
            // SAFETY: only `&'static Location`s are stored.
            let at = unsafe { &*at };
            let (thread, since) = (slot.thread.load(Acquire), slot.since_ms.load(Acquire));
            named.push(format!("{}:{} on thread {thread} for {} ms", short_path(at.file()), at.line(), now.saturating_sub(since)));
        }
        let unnamed = COUNT.load(Acquire).saturating_sub(named.len());
        if unnamed > 0 {
            named.push(format!("{unnamed} more unnamed"));
        }
        if named.is_empty() { "none".to_owned() } else { named.join("; ") }
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        if self.slot != NO_SLOT {
            TABLE[self.slot].at.store(ptr::null_mut(), Release);
        }
        COUNT.fetch_sub(1, AcqRel);
    }
}

/// A source path from its crate's folder on (`wolf_tno/src/providers/focus.rs`): no folders above
/// the crate, which would name the machine's user.
pub fn short_path(file: &str) -> String {
    let parts: Vec<&str> = file.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
    let from = parts.iter().rposition(|&p| p == "src").map_or(0, |src| src.saturating_sub(1));
    parts[from..].join("/")
}

#[cfg(test)]
mod tests;
