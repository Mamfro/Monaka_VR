//! Locks inside a game: a mutex whose holder panicked is still taken (code running in a game never
//! unwinds into it, so what the holder left is the last good state, and refusing it would only
//! stop the work for the rest of the run).

use std::sync::{Mutex, MutexGuard};

/// [`Mutex::lock`] that takes a poisoned lock as it is.
pub trait Locked<T> {
    fn locked(&self) -> MutexGuard<'_, T>;
}

impl<T> Locked<T> for Mutex<T> {
    fn locked(&self) -> MutexGuard<'_, T> {
        self.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests;
