//! In-process hooking for the producers that run inside a game.
//!
//! This is an edge crate: every Win32 call and every write to game code lives here, behind small
//! types with `SAFETY` notes. What the producers build on:
//!
//! - [`mem`]: reading and writing the game's memory without trusting it (a bad address is a failed
//!   call, never a crash).
//! - [`module`]: loaded modules, exports and fingerprints.
//! - [`patch`]: inline hooks on signature-checked prologues, vtable hooks, and [`Original`], the
//!   typed pointer a detour calls through.
//! - [`probe`]: class names from RTTI, fields holding a known value, and a detour's call site.
//! - [`watch`]: hardware write watches that record which code writes an address.
//! - [`coverage`]: which of a module's functions run (a breakpoint on each, taken out when hit):
//!   what runs only while the player does something, and the functions handed a value (64-bit only).
//! - [`live`]: probes added while the game runs, at any address (64-bit only): each call's
//!   thread, frame, arguments and caller, and the fields an action changes.
//! - [`InFlight`]: counts detours in progress so a producer can stop only when none is running,
//!   and names the ones still running when it cannot.
//!
//! It builds for 64-bit games and for 32-bit ones (`i686-pc-windows-msvc`): what differs between
//! the two (jump encodings, the decoder's prefixes, register names) is in `arch/x64.rs` and
//! `arch/x86.rs`.

#[cfg_attr(target_arch = "x86_64", path = "arch/x64.rs")]
#[cfg_attr(target_arch = "x86", path = "arch/x86.rs")]
mod arch;
#[cfg(target_arch = "x86_64")]
pub mod coverage;
pub mod crash;
pub mod decode;
mod in_flight;
#[cfg(target_arch = "x86_64")]
pub mod live;
pub mod mem;
pub mod module;
pub mod patch;
pub mod probe;
mod threads;
pub mod watch;

pub use in_flight::InFlight;
pub use patch::{Hooks, Instruction, Original, Placement, Site, slot};

/// A float as an atomic (its bits in an `AtomicU32`), for values detours share across threads.
pub struct AtomicF32(std::sync::atomic::AtomicU32);

impl AtomicF32 {
    pub const fn new(value: f32) -> Self {
        Self(std::sync::atomic::AtomicU32::new(value.to_bits()))
    }

    pub const fn zero() -> Self {
        Self::new(0.0)
    }

    pub fn load(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Acquire))
    }

    pub fn store(&self, value: f32) {
        self.0.store(value.to_bits(), Ordering::Release);
    }
}

/// The calling thread's id, for logs.
pub fn thread_id() -> u32 {
    // SAFETY: plain query.
    unsafe { windows::Win32::System::Threading::GetCurrentThreadId() }
}

use std::fmt;
use std::sync::atomic::Ordering;

#[derive(Debug)]
pub enum Error {
    /// The bytes at a hook target are not the expected prologue (a different game build).
    Signature(String),
    /// No memory could be reserved within reach of a hook target.
    NoNearMemory(String),
    /// Another thread kept executing inside the bytes to be patched.
    Busy(String),
    /// A Windows call failed.
    Os(String, windows::core::Error),
    /// Anything else that makes a hook unsafe to install.
    Refused(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Signature(name) => write!(f, "{name}: unexpected code at the hook target"),
            Error::NoNearMemory(name) => write!(f, "{name}: no free memory near the hook target"),
            Error::Busy(name) => write!(f, "{name}: a thread stayed inside the patched bytes"),
            Error::Os(what, e) => write!(f, "{what}: {e}"),
            Error::Refused(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;
