//! Same-frame pairs over the fence channel from D3D11: a game that renders both eyes on one
//! frame copies each eye out of its back buffer as that eye's present comes ([`PairPublisher11::copy_eye`])
//! and publishes the pair at the second ([`PairPublisher11::publish_pair`]), the pose record
//! first; a frame with no pair (a menu, a loading screen) goes out flat
//! ([`PairPublisher11::publish_flat`]). The channel is made at the first frame's size, once the
//! game says the size is right (`ready`: a producer waiting for the headset's eye size).

pub use crate::alternate::Step;
use crate::publish::{FLAT, Tally, copy_into, record_first};
use monaka_channel::fence11::{FenceProducer11, LazyProducer11};
use monaka_channel::pose::HeadSource;
use monaka_channel::{ChannelName, d3d};
use monaka_contract::protocol::HeadPose;
use monaka_producer::log;
use windows::Win32::Graphics::Direct3D11::{D3D11_TEXTURE2D_DESC, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D};

pub struct PairPublisher11 {
    writer: LazyProducer11,
    /// Private copies of the two eyes, at the back buffer's shape.
    eyes: [Option<ID3D11Texture2D>; 2],
    /// The pair each copy belongs to (the game's own numbering), so a right eye is published
    /// only beside its own left eye.
    pair: [u64; 2],
    /// Publishing stopped for good (the channel failed).
    broken: bool,
    tally: Tally,
}

impl PairPublisher11 {
    pub fn new(channel: ChannelName) -> Self {
        Self { writer: LazyProducer11::new(channel), eyes: [None, None], pair: [0; 2], broken: false, tally: Tally::NEW }
    }

    pub fn pairs(&self) -> u64 {
        self.tally.pairs
    }

    pub fn failures(&self) -> u64 {
        self.tally.failures
    }

    pub fn broken(&self) -> bool {
        self.broken
    }

    /// The channel's eye size, once made.
    pub fn size(&self) -> Option<(u32, u32)> {
        self.writer.size()
    }

    /// The writer for a frame of `desc`, making the channel at the first one `ready` accepts (a
    /// size the game should present at); none while it is not ready, or after a failure.
    fn writer(&mut self, device: &ID3D11Device, desc: &D3D11_TEXTURE2D_DESC, ready: impl FnOnce(u32, u32) -> bool) -> Option<&mut FenceProducer11> {
        if self.broken || (self.writer.size().is_none() && !ready(desc.Width, desc.Height)) {
            return None;
        }
        self.writer.get(device, desc.Width, desc.Height, desc.Format).ready(|note| log!("{note}; swapchain format {} samples {}", desc.Format.0, desc.SampleDesc.Count))
    }

    /// Copies `source` (the back buffer at eye `eye`'s present, in pair `pair`) into that eye's
    /// private copy. Whether it was copied (the channel ready and the copy made).
    pub fn copy_eye(&mut self, device: &ID3D11Device, context: &ID3D11DeviceContext, eye: usize, source: &ID3D11Texture2D, pair: u64, ready: impl FnOnce(u32, u32) -> bool) -> bool {
        let desc = d3d::texture_desc(source);
        if self.writer(device, &desc, ready).is_none() {
            return false;
        }
        if copy_into(&mut self.eyes[eye], device, context, source, &d3d::private_copy_desc(&desc)).is_none() {
            log!("eye image allocation failed; not publishing");
            self.broken = true;
            return false;
        }
        self.pair[eye] = pair;
        true
    }

    /// Whether both eyes of `pair` have been copied.
    pub fn complete(&self, pair: u64) -> bool {
        self.eyes.iter().all(Option::is_some) && self.pair == [pair, pair]
    }

    /// Publishes the copied pair with its poses, the record first (to `head`).
    pub fn publish_pair(&mut self, context: &ID3D11DeviceContext, poses: [HeadPose; 2], head: &HeadSource, tick: u64) -> Step {
        let [Some(left), Some(right)] = self.eyes.clone() else { return Step::Waiting };
        self.publish(context, [&left, &right], poses, head, tick)
    }

    /// Publishes `image` to both eyes without head poses: a frame the viewer shows on its flat
    /// screen.
    pub fn publish_flat(&mut self, device: &ID3D11Device, context: &ID3D11DeviceContext, image: &ID3D11Texture2D, head: &HeadSource, tick: u64, ready: impl FnOnce(u32, u32) -> bool) -> Step {
        let desc = d3d::texture_desc(image);
        if self.writer(device, &desc, ready).is_none() {
            return Step::Waiting;
        }
        self.publish(context, [image, image], FLAT, head, tick)
    }

    fn publish(&mut self, context: &ID3D11DeviceContext, eyes: [&ID3D11Texture2D; 2], poses: [HeadPose; 2], head: &HeadSource, tick: u64) -> Step {
        let Some(writer) = self.writer.made() else { return Step::Waiting };
        record_first(head, writer.next_sequence(), poses, |_| {});
        match self.tally.count(writer.publish_whole(context, eyes, tick)) {
            Ok(step) => step,
            Err(e) => {
                log!("publish failed: {e}; not publishing");
                self.broken = true;
                self.tally.failed()
            }
        }
    }
}

#[cfg(test)]
mod tests;
