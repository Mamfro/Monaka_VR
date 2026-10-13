//! The session log: [`LogFile`] behind [`crate::log()`] and `log!`, the detailed-logging switch,
//! and helpers for the lines.
//!
//! A line never waits on the disk: whoever logs formats it into a buffer in memory, and a writer
//! thread writes the buffer to the file every few milliseconds. A crash or a panic writes what is
//! pending at once, then its own line ([`LogFile::urgent`]), before the game goes down. Lines from
//! the last few milliseconds before the game exits on its own can be lost.

use monaka_core::scrub::Scrub;
use monaka_core::sync::Locked;
use std::fmt;
use std::fs::File;
use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, TryLockError};
use std::time::{Duration, Instant};

/// How often the writer thread writes the pending lines.
pub const WRITE_EVERY: Duration = Duration::from_millis(10);

/// How long a crash line waits for the log's locks before it gives up on its line.
const URGENT_WAIT: Duration = Duration::from_millis(200);

/// One log file: the lines not yet written, and the file with a spare buffer to swap them into.
/// Locks are always taken file first, then pending.
pub struct LogFile {
    pending: Mutex<Vec<u8>>,
    out: Mutex<Out>,
    started: Instant,
}

struct Out {
    file: File,
    spare: Vec<u8>,
    /// The lines as written: with the player's folder names taken out ([`Scrub`]), here rather
    /// than as each line is logged, so it costs the game's threads nothing.
    scrubbed: Vec<u8>,
    scrub: Scrub,
    /// Bytes written so far.
    written: u64,
}

impl LogFile {
    pub fn create(path: &Path, scrub: Scrub) -> io::Result<Self> {
        let out = Out { file: File::create(path)?, spare: Vec::with_capacity(64 * 1024), scrubbed: Vec::with_capacity(64 * 1024), scrub, written: 0 };
        Ok(Self { pending: Mutex::new(Vec::with_capacity(64 * 1024)), out: Mutex::new(out), started: Instant::now() })
    }

    /// The bytes on the disk so far.
    pub fn written(&self) -> u64 {
        self.out.locked().written
    }

    /// Adds one line, stamped with the seconds since the log opened.
    pub fn line(&self, args: fmt::Arguments<'_>) {
        self.append(&mut self.pending.locked(), args);
    }

    fn append(&self, pending: &mut Vec<u8>, args: fmt::Arguments<'_>) {
        let _ = writeln!(pending, "{:>9.3} {args}", self.started.elapsed().as_secs_f64());
    }

    /// Writes the pending lines to the file (the writer thread's work).
    pub fn write_pending(&self) {
        let mut out = self.out.locked();
        Self::write_out(&mut out, self.pending.locked());
    }

    fn write_out(out: &mut Out, mut pending: MutexGuard<'_, Vec<u8>>) {
        std::mem::swap(&mut *pending, &mut out.spare);
        drop(pending);
        if !out.spare.is_empty() {
            let Out { file, spare, scrubbed, scrub, written } = out;
            scrub.bytes(spare, scrubbed);
            let _ = file.write_all(scrubbed);
            let _ = file.flush();
            *written += scrubbed.len() as u64;
            spare.clear();
            scrubbed.clear();
        }
    }

    /// A line the game may not outlive (a fault, a panic): the pending lines, then this one, are
    /// on the disk when this returns. On a thread that faulted while holding the log's lock,
    /// waiting would hang the game, so the line is dropped after a short wait instead.
    pub fn urgent(&self, args: fmt::Arguments<'_>) {
        let Some(mut out) = lock_within(&self.out, URGENT_WAIT) else { return };
        let Some(mut pending) = lock_within(&self.pending, URGENT_WAIT) else { return };
        self.append(&mut pending, args);
        Self::write_out(&mut out, pending);
    }
}

fn lock_within<T>(mutex: &Mutex<T>, wait: Duration) -> Option<MutexGuard<'_, T>> {
    let started = Instant::now();
    loop {
        match mutex.try_lock() {
            Ok(guard) => return Some(guard),
            Err(TryLockError::Poisoned(poisoned)) => return Some(poisoned.into_inner()),
            Err(TryLockError::WouldBlock) if started.elapsed() < wait => std::thread::yield_now(),
            Err(TryLockError::WouldBlock) => return None,
        }
    }
}

/// Whether detailed lines are wanted (the player's "Diagnostic logging", [`LogFile::follow`]).
/// Lines worth having only when someone is looking into a problem (every frame, every draw) go
/// behind it: `if log::detailed() { log!(..) }`, which costs one load when it is off.
static DETAILED: AtomicBool = AtomicBool::new(false);

pub fn detailed() -> bool {
    DETAILED.load(Ordering::Relaxed)
}

/// How big a log may grow before detailed lines stop for the rest of the run.
pub const DETAILED_CAP: u64 = 64 * 1024 * 1024;

impl LogFile {
    /// Detailed lines on while `wanted` (the player's setting), until this log reaches
    /// [`DETAILED_CAP`]; each change said in the log.
    pub fn follow(&self, wanted: bool) {
        if let Some((on, why)) = switched(detailed(), wanted, self.written() >= DETAILED_CAP) {
            DETAILED.store(on, Ordering::Relaxed);
            self.line(format_args!("diagnostic logging {why}"));
        }
    }
}

/// Detailed lines' new state and how to say it, when it changes: on while `wanted` and the log is
/// not `full`.
fn switched(was: bool, wanted: bool, full: bool) -> Option<(bool, &'static str)> {
    let on = wanted && !full;
    (on != was).then_some((on, if on { "on" } else if wanted { "stopped: the log reached its size limit" } else { "off" }))
}

/// Whether the `n`th time something happens (counted from 1) is worth a log line: the first
/// `first` times, then at each power of two, so a site that keeps firing stays a handful of lines.
pub fn seldom(n: u64, first: u64) -> bool {
    n <= first || n.is_power_of_two()
}

#[cfg(test)]
mod tests;
