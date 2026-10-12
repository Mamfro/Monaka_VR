//! The dynamic HUD ready-made (`Feature::HudPanels`, the `world_hud` option): HUD pieces (the
//! minimap, a gun's ammo, ...) taken out of the HUD in front of the eyes and put on the hands,
//! each on its own panel (`<channel>-panel-<piece>`, which the viewer places by the piece's name;
//! [`crate::panel11`]).
//!
//! Method: draw-level. While the game draws its HUD, its HUD pieces provider says which draws are
//! which piece's ([`d3d11::draw`], [`gl::draw`]) and where each piece lies in the HUD's layout
//! ([`show`], [`hide`]); this does the rest. A piece's box grows while it shows and is made its
//! panel's shape; at the frame's first piece draw every shown piece's panel starts empty; a
//! piece's draws run into its panel, onto black and onto white, with the layout scaled and moved
//! so the box fills it, and into no eye; once a frame every shown panel is resolved and
//! published, and one no longer shown is published cleared, once. A piece not shown is drawn as
//! the game draws it. The panels are drawn with the game's own graphics API: D3D11 on the game's
//! device ([`d3d11`]), OpenGL into canvases of our own captured into D3D11 ([`gl`]).
//!
//! A game lists the ready-made with its own install (which calls [`install`], then hooks where
//! its HUD is drawn) by struct update: `Component { install: install_panels, ..hud_panels() }`.

pub mod d3d11;
pub mod gl;

use monaka_channel::ChannelName;
use monaka_core::feature::Feature;
use monaka_core::hud::{LAYOUT_FRESH_MS, fit, union};
use monaka_core::manifest::WORLD_HUD;
use monaka_core::sync::Locked;
use monaka_producer::game::{Component, Game, Lack, On, Start};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering::*};
use std::time::Instant;

/// A piece a game puts on a hand: the viewer's name for it (where it goes: `minimap`, `weapon`,
/// `weapon_left`, ...) and its panel's size in pixels (the piece's box is made this shape).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Piece {
    pub name: &'static str,
    pub size: (u32, u32),
}

/// A game whose HUD pieces go on the hands. Its pieces are named by their index in [`PIECES`]
/// in every call here.
///
/// [`PIECES`]: HudPieces::PIECES
pub trait HudPieces: Game {
    const PIECES: &'static [Piece];
    /// The game masks its HUD's draws through the stencil (a bar's fill): its D3D11 panels get
    /// depth-stencils of their own, so the masks hold on the hands.
    const MASKED: bool = false;
}

/// A piece's box grown by this share of its larger side on each side for its panel.
pub const MARGIN: f32 = 0.06;

/// The dynamic HUD for game `G`, on with `world_hud`.
pub const fn hud_panels<G: HudPieces>() -> Component<G> {
    Component {
        name: "hud panels",
        features: &[Feature::HudPanels],
        options: &[WORLD_HUD],
        needs_channel: true,
        on: |run| On::when(run.shared.hud.world, "the dynamic HUD is off"),
        install: install::<G>,
        arm,
        quiet,
        release,
        ..Component::NONE
    }
}

/// Where a shown piece lies: its box (left, top, right, bottom, as fractions of the layout),
/// grown while it shows; the layout's size it was measured in; when it was last reported.
#[derive(Clone, Copy, Debug)]
struct Shown {
    found: [f32; 4],
    layout: (f32, f32),
    seen: Instant,
}

