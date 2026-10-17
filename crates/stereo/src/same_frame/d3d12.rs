//! Same-frame pairs on D3D12: each eye is presented on its own through the game's swapchain, and
//! the game's present entry hands each present to [`present`]. The left eye is kept on the game's
//! queue ([`PairPublisher12::hold`]) so it survives the right eye being drawn over the same back
//! buffer; the right eye's present publishes the pair over the fence channel, with both eyes' depth
//! while it goes to the headset (`crate::depth_layer`). With the game's HUD layer
//! ([`PairLoop::hud`](super::PairLoop::hud)) each eye goes out with the HUD laid over it. A frame of
//! no eye without a pair lately goes out flat.
//!
//! With per-eye frame generation (`crate::framegen12::pairs`) each eye goes to its generator
//! instead, and with frame warping on a game fed one eye per frame (`crate::afw::d3d12`'s
//! `frame_warping_pairs`) each frame (the HUD laid over) goes to the warp, its pair of one moment
//! made there; both publish through the pair sink ([`sink`]): any feature's finished pairs go out
//! there, on the loop's channel, counted with its own.

use super::{EyeHud, PAIRS, Settings};
use crate::pairs::d3d12::{PairDepth, PairPublisher12, Step};
use monaka_channel::{ChannelName, d3d12};
use monaka_contract::protocol::HeadPose;
use monaka_core::sync::Locked;
use monaka_producer::{log, log_first};
use monaka_warp::warp12::DepthWarp;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::*};
use windows::Win32::Graphics::Direct3D12::{D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_PRESENT, D3D12_RESOURCE_STATES, ID3D12CommandQueue, ID3D12Resource};
use windows::Win32::Graphics::Dxgi::IDXGISwapChain3;
use windows::core::Interface;

static PUBLISHER: Mutex<Option<PairPublisher12>> = Mutex::new(None);
/// The pair the eyes belong to (a left eye starts one, a right eye ends it), and the one whose left
/// eye is held.
static PAIR: AtomicU64 = AtomicU64::new(0);
static HELD_PAIR: AtomicU64 = AtomicU64::new(0);
/// The held left eye's pose.
static LEFT_POSE: Mutex<HeadPose> = Mutex::new(HeadPose::EMPTY);
/// Lays the HUD layer over an eye's frame (made on first use).
static OVERLAY: Mutex<Option<DepthWarp>> = Mutex::new(None);
static OVERLAY_FAILED: AtomicBool = AtomicBool::new(false);
/// Eyes with the HUD laid over so far.
static OVERLAID: AtomicU64 = AtomicU64::new(0);

pub(crate) fn install(channel: ChannelName) {
    *PUBLISHER.locked() = Some(PairPublisher12::new(channel));
}

/// Lets go of the channel and the lay-over once the GPU is done with them (leaked if it never is).
pub(crate) fn close() {
    if let Some(mut publisher) = PUBLISHER.locked().take() {
        publisher.close();
    }
    if let Some(overlay) = OVERLAY.locked().take()
        && !overlay.idle()
    {
        std::mem::forget(overlay);
    }
}

/// The frame about to present on `swap` (the game's present entry, before the game presents it):
/// counted, the channel made for it once the game presents at the size it may publish; then `own`
/// asked (a stereo method of the game's own, given the back buffer: whether it took the frame) and,
/// when it does not take it, the frame published as its frame label says: a left eye held, a right
/// eye's pair published, a frame of no eye flat when no pair came lately. Whether the frame's eye
/// was taken (held, or its pair published).
pub fn present(swap: &IDXGISwapChain3, own: impl FnOnce(&ID3D12Resource) -> Option<bool>) -> bool {
    PAIRS.present();
    let Some(settings) = PAIRS.settings() else { return false };
    // SAFETY: COM calls on the game's live swapchain, on the thread that presents it.
    let Ok(buffer) = (unsafe { swap.GetBuffer::<ID3D12Resource>(swap.GetCurrentBackBufferIndex()) }) else { return false };
    if !channel_ready(&settings, &buffer) {
        return false;
    }
    if let Some(taken) = own(&buffer) {
        return taken;
    }
    let label = settings.presenting();
    if let Some(label) = label
        && crate::framegen12::pairs::enabled()
    {
        let Some(queue) = crate::queue12::game_queue(None) else { return false };
        return crate::framegen12::pairs::present(&queue, &buffer, label.eye.min(1), label.pose, settings.hud);
    }
    match label {
        Some(label) => eye(&settings, &buffer, label.eye.min(1), label.pose),
        // Frame warping takes every frame of no eye too (its own flat and unlabelled frames).
        None if crate::afw::d3d12::on_same_frame() => crate::queue12::game_queue(None).is_some_and(|queue| crate::afw::d3d12::pair_present(&queue, &buffer, D3D12_RESOURCE_STATE_PRESENT, None)),
        None => {
            if !PAIRS.recent_pair() {
                sink([(&buffer, D3D12_RESOURCE_STATE_PRESENT); 2], None, None);
            }
            false
        }
    }
}

/// Whether the channel is there for `buffer`: made at the first frame the game may publish, on its
/// queue.
fn channel_ready(settings: &Settings, buffer: &ID3D12Resource) -> bool {
    let mut guard = PUBLISHER.locked();
    let Some(publisher) = guard.as_mut().filter(|p| !p.broken()) else { return false };
    if publisher.made() {
        return true;
    }
    // SAFETY: reads a descriptor.
    let desc = unsafe { buffer.GetDesc() };
    if !(settings.ready)(desc.Width as u32, desc.Height) {
        return false;
    }
    let Some(queue) = crate::queue12::game_queue(None) else {
        log_first(3, format_args!("game queue unavailable; not publishing yet"));
        return false;
    };
    if !publisher.ready_for(buffer, &queue) {
        return false;
    }
    log!("D3D12 fence channel ready on queue {:?}", queue.as_raw());
    true
}

