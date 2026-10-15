//! Alternate-eye publishing over a D3D11 swapchain: every presented frame is one eye, and each
//! left/right pair goes to the viewer with the head poses its two frames were rendered from (see
//! [`monaka_core::alternate`] for the timing).
//!
//! The game crate, after each present `n`, makes the next frame render eye
//! `Schedule::camera_eye(n)` from a head pose and gives that pose to [`AlternatePublisher::record_pose`];
//! before each present it calls [`AlternatePublisher::before_present`], which copies the back
//! buffer as the eye it shows and publishes completed pairs, the pose record first.

use super::Step;
use crate::publish::{FLAT, Tally, copy_into, record_first};
use monaka_channel::fence11::LazyProducer11;
use monaka_channel::pose::HeadSource;
use monaka_channel::{ChannelName, d3d};
use monaka_core::alternate::{POSE_RING, Schedule};
use monaka_contract::protocol::HeadPose;
use monaka_producer::log;
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::IDXGISwapChain;

pub struct AlternatePublisher {
    schedule: Schedule,
    writer: LazyProducer11,
    eyes: [Option<ID3D11Texture2D>; 2],
    size: (u32, u32),
    left_ready: bool,
    ring: [HeadPose; POSE_RING],
    tally: Tally,
}

impl AlternatePublisher {
    pub fn new(channel: ChannelName, schedule: Schedule) -> Self {
        Self {
            schedule,
            writer: LazyProducer11::new(channel),
            eyes: [None, None],
            size: (0, 0),
            left_ready: false,
            ring: [HeadPose::default(); POSE_RING],
            tally: Tally::NEW,
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
        self.tally.pairs
    }

    pub fn failures(&self) -> u64 {
        self.tally.failures
    }

    /// At present `n` (counted from attachment), before the game's present: copies the back buffer
    /// as the eye it shows, and at a right eye publishes the pair, its pose record first (to
    /// `head`). `report` adjusts each eye's pose before it goes out (for example the field of view
    /// actually rendered).
    pub fn before_present(&mut self, chain: &IDXGISwapChain, n: u64, head: &HeadSource, report: impl Fn(&mut HeadPose)) -> Step {
        if self.schedule.shown_eye(n).is_none() {
            return Step::Waiting;
        }
        let Ok((source, device, context)) = d3d::swapchain_parts(chain) else { return self.tally.failed() };
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
        if !self.copy_eye(device, context, eye, source) {
            return self.tally.failed();
        }
        if eye == 0 {
            self.left_ready = true;
            return Step::Left;
        }
        if !std::mem::take(&mut self.left_ready) {
            return Step::Unpaired;
        }
        let slots = self.schedule.pair_slots(n);
        self.send(device, context, head, [self.ring[slots[0]], self.ring[slots[1]]], Some(self.schedule.pair_keys(n)), report)
    }

    /// At present `n`: `image` as both eyes of a pair without head poses, which the viewer shows on
    /// its flat screen (a frame with no 3D view: a movie, a loading screen). Sent at the presents
    /// that end a pair, so flat frames keep the pairs' pace (and show one eye's frames only).
    pub fn publish_flat(&mut self, device: &ID3D11Device, context: &ID3D11DeviceContext, image: &ID3D11Texture2D, n: u64, head: &HeadSource) -> Step {
        let Some(eye) = self.schedule.shown_eye(n) else { return Step::Waiting };
        // A left eye copied before this is not paired with a flat frame.
        self.left_ready = false;
        if eye == 0 {
            return Step::Waiting;
        }
        if !(self.copy_eye(device, context, 0, image) && self.copy_eye(device, context, 1, image)) {
            return self.tally.failed();
        }
        self.send(device, context, head, FLAT, None, |_| {})
    }

    /// Both eyes from one image drawn from `pose` (a game shown in a single view that follows the
    /// head): published at every present, the runtime reprojecting it to the head as it moves.
    pub fn publish_mono(&mut self, device: &ID3D11Device, context: &ID3D11DeviceContext, image: &ID3D11Texture2D, head: &HeadSource, pose: HeadPose) -> Step {
        self.left_ready = false;
        if !(self.copy_eye(device, context, 0, image) && self.copy_eye(device, context, 1, image)) {
            return self.tally.failed();
        }
        self.send(device, context, head, [pose, pose], None, |_| {})
    }

    /// Copies `source` as eye `eye`, into a copy made at its size (both made again when the size
    /// changes).
    fn copy_eye(&mut self, device: &ID3D11Device, context: &ID3D11DeviceContext, eye: usize, source: &ID3D11Texture2D) -> bool {
        let desc = d3d::texture_desc(source);
        if self.size != (desc.Width, desc.Height) {
            self.eyes = [None, None];
            self.left_ready = false;
            self.size = (desc.Width, desc.Height);
        }
        if self.eyes[eye].is_none() && desc.SampleDesc.Count != 1 {
            return false;
        }
        copy_into(&mut self.eyes[eye], device, context, source, &d3d::private_copy_desc(&desc)).is_some()
    }

    /// Publishes the two eye copies with `poses`, the poses they were rendered from (none: flat),
    /// naming the frames they were drawn in (`keys`) for the debug drawing.
    fn send(&mut self, device: &ID3D11Device, context: &ID3D11DeviceContext, head: &HeadSource, poses: [HeadPose; 2], keys: Option<[u64; 2]>, report: impl Fn(&mut HeadPose)) -> Step {
        let (Some(left), Some(right)) = (&self.eyes[0], &self.eyes[1]) else { return self.tally.failed() };
        let (width, height) = self.size;
        let format = d3d::texture_desc(left).Format;
        let Some(writer) = self.writer.get(device, width, height, format).ready(|note| log!("{note}")) else { return self.tally.failed() };
        let sequence = writer.next_sequence();
        record_first(head, sequence, poses, report);
        if let Some(keys) = keys {
            monaka_producer::debug::eye_keys(sequence, keys);
        }
        match self.tally.count(writer.publish_whole(context, [left, right], monaka_channel::tick())) {
            Ok(step) => step,
            Err(e) => {
                if self.tally.failures == 0 {
                    log!("publish failed: {e}");
                }
                self.tally.failed()
            }
        }
    }

    /// Drops the eye copies (end of a run); the channel stays for the next one.
    pub fn drop_eyes(&mut self) {
        self.eyes = [None, None];
        self.left_ready = false;
    }
}

#[cfg(test)]
mod tests;
