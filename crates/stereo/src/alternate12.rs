//! Alternate-eye publishing over a D3D12 swapchain: the D3D12 twin of [`crate::alternate`], over
//! the fence channel. Every presented frame is one eye; the left eye's back buffer is kept
//! ([`FenceProducer::hold`]) on the game's queue, and the right eye's present publishes the pair
//! with the head poses both were rendered from (see [`monaka_core::alternate`] for the timing).
//!
//! The game crate, after each present `n`, makes the next frame render eye
//! `Schedule::camera_eye(n)` from a head pose and gives that pose to
//! [`AlternatePublisher12::record_pose`]; before each present it calls
//! [`AlternatePublisher12::before_present`] with the swapchain and the game's direct queue (the
//! copies go on that queue, ahead of the present).

pub use crate::alternate::Step;
use monaka_channel::fence::FenceProducer;
use monaka_channel::pose::HeadSource;
use monaka_channel::ChannelName;
use monaka_core::alternate::{POSE_RING, Schedule};
use monaka_core::protocol::HeadPose;
use monaka_producer::log;
use windows::Win32::Graphics::Direct3D12::{D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_PRESENT, ID3D12CommandQueue, ID3D12Resource};
use windows::Win32::Graphics::Dxgi::IDXGISwapChain3;

pub struct AlternatePublisher12 {
    schedule: Schedule,
    channel: ChannelName,
    producer: Option<FenceProducer>,
    /// The channel could not be made (or the image changed size): nothing more is published.
    broken: bool,
    size: (u32, u32),
    left_ready: bool,
    ring: [HeadPose; POSE_RING],
    pairs: u64,
    failures: u64,
    last: Option<(u64, [HeadPose; 2])>,
    /// The pose of the left eye kept by [`AlternatePublisher12::show_labelled`].
    left_pose: HeadPose,
}

/// The fence channel at `buffer`'s size and format (the game's back buffer), on the game queue's
/// device; and that size.
pub(crate) fn create_producer(channel: &ChannelName, buffer: &ID3D12Resource, queue: &ID3D12CommandQueue) -> Result<(FenceProducer, (u32, u32)), String> {
    let (producer, (width, height)) = FenceProducer::for_back_buffer(channel, buffer, queue)?;
    log!("D3D12 fence channel {width}x{height}");
    Ok((producer, (width, height)))
}

impl AlternatePublisher12 {
    pub fn new(channel: ChannelName, schedule: Schedule) -> Self {
        Self {
            schedule,
            channel,
            producer: None,
            broken: false,
            size: (0, 0),
            left_ready: false,
            ring: [HeadPose::default(); POSE_RING],
            pairs: 0,
            failures: 0,
            last: None,
            left_pose: HeadPose::default(),
        }
    }

    /// The last published pair's sequence and the poses its record carried.
    pub fn last_pair(&self) -> Option<(u64, [HeadPose; 2])> {
        self.last
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
        self.size
    }

    pub fn pairs(&self) -> u64 {
        self.pairs
    }

    pub fn failures(&self) -> u64 {
        self.failures
    }

    fn failed(&mut self) -> Step {
        self.failures += 1;
        Step::Failed
    }

    fn broke(&mut self, why: String) -> Step {
        log!("{why}; not publishing");
        self.broken = true;
        self.failed()
    }

    /// Makes the channel at the first frame's size, on the game queue's device.
    fn create(&mut self, buffer: &ID3D12Resource, queue: &ID3D12CommandQueue) -> Result<FenceProducer, String> {
        let (producer, size) = create_producer(&self.channel, buffer, queue)?;
        self.size = size;
        Ok(producer)
    }

    /// At present `n` (counted from attachment), before the game's present: keeps the back buffer
    /// as the eye it shows, and at a right eye publishes the pair, its pose record first (to
    /// `head`). `report` adjusts each eye's pose before it goes out (for example the field of view
    /// actually rendered).
    pub fn before_present(&mut self, swap: &IDXGISwapChain3, queue: &ID3D12CommandQueue, n: u64, head: &HeadSource, report: impl Fn(&mut HeadPose)) -> Step {
        let Some(eye) = self.schedule.shown_eye(n) else { return Step::Waiting };
        let slots = self.schedule.pair_slots(n);
        self.show(swap, queue, eye, [self.ring[slots[0]], self.ring[slots[1]]], head, report)
    }

    /// Like [`AlternatePublisher12::before_present`] for a frame whose eye and head pose are known
    /// exactly (labelled where it was rendered), so no latency is assumed: a left eye is kept with
    /// its pose, and a right eye completes the pair with the left eye kept last.
    pub fn show_labelled(&mut self, swap: &IDXGISwapChain3, queue: &ID3D12CommandQueue, eye: usize, pose: HeadPose, head: &HeadSource, report: impl Fn(&mut HeadPose)) -> Step {
        if eye == 0 {
            self.left_pose = pose;
        }
        self.show(swap, queue, eye.min(1), [self.left_pose, pose], head, report)
    }

    /// Keeps `eye`'s frame, or with a right eye publishes the pair rendered from `poses`.
    fn show(&mut self, swap: &IDXGISwapChain3, queue: &ID3D12CommandQueue, eye: usize, poses: [HeadPose; 2], head: &HeadSource, report: impl Fn(&mut HeadPose)) -> Step {
        if self.broken {
            return Step::Failed;
        }
        // SAFETY: COM calls on the game's live swapchain, on the thread that presents it.
        let Ok(buffer) = (unsafe { swap.GetBuffer::<ID3D12Resource>(swap.GetCurrentBackBufferIndex()) }) else { return self.failed() };
        if self.producer.is_none() {
            match self.create(&buffer, queue) {
                Ok(producer) => self.producer = Some(producer),
                Err(why) => return self.broke(why),
            }
        }
        // SAFETY: reads a descriptor.
        let desc = unsafe { buffer.GetDesc() };
        if (desc.Width as u32, desc.Height) != self.size {
            // The channel's slots have one size; a resized game needs a new run.
            let (w, h) = self.size;
            return self.broke(format!("the game resized from {w}x{h} to {}x{}", desc.Width, desc.Height));
        }
        let producer = self.producer.as_mut().expect("created above");
        if eye == 0 {
            return match producer.hold(&buffer, D3D12_RESOURCE_STATE_PRESENT) {
                Ok(true) => {
                    self.left_ready = true;
                    Step::Left
                }
                Ok(false) => {
                    self.left_ready = false;
                    Step::Waiting
                }
                Err(e) => self.broke(format!("left-eye copy failed: {e}")),
            };
        }
        if !std::mem::take(&mut self.left_ready) {
            return Step::Unpaired;
        }
        let Some(held) = producer.held() else { return self.failed() };
        // The pose record goes out first under the sequence this pair will get, so a viewer that
        // sees the images always finds their poses.
        let mut eyes = poses;
        eyes.iter_mut().filter(|e| e.valid != 0).for_each(&report);
        let sequence = producer.next_sequence();
        head.write_pair(sequence, eyes);
        match producer.publish((&held, D3D12_RESOURCE_STATE_COMMON), (&buffer, D3D12_RESOURCE_STATE_PRESENT), monaka_channel::tick()) {
            Ok(true) => {
                self.pairs += 1;
                self.last = Some((sequence, eyes));
                Step::Published
            }
            Ok(false) => self.failed(),
            Err(e) => self.broke(format!("publish failed: {e}")),
        }
    }

    /// Lets go of the channel (end of a run). Its GPU objects are freed once the GPU is done with
    /// them, or leaked if it never is.
    pub fn close(&mut self) {
        self.producer = None;
        self.left_ready = false;
    }
}
