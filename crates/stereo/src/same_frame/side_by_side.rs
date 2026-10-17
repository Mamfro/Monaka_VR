//! Same-frame pairs drawn side by side in one present: the game draws both eyes into one back
//! buffer (left half, right half) and its present entry hands each present to [`present`], with
//! the pose its frame was drawn from (or none: the loop's ticket). The halves are copied out on the
//! swapchain's own device and published as one pair over the fence channel: on D3D12 into eye
//! images of the loop's own on the game's queue, on D3D11 straight from the back buffer's halves.
//! Each eye's image is half the back buffer; any format the fence channel carries (an engine's
//! 10-bit back buffer too).
//!
//! The channel is made at the first present whose back buffer the game says may be published
//! ([`PairLoop::ready`](super::PairLoop::ready), its whole size); a resized back buffer after that
//! stops the publishing for the run.

use super::{PAIRS, Settings};
use crate::pairs::d3d12::{PairPublisher12, Step};
use crate::publish::record_first;
use monaka_channel::{ChannelName, d3d, d3d12, fence11::FenceProducer11};
use monaka_contract::protocol::HeadPose;
use monaka_core::sync::Locked;
use monaka_producer::log;
use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering::*};
use windows::Win32::Graphics::Direct3D11::{D3D11_BOX, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D};
use windows::Win32::Graphics::Direct3D12::{
    D3D12_BOX, D3D12_RESOURCE_FLAG_NONE, D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COPY_SOURCE,
    D3D12_RESOURCE_STATE_PRESENT, ID3D12CommandQueue, ID3D12Resource,
};
use windows::Win32::Graphics::Dxgi::{IDXGISwapChain, IDXGISwapChain3};
use windows::core::Interface;

static OUTPUT: Mutex<Option<Sides>> = Mutex::new(None);
/// Presents with no pose to publish them with.
static UNPOSED: AtomicU64 = AtomicU64::new(0);

/// The run's channel, and the publisher made for the swapchain the game presents on.
struct Sides {
    channel: ChannelName,
    publisher: Option<Publisher>,
    /// Stopped for the run (logged once).
    broken: bool,
}

/// The first swapchain's publisher (a present on another swapchain is not the game's frame).
enum Publisher {
    D3D12 { swap: usize, eyes: Box<Eyes12> },
    D3D11 { swap: usize, eyes: Box<Eyes11> },
}

pub(crate) fn install(channel: ChannelName) {
    if let Err(why) = crate::swapchain_queue::learn() {
        log!("side-by-side pairs: the swapchain's queue cannot be read ({why}); the game's queue is asked instead");
    }
    *OUTPUT.locked() = Some(Sides { channel, publisher: None, broken: false });
}

/// Lets go of the channel once the GPU is done with the eye images (leaked if it never is).
pub(crate) fn close() {
    let Some(sides) = OUTPUT.locked().take() else { return };
    match sides.publisher {
        Some(Publisher::D3D12 { mut eyes, .. }) => {
            eyes.publisher.close();
            if !eyes.recorder.idle(2000) {
                std::mem::forget(eyes);
                log!("side-by-side pairs: GPU work still pending at the stop; the eye images are kept");
            }
        }
        Some(Publisher::D3D11 { eyes, .. }) => log!("side-by-side pairs: {} on the D3D11 fence channel", eyes.published),
        None => {}
    }
}

/// The frame about to present on `swap` (the game's present entry, before the game presents it):
/// both halves of its back buffer published as one pair drawn from `pose` (none: the oldest frame
/// built and not shown, [`super::Pairs::built`]); not published without a pose, or until the game
/// says its back buffer's size may be. Whether the pair went out.
///
/// # Safety
/// `swap` is the live DXGI swapchain whose present is running on this thread.
pub unsafe fn present(swap: *mut c_void, pose: Option<HeadPose>) -> bool {
    PAIRS.present();
    let Some(settings) = PAIRS.settings() else { return false };
    let Some(pose) = pose.or_else(|| PAIRS.shown().0) else {
        UNPOSED.fetch_add(1, Relaxed);
        return false;
    };
    // SAFETY: the caller's guarantee.
    let Some(chain) = (unsafe { IDXGISwapChain::from_raw_borrowed(&swap) }) else { return false };
    let mut guard = OUTPUT.locked();
    let Some(sides) = guard.as_mut().filter(|s| !s.broken) else { return false };
    let published = match publish(sides, &settings, chain, swap as usize, pose) {
        Ok(published) => published,
        Err(why) => {
            log!("side-by-side pairs stopped for this run: {why}");
            sides.broken = true;
            Some(false)
        }
    };
    if let Some(published) = published {
        PAIRS.count(published);
        if published {
            PAIRS.pair_done("side-by-side", 0);
        }
    }
    published == Some(true)
}

