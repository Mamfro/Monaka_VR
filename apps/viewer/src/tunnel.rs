//! Tunnel vision (`tunnel_vision`, Monaka's own setting): while the player moves the character with
//! the left stick or turns it smoothly with the right, the view's edges darken, closing in as far
//! as the sticks are pushed and opening again once they are let go. The view the eyes keep while
//! the world slides past is what holds motion sickness off.
//!
//! A quad fixed to the head ([`DISTANCE`] in front, wide enough to fill any headset's view) carries
//! a black ring with a clear middle; the ring's images, from open to closed ([`LEVELS`]), are made
//! once and the swapchain refilled only when the level shown changes.

use monaka_channel::d3d::{self, simple_texture_desc};
use monaka_contract::gamepad::Gamepad;
use monaka_core::pad::STICK_DEADZONE;
use monaka_openxr::{Extent2Df, Layer, Posef, Session, Space, Swapchain};
use std::time::Instant;
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;

/// How far in front of the eyes the quad hangs, and how wide it is (metres): out to 84 degrees from
/// the middle, past any headset's view.
const DISTANCE: f32 = 0.5;
const WIDTH: f32 = 2.0 * DISTANCE * HALF_TAN;
/// The quad's half width as the tangent of the angle from the middle of the view.
const HALF_TAN: f32 = 9.5;
const PIXELS: u32 = 512;
/// Images from open (none drawn) to fully closed.
pub const LEVELS: usize = 16;
/// The clear middle when fully closed, and how wide its soft edge is (tangents of the angle from
/// the middle: 30 degrees clear, dark from about 48); fully open, the clear middle reaches past
/// the view.
const CLOSED: f32 = 0.58;
const OPEN: f32 = 2.2;
const FEATHER: f32 = 0.55;
/// How long the ring takes to close at a full push, and to open once let go (seconds).
const CLOSING: f32 = 0.2;
const OPENING: f32 = 0.35;

/// The ring's image at `strength` (0 open, 1 closed), premultiplied RGBA, `size` pixels square:
/// black, clear in the middle.
pub fn image(size: u32, strength: f32) -> Vec<u8> {
    let clear = OPEN + (CLOSED - OPEN) * strength.clamp(0.0, 1.0);
    let centre = (size as f32 - 1.0) * 0.5;
    let mut pixels = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let r = ((x as f32 - centre).hypot(y as f32 - centre) / centre) * HALF_TAN;
            let t = ((r - clear) / FEATHER).clamp(0.0, 1.0);
            let alpha = t * t * (3.0 - 2.0 * t);
            pixels.extend_from_slice(&[0, 0, 0, (alpha * 255.0).round() as u8]);
        }
    }
    pixels
}

/// How hard the sticks move the player now (0..=1): the left stick's push, and the right stick's
/// sideways push unless it snaps (`snap`: a snap turn is no smooth turn).
pub fn moving(pad: &Gamepad, snap: bool) -> f32 {
    let axis = |v: i16| f32::from(v) / 32767.0;
    let past = |push: f32| ((push - STICK_DEADZONE) / (1.0 - STICK_DEADZONE)).clamp(0.0, 1.0);
    let walk = past(axis(pad.thumb_lx).hypot(axis(pad.thumb_ly)));
    let turn = if snap { 0.0 } else { past(axis(pad.thumb_rx).abs()) };
    walk.max(turn)
}

/// The ring's strength after `seconds` heading for `target`: closing over [`CLOSING`], opening over
/// [`OPENING`].
pub fn eased(strength: f32, target: f32, seconds: f32) -> f32 {
    if target > strength {
        (strength + seconds / CLOSING).min(target)
    } else {
        (strength - seconds / OPENING).max(target)
    }
}

/// The level shown at `strength`: 0 is none.
pub fn level(strength: f32) -> usize {
    (strength.clamp(0.0, 1.0) * (LEVELS - 1) as f32).round() as usize
}

pub struct Tunnel<'s> {
    swapchain: Swapchain<'s>,
    /// The images by level (none at 0).
    images: Vec<ID3D11Texture2D>,
    strength: f32,
    shown: usize,
    at: Option<Instant>,
}

impl<'s> Tunnel<'s> {
    /// The ring's swapchain and images, in `format`.
    pub fn new(session: &'s Session<'_>, device: &ID3D11Device, format: DXGI_FORMAT) -> Result<Self, Box<dyn std::error::Error>> {
        let swapchain = session.create_swapchain(format.0 as i64, PIXELS, PIXELS)?;
        let images = (1..LEVELS)
            .map(|level| d3d::create_texture_with(device, &simple_texture_desc(PIXELS, PIXELS, format), &image(PIXELS, level as f32 / (LEVELS - 1) as f32)))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { swapchain, images, strength: 0.0, shown: 0, at: None })
    }

    /// One frame at `now` with the sticks moving the player `moving` (0..=1; 0 too while the game
    /// is not in 3D, or a menu has the pad): the ring eased toward it, its image refilled when its
    /// level changed.
    pub fn step(&mut self, context: &ID3D11DeviceContext, moving: f32, now: Instant) {
        let seconds = self.at.map_or(0.0, |at| now.duration_since(at).as_secs_f32().min(0.1));
        self.at = Some(now);
        self.strength = eased(self.strength, moving, seconds);
        let level = level(self.strength);
        if level != self.shown && level > 0 {
            let image = &self.images[level - 1];
            if self.swapchain.fill(|target| d3d::copy_texture(context, target, image)).is_err() {
                return;
            }
        }
        self.shown = level;
    }

    /// The ring's layer in `view` (the head's space), while it shows.
    pub fn layer<'a>(&'a self, view: &'a Space<'a>) -> Option<Layer<'a>> {
        (self.shown > 0).then(|| Layer::Overlay {
            space: view,
            swapchain: &self.swapchain,
            pose: Posef::new(monaka_core::math::IDENTITY, [0.0, 0.0, -DISTANCE]),
            size: Extent2Df { width: WIDTH, height: WIDTH },
        })
    }
}

#[cfg(test)]
mod tests;
