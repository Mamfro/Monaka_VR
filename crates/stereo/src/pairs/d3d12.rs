//! Any two D3D12 textures published as one stereo pair over the fence channel, each eye with its
//! own head pose: for producers that make their pairs themselves (same-frame stereo, where the
//! left eye is held until the right eye's present; alternate-eye with per-eye frame generation: a
//! generated eye beside the other eye's real frame of the same moment; depth stereo's two warped
//! eyes), and under [`crate::alternate::d3d12`]. A frame with no pose (a menu, a loading screen) goes
//! out flat ([`PairPublisher12::publish_flat`]): the viewer shows it on its flat screen.
//!
//! A pair may carry each eye's depth ([`PairPublisher12::publish_with_depth`]), which the viewer
//! gives the headset's runtime for its own reprojection of late frames. Depth the channel refuses
//! is not sent again for the rest of the run; the pairs go on without it.

pub use crate::alternate::Step;
use crate::publish::{FLAT, Tally, record_first};
use monaka_channel::ChannelName;
use monaka_channel::fence::{Depth, EyeDepth, FenceProducer};
use monaka_channel::pose::HeadSource;
use monaka_contract::protocol::{DepthParams, HeadPose};
use monaka_producer::log;
use windows::Win32::Graphics::Direct3D12::{D3D12_RESOURCE_STATES, ID3D12CommandQueue, ID3D12Resource};

/// One eye's depth beside its image (`monaka_channel::fence::EyeDepth`, owned): the texture
/// (32-bit float or 16-bit depth, not 24-bit), its state when the copy is queued, and the part of
/// it that lines up with the eye image, left, top, width, height in texels (`None`: all of it).
#[derive(Clone)]
pub struct EyeDepth12 {
    pub texture: ID3D12Resource,
    pub state: D3D12_RESOURCE_STATES,
    pub rect: Option<[u32; 4]>,
}

/// Both eyes' depth with one pair (left, right; one size and format), and how it maps to distance.
#[derive(Clone)]
pub struct PairDepth {
    pub eyes: [EyeDepth12; 2],
    pub params: DepthParams,
}

pub struct PairPublisher12 {
    channel: ChannelName,
    producer: Option<FenceProducer>,
    /// The channel could not be made (or the image changed size): nothing more is published.
    broken: bool,
    size: (u32, u32),
    tally: Tally,
    last: Option<(u64, [HeadPose; 2])>,
    /// The debug frame keys of the next pair's eyes, when its caller named them.
    eye_keys: Option<[u64; 2]>,
    /// The channel refused the depth once: the pairs go on without it.
    depth_refused: bool,
    /// The first pair with depth was logged.
    depth_logged: bool,
}

impl PairPublisher12 {
    pub fn new(channel: ChannelName) -> Self {
        Self { channel, producer: None, broken: false, size: (0, 0), tally: Tally::NEW, last: None, eye_keys: None, depth_refused: false, depth_logged: false }
    }

    pub fn pairs(&self) -> u64 {
        self.tally.pairs
    }

    /// Pairs published with each eye's depth.
    pub fn pairs_with_depth(&self) -> u64 {
        self.tally.with_depth
    }

    pub fn failures(&self) -> u64 {
        self.tally.failures
    }

    /// Counts a failure of the caller's own (a back buffer out of reach).
    pub(crate) fn failed(&mut self) -> Step {
        self.tally.failed()
    }

    /// Publishing stopped for good (the channel failed, or the game resized).
    pub fn broken(&self) -> bool {
        self.broken
    }

    /// Whether the channel is made (at the first frame's size).
    pub fn made(&self) -> bool {
        self.producer.is_some()
    }

    /// The eye image size the channel was made with.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// The last published pair's sequence and the poses its record carried.
    pub fn last_pair(&self) -> Option<(u64, [HeadPose; 2])> {
        self.last
    }

    fn broke(&mut self, why: String) -> Step {
        log!("{why}; not publishing");
        self.broken = true;
        self.tally.failed()
    }