/// The pair of `chain`'s back buffer, made ready on its first present that may be published:
/// whether it went out (none: not tried, the channel not made yet or no copy free this instant).
fn publish(sides: &mut Sides, settings: &Settings, chain: &IDXGISwapChain, swap: usize, pose: HeadPose) -> Result<Option<bool>, String> {
    if sides.publisher.is_none() {
        let Some(made) = made_for(sides, settings, chain, swap)? else { return Ok(None) };
        sides.publisher = Some(made);
    }
    match sides.publisher.as_mut() {
        Some(Publisher::D3D12 { swap: ours, eyes }) if *ours == swap => {
            let chain = chain.cast::<IDXGISwapChain3>().map_err(|e| e.to_string())?;
            // SAFETY: COM calls on the swapchain presenting, on its thread.
            let buffer = unsafe { chain.GetBuffer::<ID3D12Resource>(chain.GetCurrentBackBufferIndex()) }.map_err(|e| e.to_string())?;
            eyes.publish(&buffer, pose, settings)
        }
        Some(Publisher::D3D11 { swap: ours, eyes }) if *ours == swap => {
            // SAFETY: the flip-model swapchain's current back buffer is buffer 0.
            let buffer = unsafe { chain.GetBuffer::<ID3D11Texture2D>(0) }.map_err(|e| e.to_string())?;
            eyes.publish(&buffer, pose, settings)
        }
        _ => Ok(None),
    }
}

/// The publisher for `chain` (D3D12 or D3D11, as its device is), once its back buffer may be
/// published; none before.
fn made_for(sides: &Sides, settings: &Settings, chain: &IDXGISwapChain, swap: usize) -> Result<Option<Publisher>, String> {
    // SAFETY: a query on the live swapchain.
    if let Ok(device) = unsafe { chain.GetDevice::<ID3D11Device>() } {
        // SAFETY: the flip-model swapchain's current back buffer is buffer 0.
        let buffer = unsafe { chain.GetBuffer::<ID3D11Texture2D>(0) }.map_err(|e| e.to_string())?;
        let desc = d3d::texture_desc(&buffer);
        if !(settings.ready)(desc.Width, desc.Height) {
            return Ok(None);
        }
        return Eyes11::new(&sides.channel, &device, &buffer).map(|eyes| Some(Publisher::D3D11 { swap, eyes: Box::new(eyes) }));
    }
    let chain3 = chain.cast::<IDXGISwapChain3>().map_err(|e| e.to_string())?;
    // SAFETY: COM calls on the swapchain presenting, on its thread.
    let buffer = unsafe { chain3.GetBuffer::<ID3D12Resource>(chain3.GetCurrentBackBufferIndex()) }.map_err(|e| e.to_string())?;
    // SAFETY: reads a descriptor.
    let desc = unsafe { buffer.GetDesc() };
    if !(settings.ready)(desc.Width as u32, desc.Height) {
        return Ok(None);
    }
    let queue = crate::queue12::game_queue(Some(&chain3)).ok_or("the game's queue is unknown")?;
    Eyes12::new(&sides.channel, &buffer, queue).map(|eyes| Some(Publisher::D3D12 { swap, eyes: Box::new(eyes) }))
}

/// Each eye's half of a back buffer of `size` (left, top, right, bottom).
fn half(eye: u32, size: (u32, u32)) -> [u32; 4] {
    [eye * size.0, 0, (eye + 1) * size.0, size.1]
}

/// Checks a side-by-side back buffer (`width` x `height`, `samples`, `format`): one eye's size.
fn eye_size(width: u32, height: u32, samples: u32, format: u32) -> Result<(u32, u32), String> {
    if samples != 1 || !width.is_multiple_of(2) || !monaka_contract::protocol::fence_format_ok(format) {
        return Err(format!("a back buffer of {width}x{height}, format {format}, {samples} samples cannot be split into eyes"));
    }
    Ok((width / 2, height))
}

/// D3D12: the eyes copied into images of the loop's own on the game's queue, then published.
struct Eyes12 {
    queue: ID3D12CommandQueue,
    recorder: d3d12::Recorder,
    images: [ID3D12Resource; 2],
    publisher: PairPublisher12,
    size: (u32, u32),
    format: i32,
}

impl Eyes12 {
    fn new(channel: &ChannelName, buffer: &ID3D12Resource, queue: ID3D12CommandQueue) -> Result<Self, String> {
        // SAFETY: reads a descriptor.
        let desc = unsafe { buffer.GetDesc() };
        let size = eye_size(desc.Width as u32, desc.Height, desc.SampleDesc.Count, desc.Format.0 as u32)?;
        let device = d3d12::device_of(&queue).map_err(|e| e.to_string())?;
        let image = || d3d12::texture(&device, size.0, size.1, desc.Format, D3D12_RESOURCE_FLAG_NONE).map_err(|e| e.to_string());
        let images = [image()?, image()?];
        let mut publisher = PairPublisher12::new(channel.clone());
        if !publisher.ready_for(&images[0], &queue) {
            return Err("the D3D12 fence channel could not be made".into());
        }
        let recorder = d3d12::Recorder::new(&device, 4).map_err(|e| e.to_string())?;
        log!("side-by-side pairs: {}x{} per eye, format {}, on the D3D12 fence channel", size.0, size.1, desc.Format.0);
        Ok(Self { queue, recorder, images, publisher, size, format: desc.Format.0 })
    }

