//! Frames that go to the viewer's flat screen instead of into the eyes: why ([`Flat`], from the
//! game's detector, its menu or the run's situations), and a flat frame in its own shape where the
//! game draws its UI at a fixed size ([`FlatPicture`], [`shaped`]).

use monaka_core::situation::{Situation, Situations};
use monaka_core::sync::Locked;
use monaka_producer::{log, log_first};
use monaka_warp::Scaler;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering::*};
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D};

/// Why a frame goes to the viewer's flat screen (the game's whole frame to both eyes, without head
/// poses) instead of into the eyes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flat {
    /// A menu or the pause screen is up over the game.
    Menu,
    /// A full-screen video plays.
    Video,
    /// The world is not drawn: loading, a black screen.
    WorldHidden,
    /// A camera other than the player's shows the view.
    CameraTaken,
    /// Nothing in the game's frames is known yet (a game being explored): each is shown as the
    /// game drew it.
    Screen,
}

impl Flat {
    pub const ALL: [Flat; 5] = [Flat::Menu, Flat::Video, Flat::WorldHidden, Flat::CameraTaken, Flat::Screen];

    /// The run's situation of the same meaning (none for [`Flat::Screen`], which is no situation
    /// in the game).
    pub const fn situation(self) -> Option<Situation> {
        match self {
            Flat::Menu => Some(Situation::Menu),
            Flat::Video => Some(Situation::Video),
            Flat::WorldHidden => Some(Situation::WorldHidden),
            Flat::CameraTaken => Some(Situation::CameraTaken),
            Flat::Screen => None,
        }
    }

    pub fn from_situation(situation: Situation) -> Option<Flat> {
        Flat::ALL.into_iter().find(|f| f.situation() == Some(situation))
    }

    pub const fn label(self) -> &'static str {
        match self.situation() {
            Some(situation) => situation.label(),
            None => "the game on a screen",
        }
    }

    /// The reason the frame about to present goes flat: the game's detector first, then its menu,
    /// then the run's situations at `now` ([`Situations::flat_reason`]: a video, no world drawn).
    pub(crate) fn of_frame(detector: Option<fn() -> Option<Flat>>, menu: Option<fn() -> bool>, situations: &Situations, now: u64) -> Option<Flat> {
        detector
            .and_then(|detect| detect())
            .or_else(|| menu.is_some_and(|menu| menu()).then_some(Flat::Menu))
            .or_else(|| situations.flat_reason(now).and_then(Flat::from_situation))
    }

    /// As stored in an atomic: 0 for none.
    pub(super) fn code(flat: Option<Flat>) -> u8 {
        flat.map_or(0, |f| f as u8 + 1)
    }

    pub(super) fn from_code(code: u8) -> Option<Flat> {
        Flat::ALL.get(usize::from(code).checked_sub(1)?).copied()
    }
}

/// The size of the picture a flat frame of the given size holds ([`Install::flat_picture`]).
pub type FlatPicture = fn((u32, u32)) -> (u32, u32);

/// Resamples flat frames into their own shape ([`Install::flat_picture`]).
pub(super) static FLAT_SCALER: Mutex<Option<Scaler>> = Mutex::new(None);
pub(super) static FLAT_SHAPED: AtomicU64 = AtomicU64::new(0);

/// A flat frame `back` holding the game's picture of `picture`'s size stretched over it: that
/// picture in its own shape, at the frame's size. None when it cannot be made (logged once).
pub(super) fn shaped(device: &ID3D11Device, context: &ID3D11DeviceContext, back: &ID3D11Texture2D, picture: FlatPicture) -> Option<ID3D11Texture2D> {
    let desc = monaka_channel::d3d::texture_desc(back);
    let size = (desc.Width, desc.Height);
    let rect = monaka_warp::fit(picture(size), size);
    let mut scaler = FLAT_SCALER.locked();
    if scaler.is_none() {
        match Scaler::new(device) {
            Ok(made) => *scaler = Some(made),
            Err(e) => {
                log_first(1, format_args!("flat frames unavailable: {e}"));
                return None;
            }
        }
    }
    let image = match scaler.as_mut().expect("made above").run_into(context, back, size, rect) {
        Ok(image) => image.clone(),
        Err(e) => {
            log_first(1, format_args!("a flat frame failed: {e}"));
            return None;
        }
    };
    if FLAT_SHAPED.fetch_add(1, Relaxed) == 0 {
        log!("a frame with no 3D view (a movie or a loading screen) went to the flat screen");
    }
    Some(image)
}