/// Eye `eye`'s frame (`buffer`, drawn from `pose`): the HUD laid over it, then held (left) or its
/// pair published (right). Whether it was taken.
fn eye(settings: &Settings, buffer: &ID3D12Resource, eye: usize, pose: HeadPose) -> bool {
    let queue = crate::queue12::game_queue(None);
    let hud = queue.as_ref().zip(settings.hud).and_then(|(queue, hud)| (hud.take)(queue, buffer, eye));
    let shown = queue.as_ref().zip(hud.as_ref()).and_then(|(queue, hud)| lay(queue, buffer, hud, eye));
    if let (Some(queue), Some(pair_hud)) = (&queue, settings.hud) {
        (pair_hud.done)(queue, hud.as_ref());
    }
    let (image, state) = shown.map_or((buffer.clone(), D3D12_RESOURCE_STATE_PRESENT), |eye| (eye, D3D12_RESOURCE_STATE_COMMON));
    if crate::afw::d3d12::on_same_frame() {
        // One eye per frame, warped into the other: every frame its own pair.
        let label = Some(crate::driver::Pending { eye, pose });
        return queue.is_some_and(|queue| crate::afw::d3d12::pair_present(&queue, &image, state, label));
    }
    let pair = settings.pair(eye, &PAIR);
    let mut guard = PUBLISHER.locked();
    let Some(publisher) = guard.as_mut() else { return false };
    if eye == 0 {
        *LEFT_POSE.locked() = pose;
        let held = publisher.hold(&image, state);
        if held {
            HELD_PAIR.store(pair, Release);
        }
        return held;
    }
    if HELD_PAIR.load(Acquire) != pair {
        return false;
    }
    let Some(held) = publisher.held() else { return false };
    // Each eye's depth to the headset with it, while it goes there (the right eye's mapping).
    let depth = crate::depth_layer::pair_depth(1);
    let poses = [*LEFT_POSE.locked(), pose];
    let step = publisher.publish_with_depth([(&held, D3D12_RESOURCE_STATE_COMMON), (&image, state)], depth.as_ref(), poses, PAIRS.head(), |p| settings.report(p));
    let published = step == Step::Published;
    PAIRS.count(published);
    published
}

/// Eye `eye`'s frame (`buffer`, in PRESENT) with `hud` laid over it, into an image of the
/// lay-over's own (in COMMON). `None`: the lay-over could not run (the plain frame goes out).
fn lay(queue: &ID3D12CommandQueue, buffer: &ID3D12Resource, hud: &EyeHud, eye: usize) -> Option<ID3D12Resource> {
    let mut guard = OVERLAY.locked();
    if guard.is_none() && !OVERLAY_FAILED.load(Acquire) {
        match d3d12::device_of(queue).map_err(|e| e.to_string()).and_then(|device| DepthWarp::new(&device)) {
            Ok(made) => {
                log!("HUD lay-over ready");
                *guard = Some(made);
            }
            Err(why) => {
                OVERLAY_FAILED.store(true, Release);
                log!("HUD lay-over unavailable: {why}");
            }
        }
    }
    let overlay = guard.as_mut()?;
    overlay.use_layer(&hud.layer);
    let shown = match overlay.overlay(queue, buffer, D3D12_RESOURCE_STATE_PRESENT, &hud.params, eye) {
        Ok(true) => overlay.eye(eye).cloned(),
        Ok(false) => None,
        Err(e) => {
            log_first(1, format_args!("HUD lay-over not run: {e}"));
            None
        }
    };
    if shown.is_some() && OVERLAID.fetch_add(1, Relaxed) == 0 {
        log!("rendered eyes: the HUD layer (kept out of the frame) is laid over each eye at its place; its pieces go to the hands");
    }
    shown
}

/// The pair sink: `images` (left, right; each in its state) published as one pair on the loop's
/// channel, with `poses` (each adjusted as the loop reports them) and each eye's `depth`, or flat
/// without poses; counted with the loop's. Any thread (a pacing thread's pairs).
pub fn sink(images: [(&ID3D12Resource, D3D12_RESOURCE_STATES); 2], poses: Option<[HeadPose; 2]>, depth: Option<&PairDepth>) -> Step {
    let settings = PAIRS.settings();
    let mut guard = PUBLISHER.locked();
    let Some(publisher) = guard.as_mut() else { return Step::Failed };
    let step = match poses {
        Some(poses) => publisher.publish_with_depth(images, depth, poses, PAIRS.head(), |p| {
            if let Some(settings) = &settings {
                settings.report(p);
            }
        }),
        None => publisher.publish_flat(images, PAIRS.head()),
    };
    PAIRS.count(step == Step::Published);
    step
}

pub(crate) fn report() -> String {
    let guard = PUBLISHER.locked();
    let (pairs, failures, depth) = guard.as_ref().map_or((0, 0, 0), |p| (p.pairs(), p.failures(), p.pairs_with_depth()));
    format!("{pairs} pairs on the fence channel ({depth} with depth), {failures} failures, {} eyes with the HUD laid over", OVERLAID.load(Relaxed))
}
