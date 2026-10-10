//! The reading end of a stereo channel on either transport, as the viewer and the probe use it:
//! what the producer publishes, learned before anything is opened ([`describe_any`], [`wait_for`]),
//! and one reader over whichever transport it is ([`AnyReader`]). A channel is one or the other:
//! D3D12 producers publish over [`crate::fence`], D3D11 ones over [`crate::keyed`].

use crate::fence::{self, FenceReader};
use crate::keyed::{self, KeyedReader, Pair};
use crate::{ChannelName, Result};
use monaka_core::protocol::{Luid, dxgi_format};
use std::time::{Duration, Instant};
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D};

/// What a channel's producer publishes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Description {
    pub width: u32,
    pub height: u32,
    /// The GPU the producer renders on: a reader opens the channel on a device there.
    pub adapter: Luid,
    /// The eye images' `DXGI_FORMAT` (a keyed channel's are always RGBA8).
    pub format: u32,
    /// The fence transport (else keyed).
    pub fence: bool,
}

/// The channel's producer, if one has made its channel: a fence channel first, then a keyed one.
pub fn describe_any(name: &ChannelName) -> Option<Description> {
    if let Some(d) = fence::describe(name) {
        return Some(Description { width: d.width, height: d.height, adapter: d.adapter, format: d.format, fence: true });
    }
    keyed::describe(name).map(|d| Description { width: d.width, height: d.height, adapter: d.adapter, format: dxgi_format::R8G8B8A8_UNORM, fence: false })
}

/// [`describe_any`], asked every 20 ms for up to `timeout`: a producer makes its channel with its
/// first frame, and may first switch the game to the headset's resolution.
pub fn wait_for(name: &ChannelName, timeout: Duration) -> Option<Description> {
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

/// A reader of either transport.
pub enum AnyReader {
    Keyed(KeyedReader),
    Fence(FenceReader),
}

impl AnyReader {
    /// Opens the channel on `device` (on the producer's adapter) with the reader of its transport
    /// ([`Description::fence`]). Only one reader may hold a channel.
    pub fn open(name: &ChannelName, device: &ID3D11Device, fence: bool) -> Result<Self> {
        Ok(if fence { AnyReader::Fence(FenceReader::open(name, device)?) } else { AnyReader::Keyed(KeyedReader::open(name, device)?) })
    }

    /// A fence reader's empty reads so far: the header locked by the producer, and no finished
    /// pair newer than the one shown (for diagnostics).
    pub fn misses(&self) -> Option<(u64, u64)> {
        match self {
            AnyReader::Fence(r) => Some((r.busy, r.idle)),
            AnyReader::Keyed(_) => None,
        }
    }

    /// Copies the newest pair into `eyes` on `context` if it is newer than `seen`. `Ok(None)`:
    /// nothing new yet.
    pub fn read(&mut self, context: &ID3D11DeviceContext, eyes: [&ID3D11Texture2D; 2], seen: u64) -> Result<Option<Pair>> {
        match self {
            AnyReader::Keyed(r) => r.read(context, eyes, seen),
            AnyReader::Fence(r) => r.read(context, eyes, seen),
        }
    }
}
