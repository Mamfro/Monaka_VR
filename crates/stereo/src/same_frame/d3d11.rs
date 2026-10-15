//! Same-frame pairs on D3D11: each eye is presented on its own through the game's swapchain. The
//! game's present entry hands each eye's present to [`around_eye_present`], which copies the eye out
//! of the back buffer just before the present runs and, at the right eye, publishes the pair over
//! the fence channel. DXGI's Present (hooked here) keeps the left eye's present off the
//! monitor, and sends a frame without a pair lately (a menu, loading, no head pose) flat.
//!
//! The game is recognised by its window, not its swapchain: a mode change (the eye size asked for)
//! can give it a new one.

use super::{PAIRS, Settings};
use crate::pairs::d3d11::{PairPublisher11, Step};
use crate::present::{PresentFn, hook_dxgi_present};
use monaka_channel::{ChannelName, d3d};
use monaka_contract::protocol::HeadPose;
use monaka_core::sync::Locked;
use monaka_hook::{Hooks, InFlight, Original};
use monaka_producer::Rejection;
use std::cell::Cell;
use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::*};
use windows::Win32::Foundation::S_OK;
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::{DXGI_PRESENT_TEST, IDXGISwapChain};
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
use windows::core::{HRESULT, Interface};

static PUBLISHER: Mutex<Option<PairPublisher11>> = Mutex::new(None);
static DXGI_PRESENT: Original<PresentFn> = Original::new();
/// The game's window (its first D3D11 swapchain's).
static GAME_WINDOW: AtomicUsize = AtomicUsize::new(0);
static ALL_PRESENTS: AtomicU64 = AtomicU64::new(0);
static HIDDEN: AtomicU64 = AtomicU64::new(0);
/// The pair the eyes copied belong to: a left eye starts one, a right eye ends it.
static PAIR: AtomicU64 = AtomicU64::new(0);
/// The left eye's pose, for its pair.
static LEFT_POSE: Mutex<HeadPose> = Mutex::new(HeadPose::EMPTY);

thread_local! {
    /// The swapchain whose eye present is being dispatched on this thread.
    static DISPATCHED: Cell<usize> = const { Cell::new(0) };
}

/// The publisher on `channel`, and DXGI's Present hooked.
///
/// # Safety
/// Called once at start, before the hooks are enabled.
pub(crate) unsafe fn install(hooks: &mut Hooks, channel: ChannelName) -> Result<(), Rejection> {
    *PUBLISHER.locked() = Some(PairPublisher11::new(channel));
    // SAFETY: the caller's guarantee; `dxgi_present` forwards to the original.
    unsafe { hook_dxgi_present(hooks, &DXGI_PRESENT, dxgi_present) }
}

pub(crate) fn close() {
    PUBLISHER.locked().take();
}

/// The game dispatches an eye's present of the swapchain `swap` by `call`: the eye the frame labels
/// say is presenting is copied out of the back buffer first, and after a right eye the pair is
/// published. Whether the eye was copied, and the call's result.
pub fn around_eye_present<R>(swap: usize, call: impl FnOnce() -> R) -> (bool, R) {
    let _flight = InFlight::enter();
    let label = PAIRS.settings().and_then(|s| s.presenting().map(|label| (s, label)));
    DISPATCHED.set(swap);
    let copied = label.is_some_and(|(settings, label)| copy_eye(&settings, swap, label.eye, label.pose));
    let result = call();
    DISPATCHED.set(0);
    (copied, result)
}

/// The back buffer, its device and immediate context.
fn back_buffer(swap: &IDXGISwapChain) -> Option<(ID3D11Texture2D, ID3D11Device, ID3D11DeviceContext)> {
    d3d::swapchain_parts(swap).ok()
}

