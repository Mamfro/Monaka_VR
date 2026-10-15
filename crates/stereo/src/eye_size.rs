//! Rendering at the headset's eye size while VR runs (`eye_size=WxH`), as a ready-made component.
//! The game is asked for the size through its own settings, which it applies on its own frames
//! (so not while paused); until it presents at that size nothing is published, since the stereo
//! channel is made at the first published frame's size. A game lists `eye_size()` before its eyes
//! and says how it is asked and how its own size goes back ([`Resizes`]); each frame its present
//! asks [`ready`] (or [`chain_ready`], or the loop step [`hold`]) before publishing.

use crate::driver::Driver;
use monaka_contract::manifest::{OWN_EYE_SIZE, Opt, RENDER_SIZE};
use monaka_core::sync::Locked;
use monaka_producer::game::{Component, Game, Lack, On, Run, Start};
use monaka_producer::log;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::*};
use std::time::{Duration, Instant};
use windows::Win32::Graphics::Dxgi::IDXGISwapChain;

/// What a game does to render at another size.
pub trait Resizes: Game {
    /// Asks the game to render at `eye` while it presents at `presented`, without waiting for it:
    /// the game applies it on its own frames. Called at a present, until it answers other than
    /// [`Ask::NotYet`].
    fn ask(eye: (u32, u32), presented: (u32, u32)) -> Ask;
    /// Asks for the game's own size back (the stop); nothing when it was never changed.
    fn put_back();
}

/// What [`Resizes::ask`] did.
pub enum Ask {
    /// Asked: the game presents at the size once it applies it.
    Asked,
    /// Not yet possible (the game's objects not made yet): asked again at the next present.
    NotYet,
    /// Not possible: the game renders at its own size, for the reason given.
    Cannot(String),
}

const OPTIONS: [Opt; 2] = [RENDER_SIZE, OWN_EYE_SIZE];

/// The eye size as a ready-made component for game `G`: on with a size asked for, the size asked
/// for at the presents after the start ([`ready`]), the game's own size back at the give-back. A
/// game says what the size means for it in its own help line (`options` by struct update).
pub const fn eye_size<G: Resizes>() -> Component<G> {
    Component {
        name: "eye size",
        options: &OPTIONS,
        on: on::<G>,
        install: want::<G>,
        give_back: give_back::<G>,
        ..Component::NONE
    }
}

fn on<G: Game>(run: &Run<'_, G>) -> On {
    On::when(run.options.size("eye_size").is_some(), "the game renders at its own size")
}

/// The component's install: the run's eye size wanted, publishing held for it. A game whose own
/// install does more first calls it at its end.
pub fn want<G: Game>(start: &mut Start<'_, G>) -> Result<(), Lack> {
    if let Some(eye) = start.options.size("eye_size") {
        *GATE.locked() = Some(Gate { eye, asked: false, since: Instant::now(), presents: 0 });
        OPEN.store(false, Release);
    }
    Ok(())
}

fn give_back<G: Resizes>() {
    *GATE.locked() = None;
    OPEN.store(true, Release);
    G::put_back();
}

/// The switch in progress: the size asked for, whether the game was asked, and since when and how
/// many presents it has not presented at it.
struct Gate {
    eye: (u32, u32),
    asked: bool,
    since: Instant,
    presents: u64,
}

static GATE: Mutex<Option<Gate>> = Mutex::new(None);
/// Frames go out: no switch, the game presents at the size, or it was refused.
static OPEN: AtomicBool = AtomicBool::new(true);

/// A switch counts as refused once the game has presented at another size this long and this
/// often since it was asked: a paused game presents rarely and keeps the switch pending.
const PATIENCE: Duration = Duration::from_secs(10);
const PATIENT_PRESENTS: u64 = 90;

/// Whether a frame the game presented at `presented` may be published: no switch asked for, the
/// game presents at the eye size, or it cannot or would not. Asks the game ([`Resizes::ask`]) the
/// first time it can.
pub fn ready<G: Resizes>(presented: (u32, u32)) -> bool {
    if OPEN.load(Acquire) {
        return true;
    }
    let mut gate = GATE.locked();
    let Some(switch) = gate.as_mut() else { return true };
    let (width, height) = switch.eye;
    let open = if presented == switch.eye {
        log!("eye size: presenting at {width}x{height} after {:.1} s", switch.since.elapsed().as_secs_f32());
        true
    } else if !switch.asked {
        match G::ask(switch.eye, presented) {
            Ask::Asked => {
                log!("eye size: asked for {width}x{height} (presenting {}x{}); publishing waits for it (the game applies it while it has focus)", presented.0, presented.1);
                (switch.asked, switch.since, switch.presents) = (true, Instant::now(), 0);
                false
            }
            Ask::NotYet => false,
            Ask::Cannot(why) => {
                log!("eye size: rendering at the game's own {}x{}: {why}", presented.0, presented.1);
                true
            }
        }
    } else {
        switch.presents += 1;
        let refused = switch.presents >= PATIENT_PRESENTS && switch.since.elapsed() >= PATIENCE;
        if refused {
            log!("eye size: still {}x{} {} s and {} presents after asking for {width}x{height}; publishing at that size", presented.0, presented.1, PATIENCE.as_secs(), switch.presents);
        }
        refused
    };
    if open {
        *gate = None;
        OPEN.store(true, Release);
    }
    open
}

/// [`ready`] for a frame on `chain`, its size read from the swapchain.
pub fn chain_ready<G: Resizes>(chain: &IDXGISwapChain) -> bool {
    if OPEN.load(Acquire) {
        return true;
    }
    // SAFETY: a plain query of the live swapchain being presented.
    match unsafe { chain.GetDesc() } {
        Ok(desc) => ready::<G>((desc.BufferDesc.Width, desc.BufferDesc.Height)),
        Err(_) => false,
    }
}

/// Frames held for the eye size ([`hold`], [`hold_labelled`]).
static HELD: AtomicU64 = AtomicU64::new(0);

/// The present loop's step before a present ([`crate::feature::Loop::before`]): nothing published
/// until the game presents at the eye size.
pub fn hold<G: Resizes>(_: &Driver, chain: &IDXGISwapChain, _: u64) -> bool {
    let held = !chain_ready::<G>(chain);
    if held {
        HELD.fetch_add(1, Relaxed);
    }
    held
}

/// [`hold`] for a game whose frames are labelled by their camera: a frame held goes with its label.
pub fn hold_labelled<G: Resizes>(driver: &Driver, chain: &IDXGISwapChain, n: u64) -> bool {
    let held = hold::<G>(driver, chain, n);
    if held {
        driver.take_label();
    }
    held
}

/// Frames held for the eye size so far.
pub fn held() -> u64 {
    HELD.load(Relaxed)
}

#[cfg(test)]
mod tests;