struct State {
    channel: ChannelName,
    pieces: &'static [Piece],
    masked: bool,
    shown: Vec<Option<Shown>>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
/// Pieces are taken out of the HUD (armed, not yet quiet).
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// The ready-made's part of the component's install: the run's channel and the game's pieces.
pub fn install<G: HudPieces>(start: &mut Start<'_, G>) -> Result<(), Lack> {
    let channel = start.channel()?.clone();
    *STATE.locked() = Some(State { channel, pieces: G::PIECES, masked: G::MASKED, shown: vec![None; G::PIECES.len()] });
    Ok(())
}

fn arm() {
    ACTIVE.store(true, Release);
}

fn quiet() {
    ACTIVE.store(false, Release);
}

/// The hooks are out: the panels go (their channels close; the viewer takes them off the hands).
fn release() {
    d3d11::release();
    gl::release();
    STATE.locked().take();
}

/// Whether pieces are taken out of the HUD now.
pub fn active() -> bool {
    ACTIVE.load(Acquire)
}

/// Piece `piece` shows, drawn within `bounds` (left, top, right, bottom) of a layout of `layout`
/// (its width and height, the same units, `bounds` from its top-left corner): its box grows to hold
/// them (afresh in a new layout or after it was gone).
pub fn show(piece: usize, bounds: [f32; 4], layout: (f32, f32)) {
    if !(layout.0 > 0.0 && layout.1 > 0.0 && bounds.iter().all(|v| v.is_finite())) {
        return;
    }
    let fraction = [bounds[0] / layout.0, bounds[1] / layout.1, bounds[2] / layout.0, bounds[3] / layout.1];
    let mut state = STATE.locked();
    let Some(slot) = state.as_mut().and_then(|s| s.shown.get_mut(piece)) else { return };
    let found = match *slot {
        Some(shown) if shown.layout == layout && fresh(&shown) => union(shown.found, fraction),
        _ => fraction,
    };
    *slot = Some(Shown { found, layout, seen: Instant::now() });
}

/// Piece `piece`, while it shows, also takes in `bounds` (a draw of it found reaching past its box):
/// [`show`] for a piece already shown.
pub fn grow(piece: usize, bounds: [f32; 4], layout: (f32, f32)) {
    let shown = STATE.locked().as_ref().and_then(|s| s.shown.get(piece).copied().flatten()).is_some_and(|s| s.layout == layout && fresh(&s));
    if shown {
        show(piece, bounds, layout);
    }
}

/// Piece `piece` is not drawn now (a menu, nothing to show): its draws go as the game makes them.
pub fn hide(piece: usize) {
    if let Some(slot) = STATE.locked().as_mut().and_then(|s| s.shown.get_mut(piece)) {
        *slot = None;
    }
}

fn fresh(shown: &Shown) -> bool {
    shown.seen.elapsed().as_millis() < u128::from(LAYOUT_FRESH_MS)
}

/// The rectangle of the layout piece `piece`'s panel shows (x, y, width, height as fractions of the
/// layout), while it shows.
fn rect(state: &State, piece: usize) -> Option<[f32; 4]> {
    let shown = state.shown.get(piece)?.filter(fresh)?;
    Some(rect_of(&shown.found, shown.layout, state.pieces[piece].size))
}

/// A box (fractions of a layout of `layout`) grown by [`MARGIN`] and made the panel's shape, as a
/// rectangle of fractions.
fn rect_of(found: &[f32; 4], (w, h): (f32, f32), size: (u32, u32)) -> [f32; 4] {
    let r = fit([found[0] * w, found[1] * h, found[2] * w, found[3] * h], MARGIN, size);
    [r[0] / w, r[1] / h, r[2] / w, r[3] / h]
}

/// What a frame's piece draws need: the run's channel, the pieces, and each one's rectangle while
/// it shows.
struct Frame {
    channel: ChannelName,
    pieces: &'static [Piece],
    masked: bool,
    rects: Vec<Option<[f32; 4]>>,
}

fn frame() -> Option<Frame> {
    let state = STATE.locked();
    let state = state.as_ref()?;
    Some(Frame { channel: state.channel.clone(), pieces: state.pieces, masked: state.masked, rects: (0..state.pieces.len()).map(|i| rect(state, i)).collect() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_box_becomes_the_panels_shape() {
        let rect = rect_of(&[0.846, 0.710, 0.921, 0.749], (2644.0, 2644.0), (768, 384));
        assert!((rect[2] / rect[3] - 2.0).abs() < 1e-4, "{rect:?}");
        assert!((rect[0] + rect[2] / 2.0 - 0.8835).abs() < 1e-4 && (rect[1] + rect[3] / 2.0 - 0.7295).abs() < 1e-4);
        let rect = rect_of(&[0.80, 0.85, 0.95, 0.95], (1920.0, 1080.0), (768, 384));
        assert!(((rect[2] * 1920.0) / (rect[3] * 1080.0) - 2.0).abs() < 1e-3, "{rect:?}");
    }

    #[test]
    fn a_shown_piece_grows_and_a_hidden_one_starts_afresh() {
        const PIECES: &[Piece] = &[Piece { name: "weapon", size: (768, 384) }];
        let channel = ChannelName::new("monaka-hud-panels-test".to_string()).unwrap();
        *STATE.locked() = Some(State { channel, pieces: PIECES, masked: false, shown: vec![None] });
        show(0, [80.0, 70.0, 90.0, 75.0], (100.0, 100.0));
        show(0, [85.0, 72.0, 95.0, 74.0], (100.0, 100.0));
        let found = |s: &Option<State>| s.as_ref().unwrap().shown[0].unwrap().found;
        assert_eq!(found(&STATE.locked()), [0.8, 0.7, 0.95, 0.75]);
        assert!(frame().unwrap().rects[0].is_some());
        hide(0);
        assert!(frame().unwrap().rects[0].is_none());
        show(0, [85.0, 72.0, 95.0, 74.0], (100.0, 100.0));
        assert_eq!(found(&STATE.locked()), [0.85, 0.72, 0.95, 0.74]);
        // Another layout size: measured afresh.
        show(0, [10.0, 10.0, 20.0, 20.0], (200.0, 200.0));
        assert_eq!(found(&STATE.locked()), [0.05, 0.05, 0.1, 0.1]);
        STATE.locked().take();
    }
}
