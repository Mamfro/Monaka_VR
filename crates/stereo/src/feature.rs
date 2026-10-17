//! The present loop as a ready-made component: Monaka's [`DRIVER`] around DXGI's Present (the
//! game's swapchain, presents counted, the next eye and head pose chosen, the pairs published
//! over the channel), for any game drawing one eye per frame. A game lists
//! `monaka_stereo::feature::present()` among its components and says, once, what its loop adds
//! ([`Presents::LOOP`]: its D3D version, its eye-view counter, its own steps around each present);
//! its eye site (`crate::eye_site::EyeSite`, installed with the loop: [`Loop::eye_site`]) reads
//! [`Driver::pending`] for the eye and pose to draw. An engine with a present call of its own
//! hooks that and calls [`Driver::around_present`] from it ([`Entry::Game`]).

use crate::alternate::d3d11::AlternatePublisher;
use crate::alternate::d3d12::AlternatePublisher12;
use crate::driver::{Driver, Flat, FlatPicture, Install, Publisher};
use crate::present::PresentFn;
use monaka_channel::pose::PoseChannel;
use monaka_contract::protocol::HeadPose;
use monaka_hook::Original;
use monaka_producer::Rejection;
use monaka_producer::game::{Component, Game, Lack, Start};
use std::sync::atomic::AtomicU64;
use windows::Win32::Graphics::Direct3D12::ID3D12CommandQueue;
use windows::Win32::Graphics::Dxgi::IDXGISwapChain;
use windows::core::HRESULT;

/// The present loop: the game's swapchain, the eye and pose of the next frame, the pairs.
pub static DRIVER: Driver = Driver::new();

static PRESENT_ORIGINAL: Original<PresentFn> = Original::new();

/// What a build draws with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Graphics {
    /// Pairs over the fence channel, copied on the swapchain's device.
    D3D11,
    /// Pairs over the fence channel, copied on the game's direct queue ([`Loop::queue`]).
    D3D12,
}

/// Where the loop is entered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entry {
    /// DXGI's Present, hooked by the component (every swapchain; the game's is found).
    Dxgi,
    /// The game's own present call, hooked by the game, which calls [`Driver::around_present`].
    Game,
}

/// What a game's present loop adds to Monaka's ([`Install`] says what each step is for).
pub struct Loop<G: Game> {
    /// What this build draws with.
    pub graphics: fn(&G::Build) -> Graphics,
    pub entry: Entry,
    /// The game's draws of eye views, for the rate line: its name and counter.
    pub work: (&'static str, &'static AtomicU64),
    /// D3D12: the game's own way to its direct queue, where it executes through one its swapchain
    /// does not hold (Streamline's proxy). None: the swapchain's, else caught at
    /// `ExecuteCommandLists` ([`crate::queue12::game_queue`]).
    pub queue: Option<fn() -> Option<ID3D12CommandQueue>>,
    pub frame_end: Option<fn(&IDXGISwapChain)>,
    pub report: Option<fn(&mut HeadPose)>,
    pub before: Option<fn(&Driver, &IDXGISwapChain, u64) -> bool>,
    pub after: Option<fn(&Driver, u64, &HeadPose)>,
    pub adjust_pose: Option<fn(u64, &mut HeadPose)>,
    /// Whether the frame about to present goes to the flat screen, and why ([`Driver::detect_flat`]):
    /// the game's own detector, beside the run's situations.
    pub flat: Option<fn() -> Option<Flat>>,
    pub in_turn: bool,
    pub pose_each_frame: Option<fn() -> bool>,
    pub finish: Option<fn(&Driver)>,
    /// The game's eye site, installed with the loop (`crate::eye_site::EyeSite::install`).
    pub eye_site: Option<crate::eye_site::InstallFn<G>>,
    /// D3D11: a flat frame's own shape ([`crate::driver::Install::flat_picture`]).
    pub flat_picture: Option<FlatPicture>,
}

impl<G: Game> Loop<G> {
    /// A loop that adds no step of its own, drawing with `graphics`, its eye views counted by
    /// `work`.
    pub const fn new(graphics: fn(&G::Build) -> Graphics, work: (&'static str, &'static AtomicU64)) -> Self {
        Loop {
            graphics,
            entry: Entry::Dxgi,
            work,
            queue: None,
            frame_end: None,
            report: None,
            before: None,
            after: None,
            adjust_pose: None,
            flat: None,
            in_turn: false,
            pose_each_frame: None,
            finish: None,
            eye_site: None,
            flat_picture: None,
        }
    }
}

/// A game whose frames go out by the present loop.
pub trait Presents: Game {
    const LOOP: Loop<Self>;
}

/// The present loop component for game `G`. Always on (with stereo off, both eyes of each pair
/// are the game's own camera); a game whose stereo switch turns publishing off as well gives it
/// its own `on`.
pub const fn present<G: Presents>() -> Component<G> {
    Component { name: "present", needs_channel: true, install: install::<G>, arm, give_back, release, summary: Some(summary), ..Component::NONE }
}

fn install<G: Presents>(start: &mut Start<'_, G>) -> Result<(), Lack> {
    let shared = *start.shared;
    let channel = start.channel()?;
    let poses = if shared.head_tracking && shared.synthetic.is_none() {
        Some(PoseChannel::open(channel).map_err(|e| Rejection::not_ready(format!("pose channel: {e}")))?)
    } else {
        None
    };
    let graphics = (G::LOOP.graphics)(start.build);
    let publisher = match graphics {
        Graphics::D3D11 => Publisher::D3D11(AlternatePublisher::new(channel.clone(), shared.schedule)),
        Graphics::D3D12 => Publisher::D3D12(AlternatePublisher12::new(channel.clone(), shared.schedule)),
    };
    crate::queue12::use_own(G::LOOP.queue);
    let Loop { entry, work, frame_end, report, before, after, adjust_pose, flat, in_turn, pose_each_frame, finish, flat_picture, .. } = G::LOOP;
    DRIVER.install(Install { shared, publisher, poses, frame_end, report, work, before, after, adjust_pose, menu: None, in_turn, pose_each_frame, finish, flat_picture });
    if let Some(flat) = flat {
        DRIVER.detect_flat(flat);
    }
    // SAFETY: Present is DXGI's, found on a probe swapchain; ExecuteCommandLists is D3D12's, found
    // on a queue of our own; each detour has its target's signature.
    unsafe {
        if entry == Entry::Dxgi {
            crate::present::hook_dxgi_present(start.hooks, &PRESENT_ORIGINAL, dxgi_present)?;
        }
        if graphics == Graphics::D3D12 && G::LOOP.queue.is_none() {
            crate::queue12::hook(start.hooks)?;
        }
    }
    if let Some(eye_site) = G::LOOP.eye_site {
        eye_site(start)?;
    }
    Ok(())
}

/// `IDXGISwapChain::Present`, for every swapchain in the process.
unsafe extern "system" fn dxgi_present(swap: *mut core::ffi::c_void, interval: u32, flags: u32) -> HRESULT {
    let flight = monaka_hook::InFlight::enter();
    // SAFETY: the caller passes a live swapchain; the original is DXGI's Present.
    unsafe { DRIVER.present(swap, interval, flags, PRESENT_ORIGINAL.get(&flight)) }
}

fn arm() {
    DRIVER.start();
}

/// The present thread retires the eye; the game's own camera comes back with the next frame.
fn give_back() {
    DRIVER.retire(2000);
}

fn release() {
    DRIVER.release();
}

fn summary() -> String {
    DRIVER.summary()
}