/// Copies eye `eye` (drawn from `pose`) out of `swap`'s back buffer; after the right eye the pair
/// goes out, its record first. Whether it was copied. The channel opens at the first frame the game
/// says may be published.
fn copy_eye(settings: &Settings, swap: usize, eye: usize, pose: HeadPose) -> bool {
    let raw = swap as *mut c_void;
    // SAFETY: the swapchain of the present being dispatched.
    let Some(swap) = (unsafe { IDXGISwapChain::from_raw_borrowed(&raw) }) else { return false };
    let Some((buffer, device, context)) = back_buffer(swap) else { return false };
    let mut guard = PUBLISHER.locked();
    let Some(publisher) = guard.as_mut().filter(|p| !p.broken()) else { return false };
    let pair = settings.pair(eye, &PAIR);
    if eye == 0 {
        *LEFT_POSE.locked() = pose;
    }
    if !publisher.copy_eye(&device, &context, eye.min(1), &buffer, pair, settings.ready) {
        return false;
    }
    if eye == 0 || !publisher.complete(pair) {
        return true;
    }
    let poses = [*LEFT_POSE.locked(), pose];
    let step = publisher.publish_pair(&context, poses, PAIRS.head(), monaka_channel::tick());
    PAIRS.count(step == Step::Published);
    true
}

/// `IDXGISwapChain::Present`, for every swapchain in the process.
unsafe extern "system" fn dxgi_present(swap: *mut c_void, interval: u32, flags: u32) -> HRESULT {
    let flight = InFlight::enter();
    if PAIRS.publishing() && flags & DXGI_PRESENT_TEST.0 == 0 {
        ALL_PRESENTS.fetch_add(1, Relaxed);
        // SAFETY: DXGI's own swapchain argument.
        if let Some(chain) = unsafe { IDXGISwapChain::from_raw_borrowed(&swap) }
            && let Some(settings) = PAIRS.settings()
        {
            // SAFETY: COM calls on a live swapchain.
            let window = unsafe { chain.GetDesc() }.map_or(0, |d| d.OutputWindow.0 as usize);
            if GAME_WINDOW.load(Relaxed) == 0 && window != 0 && unsafe { chain.GetDevice::<ID3D11Device>() }.is_ok() {
                GAME_WINDOW.store(window, Relaxed);
            }
            if window != 0 && window == GAME_WINDOW.load(Relaxed) {
                PAIRS.present();
                let eye = settings.presenting().map(|label| label.eye);
                match shown(swap as usize == DISPATCHED.get(), eye, PAIRS.recent_pair()) {
                    Shown::Hidden => {
                        HIDDEN.fetch_add(1, Relaxed);
                        return S_OK;
                    }
                    Shown::Flat => publish_flat(chain, &settings),
                    Shown::AsIs => {}
                }
            }
        }
    }
    // SAFETY: the original, with DXGI's arguments.
    unsafe { DXGI_PRESENT.get(&flight)(swap, interval, flags) }
}

/// What a present of the game's window does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Shown {
    /// The left eye's present: kept off the monitor.
    Hidden,
    /// A frame of no eye without a pair lately: also published flat.
    Flat,
    /// Shown on the monitor as it is.
    AsIs,
}

/// A present of the game's window: `dispatched` (the swapchain whose eye present is dispatched on
/// this thread), `eye` the frame labels' eye presenting, `recent` whether a pair finished lately.
/// The left eye's present is kept off the monitor: only the right eye's shows there.
pub(crate) fn shown(dispatched: bool, eye: Option<usize>, recent: bool) -> Shown {
    match eye {
        Some(0) if dispatched => Shown::Hidden,
        None if !recent => Shown::Flat,
        _ => Shown::AsIs,
    }
}

/// The frame to both eyes without head poses.
fn publish_flat(swap: &IDXGISwapChain, settings: &Settings) {
    let Some((buffer, device, context)) = back_buffer(swap) else { return };
    let mut guard = PUBLISHER.locked();
    if let Some(publisher) = guard.as_mut().filter(|p| !p.broken()) {
        let step = publisher.publish_flat(&device, &context, &buffer, PAIRS.head(), monaka_channel::tick(), settings.ready);
        PAIRS.count(step == Step::Published);
    }
}

/// Whether the game's window has focus (it applies video changes only then).
fn focused() -> bool {
    // SAFETY: plain query.
    unsafe { GetForegroundWindow() }.0 as usize == GAME_WINDOW.load(Relaxed)
}

pub(crate) fn report() -> String {
    format!("left presents hidden={}; game window focused={}; presents on any swapchain={}", HIDDEN.load(Relaxed), focused(), ALL_PRESENTS.load(Relaxed))
}