    /// Both halves of `buffer` (in PRESENT) copied out and published from `pose`.
    fn publish(&mut self, buffer: &ID3D12Resource, pose: HeadPose, settings: &Settings) -> Result<Option<bool>, String> {
        // SAFETY: reads a descriptor.
        let desc = unsafe { buffer.GetDesc() };
        if (desc.Width as u32, desc.Height, desc.Format.0) != (self.size.0 * 2, self.size.1, self.format) {
            return Err(format!("the back buffer changed to {}x{} format {}", desc.Width, desc.Height, desc.Format.0));
        }
        let Some(list) = self.recorder.begin().map_err(|e| e.to_string())? else { return Ok(None) };
        d3d12::barrier(&list, buffer, D3D12_RESOURCE_STATE_PRESENT, D3D12_RESOURCE_STATE_COPY_SOURCE);
        for (eye, image) in self.images.iter().enumerate() {
            let [left, top, right, bottom] = half(eye as u32, self.size);
            let region = D3D12_BOX { left, top, front: 0, right, bottom, back: 1 };
            d3d12::barrier(&list, image, D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_COPY_DEST);
            // SAFETY: an in-bounds half of the back buffer into a same-format image of that size.
            unsafe { list.CopyTextureRegion(&d3d12::subresource(image, 0), 0, 0, 0, &d3d12::subresource(buffer, 0), Some(&region)) };
            d3d12::barrier(&list, image, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COMMON);
        }
        d3d12::barrier(&list, buffer, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_PRESENT);
        self.recorder.submit(&self.queue).map_err(|e| e.to_string())?;
        let eyes = [(&self.images[0], D3D12_RESOURCE_STATE_COMMON), (&self.images[1], D3D12_RESOURCE_STATE_COMMON)];
        match self.publisher.publish(eyes, [pose, pose], PAIRS.head(), |p| settings.report(p)) {
            Step::Waiting => Ok(None),
            Step::Failed if self.publisher.broken() => Err("the D3D12 fence channel broke".into()),
            step => Ok(Some(step == Step::Published)),
        }
    }
}

/// D3D11: the back buffer's halves straight into the channel's slot, on the swapchain device's
/// immediate context (the present runs on the thread that renders).
struct Eyes11 {
    context: ID3D11DeviceContext,
    producer: FenceProducer11,
    size: (u32, u32),
    format: i32,
    published: u64,
}

impl Eyes11 {
    fn new(channel: &ChannelName, device: &ID3D11Device, buffer: &ID3D11Texture2D) -> Result<Self, String> {
        let desc = d3d::texture_desc(buffer);
        let size = eye_size(desc.Width, desc.Height, desc.SampleDesc.Count, desc.Format.0 as u32)?;
        // SAFETY: the swapchain device's own immediate context.
        let context = unsafe { device.GetImmediateContext() }.map_err(|e| e.to_string())?;
        let producer = FenceProducer11::create(channel, device, size.0, size.1, desc.Format).map_err(|e| format!("the D3D11 fence channel ({}x{} format {}): {e}", size.0, size.1, desc.Format.0))?;
        log!("side-by-side pairs: {}x{} per eye, format {}, on the D3D11 fence channel", size.0, size.1, desc.Format.0);
        Ok(Self { context, producer, size, format: desc.Format.0, published: 0 })
    }

    fn publish(&mut self, buffer: &ID3D11Texture2D, pose: HeadPose, settings: &Settings) -> Result<Option<bool>, String> {
        let desc = d3d::texture_desc(buffer);
        if (desc.Width, desc.Height, desc.Format.0) != (self.size.0 * 2, self.size.1, self.format) {
            return Err(format!("the back buffer changed to {}x{} format {}", desc.Width, desc.Height, desc.Format.0));
        }
        record_first(PAIRS.head(), self.producer.next_sequence(), [pose, pose], |p| settings.report(p));
        let region = |eye| {
            let [left, top, right, bottom] = half(eye, self.size);
            D3D11_BOX { left, top, front: 0, right, bottom, back: 1 }
        };
        let published = self.producer.publish(&self.context, buffer, [region(0), region(1)], monaka_channel::tick()).map_err(|e| e.to_string())?;
        self.published += u64::from(published);
        Ok(Some(published))
    }
}

pub(crate) fn report() -> String {
    let guard = OUTPUT.locked();
    let pairs = match guard.as_ref().and_then(|s| s.publisher.as_ref()) {
        Some(Publisher::D3D12 { eyes, .. }) => format!("{} pairs on the D3D12 fence channel, {} failures", eyes.publisher.pairs(), eyes.publisher.failures()),
        Some(Publisher::D3D11 { eyes, .. }) => format!("{} pairs on the D3D11 fence channel", eyes.published),
        None => "no channel made".to_owned(),
    };
    format!("{pairs}; {} presents without a pose", UNPOSED.load(Relaxed))
}

#[cfg(test)]
mod tests;
