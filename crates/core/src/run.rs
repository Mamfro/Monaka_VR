//! What lives for one run of a producer: from one start to the next. A detour has no context to be
//! handed its state, so it reads statics; these statics forget what an earlier run left, so a stop
//! and a start in the same copy of a DLL begin clean without a reset written anywhere.
//!
//! - [`RunCell`]: a value set once in a run and read from anywhere (build facts, a setup).
//! - [`RunState`]: state changed as the run goes, made fresh for each run on first use.
//! - [`RunCount`]: a counter that starts again at zero with each run.
//!
//! The framework calls [`begin`] at the top of each start. What a run set stays readable through
//! its own stop (its give-back and release still see it) until the next run begins.

use std::sync::atomic::{AtomicU64, Ordering::*};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};

/// The run in progress: 0 before the first start, then one more at each start.
static RUN: AtomicU64 = AtomicU64::new(0);

/// Begins a new run: every cell, state and count reads as fresh from here.
pub fn begin() {
    RUN.fetch_add(1, AcqRel);
}

/// The run in progress.
pub fn current() -> u64 {
    RUN.load(Acquire)
}

/// A value set once in a run and read from anywhere. One set in an earlier run reads as unset.
pub struct RunCell<T: ?Sized> {
    slot: RwLock<Option<(u64, Arc<T>)>>,
}

impl<T: ?Sized> RunCell<T> {
    pub const fn new() -> Self {
        RunCell { slot: RwLock::new(None) }
    }

    /// Sets this run's value, in place of any earlier one.
    pub fn set(&self, value: T)
    where
        T: Sized,
    {
        self.set_arc(Arc::new(value));
    }

    /// [`RunCell::set`] for a value already shared (or unsized: `Arc<dyn Any + Send + Sync>`).
    pub fn set_arc(&self, value: Arc<T>) {
        self.put(current(), value);
    }

    /// This run's value; none when it is unset in this run.
    pub fn get(&self) -> Option<Arc<T>> {
        self.get_in(current())
    }

    fn put(&self, run: u64, value: Arc<T>) {
        *self.slot.write().unwrap_or_else(|e| e.into_inner()) = Some((run, value));
    }

    fn get_in(&self, run: u64) -> Option<Arc<T>> {
        let slot = self.slot.read().unwrap_or_else(|e| e.into_inner());
        slot.as_ref().filter(|(set_in, _)| *set_in == run).map(|(_, value)| Arc::clone(value))
    }
}

impl<T: ?Sized> Default for RunCell<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// State changed as a run goes, behind one lock, made by `fresh` on its first use in each run.
/// Several values a detour reads together belong in one state, so they are read in step.
pub struct RunState<T> {
    slot: Mutex<Option<(u64, T)>>,
    fresh: fn() -> T,
}

impl<T> RunState<T> {
    /// A state made by `fresh` for each run (`RunState::new(State::new)`).
    pub const fn new(fresh: fn() -> T) -> Self {
        RunState { slot: Mutex::new(None), fresh }
    }

    /// This run's state, locked (a lock whose holder panicked is taken as it is:
    /// [`crate::sync::Locked`]).
    pub fn lock(&self) -> RunGuard<'_, T> {
        self.lock_in(current())
    }

    /// Runs `work` on this run's state.
    pub fn with<R>(&self, work: impl FnOnce(&mut T) -> R) -> R {
        work(&mut self.lock())
    }

    fn lock_in(&self, run: u64) -> RunGuard<'_, T> {
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        if !matches!(&*slot, Some((made_in, _)) if *made_in == run) {
            *slot = Some((run, (self.fresh)()));
        }
        RunGuard { slot }
    }
}

/// A [`RunState`] locked for this run.
pub struct RunGuard<'a, T> {
    slot: MutexGuard<'a, Option<(u64, T)>>,
}

impl<T> std::ops::Deref for RunGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        match &*self.slot {
            Some((_, state)) => state,
            None => unreachable!("a RunGuard is made only over a state"),
        }
    }
}

impl<T> std::ops::DerefMut for RunGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        match &mut *self.slot {
            Some((_, state)) => state,
            None => unreachable!("a RunGuard is made only over a state"),
        }
    }
}

/// A counter that starts again at zero with each run, without a lock. A count added by a thread
/// still in the last run as the next one begins may land in either.
pub struct RunCount {
    run: AtomicU64,
    count: AtomicU64,
}

impl RunCount {
    pub const fn new() -> Self {
        RunCount { run: AtomicU64::new(0), count: AtomicU64::new(0) }
    }

    /// Adds `n` to this run's count.
    pub fn add(&self, n: u64) {
        self.add_in(current(), n);
    }

    /// Adds one; this run's count after it.
    pub fn bump(&self) -> u64 {
        self.add(1);
        self.get()
    }

    /// This run's count.
    pub fn get(&self) -> u64 {
        self.get_in(current())
    }

    fn add_in(&self, run: u64, n: u64) {
        if self.run.load(Acquire) != run && self.run.swap(run, AcqRel) != run {
            self.count.store(0, Release);
        }
        self.count.fetch_add(n, AcqRel);
    }

    fn get_in(&self, run: u64) -> u64 {
        if self.run.load(Acquire) == run { self.count.load(Acquire) } else { 0 }
    }
}

impl Default for RunCount {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
