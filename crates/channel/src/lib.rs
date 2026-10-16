//! Windows transport for the channels between game producers and the viewer.
//!
//! This is an edge crate: the Win32 and Direct3D calls live here, behind small safe types. The
//! record layouts come from `monaka_contract`, the transport rules from `monaka_core`.
//!
//! - [`shared`]: named shared memory guarded by a named mutex.
//! - [`pose`]: head pose (viewer to producer) and rendered-pair poses (producer to viewer).
//! - [`hands`]: where the controllers point (viewer to producer).
//! - [`fence`]: stereo pairs from a D3D12 producer to a D3D11 reader through shared fences;
//!   [`fence11`] writes the same channel from a D3D11 producer.
//! - [`reader`]: what a channel's producer publishes, for the viewer and the probe.
//! - [`d3d`], [`d3d12`]: small Direct3D helpers the transports and the producers share.

pub mod console;
pub mod d3d;
pub mod debug;
pub mod debug_image;
pub mod d3d12;
pub mod fence;
pub mod fence11;
pub mod fingers;
pub mod hands;

pub mod page;
pub mod panels;
pub mod pose;
pub mod priority;
pub mod reader;
pub mod reticles;
pub mod seqlock;
pub mod shared;

pub use monaka_contract::protocol::ChannelName;

use std::fmt;

#[derive(Debug)]
pub enum Error {
    /// A Windows or Direct3D call failed.
    Os(windows::core::Error),
    /// A named object this side must create already exists (a stale or competing producer).
    AlreadyExists(String),
    /// Another reader holds the channel.
    ReaderTaken,
    /// The other side's records have a different magic, version or shape.
    Incompatible(&'static str),
    /// The other side's record is the same kind at another protocol version: the game pack and
    /// this Monaka were built apart.
    Version(monaka_contract::protocol::Mismatch),
    /// The header mutex could not be taken without waiting.
    Busy,
    /// A previous owner died holding a lock or key; the channel cannot be trusted.
    Unhealthy,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Os(e) => write!(f, "{e}"),
            Error::AlreadyExists(name) => write!(f, "{name} already exists"),
            Error::ReaderTaken => f.write_str("another reader holds this channel"),
            Error::Incompatible(what) => write!(f, "incompatible channel: {what}"),
            Error::Version(mismatch) => write!(f, "{mismatch}"),
            Error::Busy => f.write_str("channel lock busy"),
            Error::Unhealthy => f.write_str("channel abandoned by a crashed owner"),
        }
    }
}

impl std::error::Error for Error {}

impl From<windows::core::Error> for Error {
    fn from(e: windows::core::Error) -> Self {
        Error::Os(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// The error for a block of kind `what` (`magic` at major `major` expected) found with `stamp`: a
/// version mismatch when it is that kind of block, else an incompatible one.
pub(crate) fn incompatible_block(what: &'static str, stamp: monaka_contract::protocol::Stamp, magic: u32, major: u16) -> Error {
    monaka_contract::protocol::Mismatch::of(stamp, what, magic, major).map_or(Error::Incompatible(what), Error::Version)
}

/// Milliseconds since boot, the clock the channels stamp pairs with.
pub fn tick() -> u64 {
    // SAFETY: no preconditions.
    unsafe { windows::Win32::System::SystemInformation::GetTickCount64() }
}
