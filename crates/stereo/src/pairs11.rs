//! Same-frame pairs over the D3D11 keyed-mutex channel: a game that renders both eyes on one
//! frame copies each eye out of its back buffer as that eye's present comes ([`PairPublisher11::copy_eye`])
//! and publishes the pair at the second ([`PairPublisher11::publish_pair`]), the pose record
//! first; a frame with no pair (a menu, a loading screen) goes out flat
//! ([`PairPublisher11::publish_flat`]). The channel is made at the first frame's size, once the
//! game says the size is right (`ready`: a producer waiting for the headset's eye size).

pub use crate::alternate::Step;
use monaka_channel::keyed::{KeyedWriter, LazyWriter};
use monaka_channel::pose::HeadSource;
use monaka_channel::{ChannelName, d3d};
use monaka_core::protocol::HeadPose;
use monaka_producer::log;
use windows::Win32::Graphics::Direct3D11::{D3D11_TEXTURE2D_DESC, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D};

pub struct PairPublisher11 {
    writer: LazyWriter,
    /// Private copies of the two eyes, at the back buffer's shape.
    eyes: Option<[ID3D11Texture2D; 2]>,
    /// The pair each copy belongs to (the game's own numbering), so a right eye is published
    /// only beside its own left eye.
    pair: [u64; 2],
    /// Publishing stopped for good (the channel failed).
    broken: bool,
    pairs: u64,
    failures: u64,
}

impl PairPublisher11 {
    pub fn new(channel: ChannelName) -> Self {
        Self { writer: LazyWriter::new(channel), eyes: None, pair: [0; 2], broken: false, pairs: 0, failures: 0 }
    }

    pub fn pairs(&self) -> u64 {
        self.pairs
    }

    pub fn failures(&self) -> u64 {
        self.failures
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
    fn writer(&mut self, device: &ID3D11Device, desc: &D3D11_TEXTURE2D_DESC, ready: impl FnOnce(u32, u32) -> bool) -> Option<&mut KeyedWriter> {
        if self.broken || (self.writer.size().is_none() && !ready(desc.Width, desc.Height)) {
            return None;
        }
        self.writer.get(device, desc.Width, desc.Height).ready(|note| log!("{note}; swapchain format {} samples {}", desc.Format.0, desc.SampleDesc.Count))
    }

    fn failed(&mut self, what: &str, error: Option<monaka_channel::Error>) -> Step {
        self.failures += 1;
        if let Some(e) = error {
            log!("{what} failed: {e}; not publishing");
            self.broken = true;
        }
        Step::Failed
    }

    /// Copies `source` (the back buffer at eye `eye`'s present, in pair `pair`) into that eye's
    /// private copy. Whether it was copied (the channel ready and the copies made).
    pub fn copy_eye(&mut self, device: &ID3D11Device, context: &ID3D11DeviceContext, eye: usize, source: &ID3D11Texture2D, pair: u64, ready: impl FnOnce(u32, u32) -> bool) -> bool {
        let desc = d3d::texture_desc(source);
        if self.writer(device, &desc, ready).is_none() {
            return false;
        }
        if self.eyes.is_none() {
            let copy = d3d::private_copy_desc(&desc);
            match (d3d::create_texture(device, &copy), d3d::create_texture(device, &copy)) {
                (Ok(left), Ok(right)) => self.eyes = Some([left, right]),
                _ => {
                    log!("eye image allocation failed; not publishing");
                    self.broken = true;
                    return false;
                }
            }
        }
        let Some(eyes) = &self.eyes else { return false };
        d3d::copy_texture(context, &eyes[eye], source);
        self.pair[eye] = pair;
        true
    }

    /// Whether both eyes of `pair` have been copied.
    pub fn complete(&self, pair: u64) -> bool {
        self.eyes.is_some() && self.pair == [pair, pair]
    }

    /// Publishes the copied pair with its poses (the record first, to `head`; none without
    /// poses: a frame the viewer shows flat).
    pub fn publish_pair(&mut self, context: &ID3D11DeviceContext, poses: Option<[HeadPose; 2]>, head: &HeadSource, tick: u64) -> Step {
        let Some([left, right]) = self.eyes.clone() else { return Step::Waiting };
        self.publish(context, [&left, &right], poses, head, tick)
    }

    /// Publishes `image` to both eyes, no pose record: a frame the viewer shows flat.
    pub fn publish_flat(&mut self, device: &ID3D11Device, context: &ID3D11DeviceContext, image: &ID3D11Texture2D, tick: u64, ready: impl FnOnce(u32, u32) -> bool) -> Step {
        let desc = d3d::texture_desc(image);
        if self.writer(device, &desc, ready).is_none() {
            return Step::Waiting;
        }
        self.publish(context, [image, image], None, head_none(), tick)
    }

    fn publish(&mut self, context: &ID3D11DeviceContext, eyes: [&ID3D11Texture2D; 2], poses: Option<[HeadPose; 2]>, head: &HeadSource, tick: u64) -> Step {
        let Some(writer) = self.writer.made() else { return Step::Waiting };
        if let Some(poses) = poses {
            // The pose record goes out first under the sequence this pair will get, so a viewer
            // that sees the images always finds their poses.
            head.write_pair(writer.next_sequence(), poses);
        }
        match writer.publish(context, eyes, tick) {
            Ok(true) => {
                self.pairs += 1;
                Step::Published
            }
            Ok(false) => self.failed("publish", None),
            Err(e) => self.failed("publish", Some(e)),
        }
    }
}

/// No pose record goes anywhere.
fn head_none() -> &'static HeadSource {
    static NONE: HeadSource = HeadSource::new();
    &NONE
}
