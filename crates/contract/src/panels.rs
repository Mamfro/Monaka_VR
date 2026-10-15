//! Where the viewer puts a game's HUD pieces on the hands: the producer declares each piece it
//! publishes on its own channel (`<channel>-panel-<name>`) with its place ([`Place`]: the hand, its
//! side, its width, beside another), and the viewer lays out what it is told. The block lives beside
//! the stereo channel; the producer writes it once when its dynamic HUD starts.

use crate::protocol::{LEFT_HAND, RIGHT_HAND, Stamp, Versioned};

pub const PANELS_MAGIC: u32 = 0x4c4e_4150;
pub const PANELS_MAJOR: u16 = 2;
/// The most pieces a game declares.
pub const PANEL_PIECES: usize = 8;
/// The longest piece name, in bytes.
pub const PANEL_NAME_LEN: usize = 24;

/// Which side of a hand a panel lies on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// On the back of the hand, a little out from the handle.
    BackOfHand,
    /// Just above the palm (the fingers curl round the handle there).
    Palm,
}

/// Where one piece's panel goes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Place {
    /// [`LEFT_HAND`] or [`RIGHT_HAND`].
    pub hand: usize,
    pub side: Side,
    /// Its width in metres (a strip is widened to a least height by the viewer).
    pub width: f32,
    /// Left of the hand's main panel on the same side (the first declared there, not beside), their
    /// tops level: a wide list of text beside a map.
    pub beside: bool,
}

/// One declared piece as the block holds it.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PanelPlace {
    /// The piece's name, zero-padded UTF-8.
    pub name: [u8; PANEL_NAME_LEN],
    pub hand: u32,
    /// 0 the back of the hand, 1 the palm.
    pub side: u32,
    pub width: f32,
    pub beside: u32,
}

impl PanelPlace {
    /// Piece `name` at `place`; `None` for a name too long or a hand that is not one.
    pub fn new(name: &str, place: Place) -> Option<Self> {
        if name.is_empty() || name.len() > PANEL_NAME_LEN || place.hand > RIGHT_HAND || !(place.width > 0.0) {
            return None;
        }
        let mut bytes = [0; PANEL_NAME_LEN];
        bytes[..name.len()].copy_from_slice(name.as_bytes());
        let side = match place.side {
            Side::BackOfHand => 0,
            Side::Palm => 1,
        };
        Some(Self { name: bytes, hand: place.hand as u32, side, width: place.width, beside: place.beside as u32 })
    }

    /// The piece's name and place, if the record holds a sound one.
    pub fn get(&self) -> Option<(&str, Place)> {
        let length = self.name.iter().position(|&b| b == 0).unwrap_or(PANEL_NAME_LEN);
        let name = std::str::from_utf8(&self.name[..length]).ok().filter(|n| !n.is_empty())?;
        let hand = match self.hand as usize {
            hand @ (LEFT_HAND | RIGHT_HAND) => hand,
            _ => return None,
        };
        let side = match self.side {
            0 => Side::BackOfHand,
            1 => Side::Palm,
            _ => return None,
        };
        (self.width > 0.0 && self.width.is_finite()).then_some((name, Place { hand, side, width: self.width, beside: self.beside != 0 }))
    }
}

/// The pieces a producer declared.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PanelsBlock {
    pub stamp: Stamp,
    pub count: u32,
    pub _reserved: u32,
    pub pieces: [PanelPlace; PANEL_PIECES],
}

impl PanelsBlock {
    /// Declares `pieces` (the first [`PANEL_PIECES`]; a piece that cannot be held is left out).
    /// How many were declared.
    pub fn declare<'a>(&mut self, pieces: impl IntoIterator<Item = (&'a str, Place)>) -> usize {
        let held: Vec<PanelPlace> = pieces.into_iter().filter_map(|(name, place)| PanelPlace::new(name, place)).take(PANEL_PIECES).collect();
        self.pieces = [PanelPlace::default(); PANEL_PIECES];
        self.pieces[..held.len()].copy_from_slice(&held);
        self.count = held.len() as u32;
        held.len()
    }

    /// The declared pieces, in order.
    pub fn pieces(&self) -> impl Iterator<Item = (&str, Place)> {
        self.pieces.iter().take((self.count as usize).min(PANEL_PIECES)).filter_map(PanelPlace::get)
    }
}

impl Versioned for PanelsBlock {
    const MAGIC: u32 = PANELS_MAGIC;
    const MAJOR: u16 = PANELS_MAJOR;
    const MINOR: u16 = 0;
    const WHAT: &'static str = "HUD panels block";
    const CAPACITY: usize = 4 * 1024;

    fn stamp(&self) -> Stamp {
        self.stamp
    }

    fn start(&mut self) {
        *self = Self { stamp: Stamp::of::<Self>(), ..Self::default() };
    }
}

#[cfg(test)]
mod tests;
