//! Alternate-eye publishing over a D3D12 swapchain: the D3D12 twin of [`super::d3d11`], a
//! schedule and a pose ring over [`PairPublisher12`]. Every presented frame is one eye; the left
//! eye's back buffer is kept ([`PairPublisher12::hold`]) on the game's queue, and the right eye's
//! present publishes the pair with the head poses both were rendered from (see
//! [`monaka_core::alternate`] for the timing).
//!
//! The game crate, after each present `n`, makes the next frame render eye
//! `Schedule::camera_eye(n)` from a head pose and gives that pose to
//! [`AlternatePublisher12::record_pose`]; before each present it calls
//! [`AlternatePublisher12::before_present`] with the swapchain and the game's direct queue (the
//! copies go on that queue, ahead of the present).
//!
//! A game with each frame's depth gives it with the frame (the `_with_depth` calls, [`FrameDepth`]):
//! the left eye's depth is kept with its image ([`PairPublisher12::hold_depth`]) and the pair goes
//! out with both eyes' depth when both frames had it.

pub use crate::alternate::Step;
use crate::pairs::d3d12::{EyeDepth12, PairDepth, PairPublisher12};
use monaka_channel::ChannelName;
use monaka_channel::pose::HeadSource;
use monaka_core::alternate::{POSE_RING, Schedule};
use monaka_contract::protocol::{DepthParams, HeadPose};
use windows::Win32::Graphics::Direct3D12::{D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_PRESENT, ID3D12CommandQueue, ID3D12Resource};
use windows::Win32::Graphics::Dxgi::IDXGISwapChain3;

/// The depth of the frame about to present, for its eye: the texture, its state, the part of it
/// that lines up with the image, and how it maps to distance.
#[derive(Clone)]
pub struct FrameDepth {
    pub eye: EyeDepth12,
    pub params: DepthParams,
}

pub struct AlternatePublisher12 {
    schedule: Schedule,
    pairs: PairPublisher12,
    left_ready: bool,
    /// The kept left eye's depth: its rect (the held copy has the source's size) and parameters.
    left_depth: Option<(Option<[u32; 4]>, DepthParams)>,
    ring: [HeadPose; POSE_RING],
    /// The pose of the left eye kept by [`AlternatePublisher12::show_labelled`].
    left_pose: HeadPose,
}

impl AlternatePublisher12 {
    pub fn new(channel: ChannelName, schedule: Schedule) -> Self {
        Self {
            schedule,
            pairs: PairPublisher12::new(channel),
            left_ready: false,
            left_depth: None,
            ring: [HeadPose::default(); POSE_RING],
            left_pose: HeadPose::default(),
        }
    }

    /// The last published pair's sequence and the poses its record carried.
    pub fn last_pair(&self) -> Option<(u64, [HeadPose; 2])> {
        self.pairs.last_pair()
    }

    pub fn schedule(&self) -> Schedule {
        self.schedule
    }

    /// The pose the frame drawn after present `n` is rendered with.
    pub fn record_pose(&mut self, n: u64, pose: HeadPose) {
        self.ring[Schedule::ring_slot(n)] = pose;
    }

    /// The eye image size the channel was made with.
    pub fn size(&self) -> (u32, u32) {
        self.pairs.size()
    }

    pub fn pairs(&self) -> u64 {
        self.pairs.pairs()
    }

    /// Pairs published with each eye's depth.
    pub fn pairs_with_depth(&self) -> u64 {
        self.pairs.pairs_with_depth()
    }

    pub fn failures(&self) -> u64 {
        self.pairs.failures()
    }

    /// At present `n` (counted from attachment), before the game's present: keeps the back buffer
    /// as the eye it shows, and at a right eye publishes the pair, its pose record first (to
    /// `head`). `report` adjusts each eye's pose before it goes out (for example the field of view
    /// actually rendered).
    pub fn before_present(&mut self, swap: &IDXGISwapChain3, queue: &ID3D12CommandQueue, n: u64, head: &HeadSource, report: impl Fn(&mut HeadPose)) -> Step {
        self.before_present_with_depth(swap, queue, n, |_| None, head, report)
    }

    /// [`AlternatePublisher12::before_present`] with the frame's depth: `depth` gives it for the
    /// eye the frame shows (asked only when the schedule says which).
    pub fn before_present_with_depth(
        &mut self,
        swap: &IDXGISwapChain3,
        queue: &ID3D12CommandQueue,
        n: u64,
        depth: impl FnOnce(usize) -> Option<FrameDepth>,
        head: &HeadSource,
        report: impl Fn(&mut HeadPose),
    ) -> Step {
        let Some(eye) = self.schedule.shown_eye(n) else { return Step::Waiting };
        let slots = self.schedule.pair_slots(n);
        if eye == 1 {
            self.pairs.name_eyes(self.schedule.pair_keys(n));
        }
        self.show(swap, queue, eye, [self.ring[slots[0]], self.ring[slots[1]]], depth(eye), head, report)
    }

