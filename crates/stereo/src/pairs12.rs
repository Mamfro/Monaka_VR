//! Any two D3D12 textures published as one stereo pair over the fence channel, each eye with its
//! own head pose: for producers that make their pairs themselves (same-frame stereo, where the
//! left eye is held until the right eye's present; alternate-eye with per-eye frame generation: a
//! generated eye beside the other eye's real frame of the same moment; depth stereo's two warped
//! eyes). A frame with no pose (a menu, a loading screen) goes out unposed: the viewer shows it
//! flat.

pub use crate::alternate::Step;
use crate::alternate12::create_producer;
use monaka_channel::ChannelName;
use monaka_channel::fence::FenceProducer;
use monaka_channel::pose::HeadSource;
use monaka_core::protocol::HeadPose;
use monaka_producer::log;
use windows::Win32::Graphics::Direct3D12::{D3D12_RESOURCE_STATES, ID3D12CommandQueue, ID3D12Resource};
use monaka_channel::Error;

pub struct PairPublisher12 {
    channel: ChannelName,
    producer: Option<FenceProducer>,
    /// The channel could not be made (or the image changed size): nothing more is published.
    broken: bool,
    size: (u32, u32),
    pairs: u64,
    failures: u64,
}

impl PairPublisher12 {
    pub fn new(channel: ChannelName) -> Self {
        Self { channel, producer: None, broken: false, size: (0, 0), pairs: 0, failures: 0 }
    }

    pub fn pairs(&self) -> u64 {
        self.pairs
    }

    pub fn failures(&self) -> u64 {
        self.failures
    }

    /// Publishing stopped for good (the channel failed, or the game resized).
    pub fn broken(&self) -> bool {
        self.broken
    }

    /// Whether the channel is made (at the first frame's size).
    pub fn made(&self) -> bool {
        self.producer.is_some()
    }

    fn broke(&mut self, why: String) -> Step {
        log!("{why}; not publishing");
        self.broken = true;
        self.failures += 1;
        Step::Failed
    }

    /// Whether the channel is there for images like `like` (the game's back buffer: its size and
    /// format), made on the game's queue the first time.
    pub fn ready_for(&mut self, like: &ID3D12Resource, queue: &ID3D12CommandQueue) -> bool {
        if self.broken {
            return false;
        }
        if self.producer.is_none() {
            match create_producer(&self.channel, like, queue) {
                Ok((producer, size)) => (self.producer, self.size) = (Some(producer), size),
                Err(why) => {
                    self.broke(why);
                    return false;
                }
            }
        }
        // SAFETY: reads a descriptor.
        let desc = unsafe { like.GetDesc() };
        if (desc.Width as u32, desc.Height) != self.size {
            // The channel's slots have one size; a resized game needs a new run.
            let (w, h) = self.size;
            self.broke(format!("the game resized from {w}x{h} to {}x{}", desc.Width, desc.Height));
            return false;
        }
        true
    }

    /// Publishes `eyes` (left, right; each in its state) with their poses, the pose record first
    /// (to `head`); `report` adjusts each pose before it goes out. The copies go on the queue the
    /// channel was made with, after what was submitted there before.
    pub fn publish(&mut self, eyes: [(&ID3D12Resource, D3D12_RESOURCE_STATES); 2], poses: [HeadPose; 2], head: &HeadSource, report: impl Fn(&mut HeadPose)) -> Step {
        if self.broken {
            return Step::Failed;
        }
        let Some(producer) = self.producer.as_mut() else { return Step::Waiting };
        let mut poses = poses;
        poses.iter_mut().filter(|p| p.valid != 0).for_each(&report);
        // The pose record goes out first under the sequence this pair will get, so a viewer that
        // sees the images always finds their poses.
        head.write_pair(producer.next_sequence(), poses);
        match producer.publish(eyes[0], eyes[1], monaka_channel::tick()) {
            Ok(true) => {
                self.pairs += 1;
                Step::Published
            }
            Ok(false) => {
                self.failures += 1;
                Step::Failed
            }
            Err(e) => self.broke(format!("publish failed: {e}")),
        }
    }

    /// Publishes `eyes` with no pose record (a frame the viewer shows flat).
    pub fn publish_unposed(&mut self, eyes: [(&ID3D12Resource, D3D12_RESOURCE_STATES); 2]) -> Step {
        if self.broken {
            return Step::Failed;
        }
        let Some(producer) = self.producer.as_mut() else { return Step::Waiting };
        match producer.publish(eyes[0], eyes[1], monaka_channel::tick()) {
            Ok(true) => {
                self.pairs += 1;
                Step::Published
            }
            Ok(false) => {
                self.failures += 1;
                Step::Failed
            }
            Err(e) => self.broke(format!("publish failed: {e}")),
        }
    }

    /// Keeps `source` (in `state`) as a left eye on the queue, so it survives the right eye being
    /// drawn over the same back buffer; publish it later from [`PairPublisher12::held`]. Whether
    /// it was kept (not: no recording free this instant, or the channel broke).
    pub fn hold(&mut self, source: &ID3D12Resource, state: D3D12_RESOURCE_STATES) -> bool {
        if self.broken {
            return false;
        }
        let Some(producer) = self.producer.as_mut() else { return false };
        match producer.hold(source, state) {
            Ok(held) => held,
            Err(e) => {
                let kind = matches!(e, Error::Incompatible(_));
                self.broke(format!("left-eye copy failed: {e}"));
                let _ = kind;
                false
            }
        }
    }

    /// The eye kept by the last [`PairPublisher12::hold`], in the common state.
    pub fn held(&self) -> Option<ID3D12Resource> {
        self.producer.as_ref().and_then(|p| p.held())
    }

    /// Lets go of the channel (end of a run).
    pub fn close(&mut self) {
        self.producer = None;
    }
}