    /// Whether the channel is there for images like `like` (the game's back buffer: its size and
    /// format), made on the game's queue the first time.
    pub fn ready_for(&mut self, like: &ID3D12Resource, queue: &ID3D12CommandQueue) -> bool {
        if self.broken {
            return false;
        }
        if self.producer.is_none() {
            match FenceProducer::for_back_buffer(&self.channel, like, queue) {
                Ok((producer, (width, height))) => {
                    log!("D3D12 fence channel {width}x{height}");
                    (self.producer, self.size) = (Some(producer), (width, height));
                }
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

    /// Names the debug frame keys of the next pair's eyes (`monaka_producer::debug::eye_keys`).
    pub fn name_eyes(&mut self, keys: [u64; 2]) {
        self.eye_keys = Some(keys);
    }

    /// Publishes `eyes` (left, right; each in its state) with their poses, the pose record first
    /// (to `head`); `report` adjusts each pose before it goes out. The copies go on the queue the
    /// channel was made with, after what was submitted there before.
    pub fn publish(&mut self, eyes: [(&ID3D12Resource, D3D12_RESOURCE_STATES); 2], poses: [HeadPose; 2], head: &HeadSource, report: impl Fn(&mut HeadPose)) -> Step {
        self.publish_with_depth(eyes, None, poses, head, report)
    }

    /// [`PairPublisher12::publish`] with each eye's `depth` beside its image (`None`: this pair has
    /// none), copied on the same queue. Depth the channel refuses is left out of this pair and every
    /// later one, logged once; the pair goes out without it.
    pub fn publish_with_depth(
        &mut self,
        eyes: [(&ID3D12Resource, D3D12_RESOURCE_STATES); 2],
        depth: Option<&PairDepth>,
        poses: [HeadPose; 2],
        head: &HeadSource,
        report: impl Fn(&mut HeadPose),
    ) -> Step {
        if self.broken {
            return Step::Failed;
        }
        let Some(producer) = self.producer.as_mut() else { return Step::Waiting };
        let depth = depth.filter(|_| !self.depth_refused);
        let sequence = producer.next_sequence();
        let poses = record_first(head, sequence, poses, report);
        if let Some(keys) = self.eye_keys.take() {
            monaka_producer::debug::eye_keys(sequence, keys);
        }
        let tick = monaka_channel::tick();
        let channel_depth = depth.map(|d| Depth { eyes: [0, 1].map(|e| EyeDepth { texture: &d.eyes[e].texture, state: d.eyes[e].state, rect: d.eyes[e].rect }), params: d.params });
        let mut published = producer.publish_with_depth(eyes[0], eyes[1], channel_depth, tick);
        if let (Err(e), Some(_)) = (&published, depth) {
            // The depth is checked before anything is copied: the same pair again, without it.
            log!("each eye's depth refused ({e}); the pairs go to the headset without depth from now on");
            self.depth_refused = true;
            published = producer.publish(eyes[0], eyes[1], tick);
        }
        match self.tally.count(published) {
            Ok(step) => {
                if step == Step::Published {
                    self.last = Some((sequence, poses));
                    if let Some(depth) = depth.filter(|_| !self.depth_refused) {
                        self.tally.with_depth += 1;
                        self.log_depth(depth);
                    }
                }
                step
            }
            Err(e) => self.broke(format!("publish failed: {e}")),
        }
    }

    /// The first pair with depth: what goes to the headset.
    fn log_depth(&mut self, depth: &PairDepth) {
        if std::mem::replace(&mut self.depth_logged, true) {
            return;
        }
        // SAFETY: reads a descriptor.
        let desc = unsafe { depth.eyes[0].texture.GetDesc() };
        let [left, top, width, height] = depth.eyes[0].rect.unwrap_or([0, 0, desc.Width as u32, desc.Height]);
        let p = depth.params;
        let far = if p.far.is_finite() { format!("{} m", p.far) } else { "infinite".into() };
        log!(
            "each eye's depth to the headset: {width}x{height} at ({left}, {top}) of {}x{} format {}, near {} m, far {far}{}",
            desc.Width,
            desc.Height,
            desc.Format.0,
            p.near,
            if p.reversed != 0 { ", reversed" } else { "" }
        );
    }

    /// Publishes `eyes` without head poses: a frame the viewer shows on its flat screen.
    pub fn publish_flat(&mut self, eyes: [(&ID3D12Resource, D3D12_RESOURCE_STATES); 2], head: &HeadSource) -> Step {
        self.publish(eyes, FLAT, head, |_| {})
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
                self.broke(format!("left-eye copy failed: {e}"));
                false
            }
        }
    }

    /// The eye kept by the last [`PairPublisher12::hold`], in the common state.
    pub fn held(&self) -> Option<ID3D12Resource> {
        self.producer.as_ref().and_then(|p| p.held())
    }

    /// Keeps `source`'s depth (in `state`) beside the eye [`PairPublisher12::hold`] keeps, so it
    /// survives the other eye's frame being drawn; publish it later from
    /// [`PairPublisher12::held_depth`]. Whether it was kept (not: no recording free this instant,
    /// no channel, or depth refused, which is logged once and not tried again).
    pub fn hold_depth(&mut self, source: &ID3D12Resource, state: D3D12_RESOURCE_STATES) -> bool {
        if self.broken || self.depth_refused {
            return false;
        }
        let Some(producer) = self.producer.as_mut() else { return false };
        match producer.hold_depth(source, state) {
            Ok(held) => held,
            Err(e) => {
                log!("an eye's depth could not be kept ({e}); the pairs go to the headset without depth from now on");
                self.depth_refused = true;
                false
            }
        }
    }

    /// The depth kept by the last [`PairPublisher12::hold_depth`] (typeless R32 or R16, the
    /// source's size), in the common state.
    pub fn held_depth(&self) -> Option<ID3D12Resource> {
        self.producer.as_ref().and_then(|p| p.held_depth())
    }

    /// Lets go of the channel (end of a run).
    pub fn close(&mut self) {
        self.producer = None;
    }
}