    /// Like [`AlternatePublisher12::before_present`] for a frame whose eye and head pose are known
    /// exactly (labelled where it was rendered), so no latency is assumed: a left eye is kept with
    /// its pose, and a right eye completes the pair with the left eye kept last.
    pub fn show_labelled(&mut self, swap: &IDXGISwapChain3, queue: &ID3D12CommandQueue, eye: usize, pose: HeadPose, head: &HeadSource, report: impl Fn(&mut HeadPose)) -> Step {
        self.show_labelled_with_depth(swap, queue, eye, pose, None, head, report)
    }

    /// [`AlternatePublisher12::show_labelled`] with the frame's `depth` (`None`: it has none, and
    /// its pair goes without).
    #[allow(clippy::too_many_arguments)]
    pub fn show_labelled_with_depth(
        &mut self,
        swap: &IDXGISwapChain3,
        queue: &ID3D12CommandQueue,
        eye: usize,
        pose: HeadPose,
        depth: Option<FrameDepth>,
        head: &HeadSource,
        report: impl Fn(&mut HeadPose),
    ) -> Step {
        if eye == 0 {
            self.left_pose = pose;
        }
        self.show(swap, queue, eye.min(1), [self.left_pose, pose], depth, head, report)
    }

    /// The back buffer about to present, once the channel is there for it (made at the first
    /// frame's size on the game queue's device).
    fn back_buffer(&mut self, swap: &IDXGISwapChain3, queue: &ID3D12CommandQueue) -> Result<ID3D12Resource, Step> {
        if self.pairs.broken() {
            return Err(Step::Failed);
        }
        // SAFETY: COM calls on the game's live swapchain, on the thread that presents it.
        let Ok(buffer) = (unsafe { swap.GetBuffer::<ID3D12Resource>(swap.GetCurrentBackBufferIndex()) }) else { return Err(self.pairs.failed()) };
        if !self.pairs.ready_for(&buffer, queue) {
            return Err(Step::Failed);
        }
        Ok(buffer)
    }

    /// Keeps `eye`'s frame (and its `depth`), or with a right eye publishes the pair rendered from
    /// `poses`, with both eyes' depth when both frames had it.
    #[allow(clippy::too_many_arguments)]
    fn show(
        &mut self,
        swap: &IDXGISwapChain3,
        queue: &ID3D12CommandQueue,
        eye: usize,
        poses: [HeadPose; 2],
        depth: Option<FrameDepth>,
        head: &HeadSource,
        report: impl Fn(&mut HeadPose),
    ) -> Step {
        let buffer = match self.back_buffer(swap, queue) {
            Ok(buffer) => buffer,
            Err(step) => return step,
        };
        if eye == 0 {
            self.left_ready = self.pairs.hold(&buffer, D3D12_RESOURCE_STATE_PRESENT);
            self.left_depth = depth
                .filter(|_| self.left_ready)
                .and_then(|d| self.pairs.hold_depth(&d.eye.texture, d.eye.state).then_some((d.eye.rect, d.params)));
            return match (self.left_ready, self.pairs.broken()) {
                (true, _) => Step::Left,
                (false, true) => Step::Failed,
                (false, false) => Step::Waiting,
            };
        }
        let left_depth = self.left_depth.take();
        if !std::mem::take(&mut self.left_ready) {
            return Step::Unpaired;
        }
        let Some(held) = self.pairs.held() else { return self.pairs.failed() };
        // The pair's depth: the kept left eye's and this one's, mapped as the newer frame says.
        let pair_depth = match (left_depth, depth, self.pairs.held_depth()) {
            (Some((left_rect, _)), Some(right), Some(left)) => Some(PairDepth {
                eyes: [EyeDepth12 { texture: left, state: D3D12_RESOURCE_STATE_COMMON, rect: left_rect }, right.eye],
                params: right.params,
            }),
            _ => None,
        };
        let eyes = [(&held, D3D12_RESOURCE_STATE_COMMON), (&buffer, D3D12_RESOURCE_STATE_PRESENT)];
        self.pairs.publish_with_depth(eyes, pair_depth.as_ref(), poses, head, report)
    }

    /// At present `n`: this frame to both eyes without head poses, which the viewer shows on its
    /// flat screen (a menu over the game). Sent at the presents that end a pair, as
    /// [`crate::alternate::d3d11::AlternatePublisher::publish_flat`]; an eye kept for a pair is let go.
    pub fn show_flat(&mut self, swap: &IDXGISwapChain3, queue: &ID3D12CommandQueue, n: u64, head: &HeadSource) -> Step {
        let Some(eye) = self.schedule.shown_eye(n) else { return Step::Waiting };
        self.left_ready = false;
        self.left_depth = None;
        if eye == 0 {
            return Step::Waiting;
        }
        match self.back_buffer(swap, queue) {
            Ok(buffer) => self.pairs.publish_flat([(&buffer, D3D12_RESOURCE_STATE_PRESENT); 2], head),
            Err(step) => step,
        }
    }

    /// Lets go of the channel (end of a run). Its GPU objects are freed once the GPU is done with
    /// them, or leaked if it never is.
    pub fn close(&mut self) {
        self.pairs.close();
        self.left_ready = false;
        self.left_depth = None;
    }
}
