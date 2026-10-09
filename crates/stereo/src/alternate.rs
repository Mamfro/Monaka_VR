//! Alternate-eye publishing over a D3D11 swapchain: every presented frame is one eye, and each
//! left/right pair goes to the viewer with the head poses its two frames were rendered from (see
//! [`monaka_core::alternate`] for the timing).
//!
//! The game crate, after each present `n`, makes the next frame render eye
//! `Schedule::camera_eye(n)` from a head pose and gives that pose to [`AlternatePublisher::record_pose`];
//! before each present it calls [`AlternatePublisher::before_present`], which copies the back
//! buffer as the eye it shows and publishes completed pairs, the pose record first.

use monaka_channel::keyed::LazyWriter;
use monaka_channel::pose::HeadSource;
use monaka_channel::{ChannelName, d3d};
use monaka_core::alternate::{POSE_RING, Schedule};
use monaka_core::protocol::HeadPose;
use monaka_producer::log;
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::IDXGISwapChain;

/// What a present did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Nothing shown yet that belongs to a pair.
    Waiting,
    /// A left eye was copied.
    Left,
    /// A pair was published.
    Published,
    /// A right eye arrived without a left one (after a resize or a failure).
    Unpaired,
    Failed,
}

pub struct AlternatePublisher {
    schedule: Schedule,
    writer: LazyWriter,
    eyes: [Option<ID3D11Texture2D>; 2],
    size: (u32, u32),
    left_ready: bool,
    ring: [HeadPose; POSE_RING],
    pairs: u64,
    failures: u64,
}

impl AlternatePublisher {
    pub fn new(channel: ChannelName, schedule: Schedule) -> Self {
        Self {
            schedule,
            writer: LazyWriter::new(channel),
            eyes: [None, None],
            size: (0, 0),
            left_ready: false,
            ring: [HeadPose::default(); POSE_RING],
            pairs: 0,
            failures: 0,
        }
    }

    pub fn schedule(&self) -> Schedule {
        self.schedule
    }

    /// The pose the frame drawn after present `n` is rendered with.
    pub fn record_pose(&mut self, n: u64, pose: HeadPose) {
        self.ring[Schedule::ring_slot(n)] = pose;
    }

    /// The back buffer size of the last copied eye.
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

    /// At present `n` (counted from attachment), before the game's present: copies the back buffer
    /// as the eye it shows, and at a right eye publishes the pair, its pose record first (to
    /// `head`). `report` adjusts each eye's pose before it goes out (for example the field of view
    /// actually rendered).
    pub fn before_present(&mut self, chain: &IDXGISwapChain, n: u64, head: &HeadSource, report: impl Fn(&mut HeadPose)) -> Step {
        if self.schedule.shown_eye(n).is_none() {
            return Step::Waiting;
        }
        let Ok((source, device, context)) = d3d::swapchain_parts(chain) else { return self.failed() };
        self.publish_image(&device, &context, &source, n, head, report)
    }

    /// [`AlternatePublisher::before_present`] with `source` as the eye present `n` shows instead
    /// of the back buffer (an upscaled image, at its own size).
    pub fn publish_image(
        &mut self,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        source: &ID3D11Texture2D,
        n: u64,
        head: &HeadSource,
        report: impl Fn(&mut HeadPose),
    ) -> Step {
        let Some(eye) = self.schedule.shown_eye(n) else { return Step::Waiting };
        let desc = d3d::texture_desc(source);
        if self.size != (desc.Width, desc.Height) {
            self.eyes = [None, None];
            self.left_ready = false;
            self.size = (desc.Width, desc.Height);
        }
        if self.eyes[eye].is_none() {
            if desc.SampleDesc.Count != 1 {
                return self.failed();
            }
            match d3d::create_texture(device, &d3d::private_copy_desc(&desc)) {
                Ok(texture) => self.eyes[eye] = Some(texture),
                Err(_) => return self.failed(),
            }
        }
        let Some(target) = &self.eyes[eye] else { return self.failed() };
        // SAFETY: copies the back buffer into a same-shaped texture on its own device.
        unsafe { context.CopyResource(target, source) };
        if eye == 0 {
            self.left_ready = true;
            return Step::Left;
        }
        if !std::mem::take(&mut self.left_ready) {
            return Step::Unpaired;
        }

        let Some(writer) = self.writer.get(device, desc.Width, desc.Height).ready(|note| log!("{note}")) else {
            self.failures += 1;
            return Step::Failed;
        };
        // The pose record goes out first under the sequence this pair will get, so a viewer that
        // sees the images always finds their poses.
        let slots = self.schedule.pair_slots(n);
        let mut eyes = [self.ring[slots[0]], self.ring[slots[1]]];
        eyes.iter_mut().filter(|e| e.valid != 0).for_each(&report);
        head.write_pair(writer.next_sequence(), eyes);
        let (Some(left), Some(right)) = (&self.eyes[0], &self.eyes[1]) else { return self.failed() };
        match writer.publish(context, [left, right], monaka_channel::tick()) {
            Ok(true) => {
                self.pairs += 1;
                Step::Published
            }
            Ok(false) => self.failed(),
            Err(e) => {
                if self.failures == 0 {
                    log!("publish failed: {e}");
                }
                self.failed()
            }
        }
    }

    /// Drops the eye copies (end of a run); the channel stays for the next one.
    pub fn drop_eyes(&mut self) {
        self.eyes = [None, None];
        self.left_ready = false;
    }
}
