//! The reading end of a stereo channel as the viewer and the probe use it: what the producer
//! publishes, learned before anything is opened ([`describe_any`], [`wait_for`]). Every producer,
//! D3D12 or D3D11, publishes over [`crate::fence`], read with [`FenceReader`].

pub use crate::fence::{Description, FenceReader};
use crate::shared::SharedBlock;
use crate::{ChannelName, fence};
use monaka_contract::protocol::{FENCE_MAGIC, FENCE_MAJOR, Mismatch, Stamp};
use std::time::{Duration, Instant};

/// The channel's producer, if one has made its channel. `Err`: a producer is there, but its
/// channel is another version of the protocol.
pub fn describe_any(name: &ChannelName) -> Option<std::result::Result<Description, Mismatch>> {
    if let Some(d) = fence::describe(name) {
        return Some(Ok(d));
    }
    Mismatch::of(header_stamp(name)?, "stereo channel", FENCE_MAGIC, FENCE_MAJOR).map(Err)
}

/// The stamp at the start of the channel's header, whatever its layout's size.
fn header_stamp(name: &ChannelName) -> Option<Stamp> {
    let mut block = SharedBlock::<Stamp>::open(&name.header(), &name.header_lock(), false).ok()?;
    block.lock(0).tolerant().map(|stamp| *stamp)
}

/// [`describe_any`], asked every 20 ms for up to `timeout`: a producer makes its channel with its
/// first frame, and may first switch the game to the headset's resolution. A channel of another
/// protocol version ends the wait at once.
pub fn wait_for(name: &ChannelName, timeout: Duration) -> Option<std::result::Result<Description, Mismatch>> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(d) = describe_any(name) {
            return Some(d);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
