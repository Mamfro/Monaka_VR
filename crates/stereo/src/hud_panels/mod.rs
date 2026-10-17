//! The dynamic HUD ready-made (`Feature::HudPanels`, the `world_hud` option): HUD pieces (the
//! minimap, a gun's ammo, ...) taken out of the HUD in front of the eyes and put on the hands,
//! each on its own panel (`<channel>-panel-<piece>`, which the viewer places by the piece's name;
//! [`crate::hud_panels::panel`]).
//!
//! Two methods, one set of pieces. Draw-level: while the game draws its HUD, its HUD pieces
//! provider says which draws are which piece's ([`d3d11::draw`], [`gl::draw`]) and where each
//! piece lies in the HUD's layout ([`show`], [`hide`]); this does the rest. A piece's box grows
//! while it shows and is made its panel's shape; at the frame's first piece draw every shown
//! piece's panel starts empty; a piece's draws run into its panel, onto black and onto white, with
//! the layout scaled and moved so the box fills it, and into no eye; once a frame every shown panel
//! is resolved and published, and one no longer shown is published cleared, once. A piece not shown
//! is drawn as the game draws it. The panels are drawn with the game's own graphics API: D3D11 on
//! the game's device ([`d3d11`]), OpenGL into canvases of our own captured into D3D11 ([`gl`]).
//!
//! Layer crops, for a D3D12 game whose HUD is a layer of its own ([`LayerPieces`], [`d3d12`]): at
//! each present the game says where each piece lies in the layer, the boxes grow and take their
//! panels' shape as above, and each shown piece's rectangle of the layer is scaled into its panel
//! and published; the rectangles are left out of the HUD laid over the eyes, or taken out of an
//! eye frame that has the HUD drawn in ([`d3d12::take_out`]).
//!
//! A game lists the ready-made with its own install (which calls [`install`], or
//! [`install_layer`] for layer crops, then hooks where its HUD is drawn) by struct update:
//! `Component { install: install_panels, ..hud_panels() }`.

pub mod d3d11;
pub mod d3d12;
pub mod gl;
pub mod panel;
#[cfg(test)]
mod tests;

use monaka_channel::ChannelName;
use monaka_channel::panels::PanelsChannel;
pub use monaka_contract::panels::{PANEL_PIECES, Place, Side};
use monaka_producer::log;
use monaka_core::feature::Feature;
use monaka_core::hud::{LAYOUT_FRESH_MS, fit, union};
use monaka_contract::manifest::WORLD_HUD;
use monaka_core::sync::Locked;
use monaka_producer::game::{Component, Game, Lack, On, Start};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering::*};
use std::time::Instant;

/// A piece a game puts on a hand: its name (its channel's, `<channel>-panel-<name>`), its panel's
/// size in pixels (the piece's box is made this shape), and where the viewer puts it ([`places`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Piece {
    pub name: &'static str,
    pub size: (u32, u32),
    pub place: Place,
}

/// Where the pieces games share go: the map on the back of the right hand with the quest list
/// beside it, the weapon above the right palm, health on the back of the left hand, the quick item
/// (one icon) and a second gun above the left palm.
pub mod places {
    use monaka_contract::panels::{Place, Side};
    use monaka_contract::protocol::{LEFT_HAND, RIGHT_HAND};

    /// A panel about a hand's width (metres); the quests (a wide list of text) wider, one icon
    /// smaller.
    const HAND_WIDTH: f32 = 0.08;
    const LIST_WIDTH: f32 = 0.20;
    const ICON_WIDTH: f32 = 0.05;

    pub const MINIMAP: Place = Place { hand: RIGHT_HAND, side: Side::BackOfHand, width: HAND_WIDTH, beside: false };
    pub const QUESTS: Place = Place { hand: RIGHT_HAND, side: Side::BackOfHand, width: LIST_WIDTH, beside: true };
    pub const WEAPON: Place = Place { hand: RIGHT_HAND, side: Side::Palm, width: HAND_WIDTH, beside: false };
    pub const HEALTH: Place = Place { hand: LEFT_HAND, side: Side::BackOfHand, width: HAND_WIDTH, beside: false };
    pub const TOOL: Place = Place { hand: LEFT_HAND, side: Side::Palm, width: ICON_WIDTH, beside: false };
    pub const WEAPON_LEFT: Place = Place { hand: LEFT_HAND, side: Side::Palm, width: HAND_WIDTH, beside: false };
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
    /// A piece's box grown by this share of its larger side on each side for its panel (smaller
    /// where a long thin piece has another close by).
    const MARGIN: f32 = MARGIN;
}

/// A game whose pieces are cut out of its HUD layer ([`d3d12`]): where each lies in the layer.
pub trait LayerPieces: HudPieces {
    /// The layer's alpha is how much of what is behind shows through (1 where nothing is drawn, as
    /// id Tech 6 keeps its GUI), not its coverage.
    const SEE_THROUGH: bool = false;
    /// Each piece's box this frame, by its index in `PIECES` (left, top, right, bottom in the
    /// layer's pixels); `None` for a piece not shown. Asked at each present before the crops.
    fn boxes(boxes: &mut [Option<[f32; 4]>]);
}

/// A piece's box grown by this share of its larger side on each side for its panel, unless the
/// game says otherwise ([`HudPieces::MARGIN`]).
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

/// Where a layer-crops game's pieces lie in its layer ([`LayerPieces::boxes`]).
type Boxes = fn(&mut [Option<[f32; 4]>]);

struct State {
    channel: Arc<ChannelName>,
    pieces: &'static [Piece],
    masked: bool,
    margin: f32,
    /// The game's boxes, for layer crops, and whether its layer's alpha is what shows through.
    boxes: Option<Boxes>,
    see_through: bool,
    shown: Vec<Option<Shown>>,
    /// The pieces declared to the viewer, kept open while the panels run.
    _declared: Option<PanelsChannel>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
/// Pieces are taken out of the HUD (armed, not yet quiet).
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// The ready-made's part of the component's install: the run's channel and the game's pieces.
pub fn install<G: HudPieces>(start: &mut Start<'_, G>) -> Result<(), Lack> {
    let channel = Arc::new(start.channel()?.clone());
    let declared = match PanelsChannel::open(&channel) {
        Ok(mut block) => {
            let count = block.declare(G::PIECES.iter().map(|p| (p.name, p.place)));
            if count < G::PIECES.len() {
                log!("HUD panels: {} of {} pieces declared to the viewer", count, G::PIECES.len());
            }
            Some(block)
        }
        Err(e) => {
            log!("HUD panels: the viewer cannot be told the pieces ({e}); none go on the hands");
            None
        }
    };
    let shown = vec![None; G::PIECES.len()];
    *STATE.locked() = Some(State { channel, pieces: G::PIECES, masked: G::MASKED, margin: G::MARGIN, boxes: None, see_through: false, shown, _declared: declared });
    Ok(())
}

/// [`install`] for a game whose pieces are cut out of its HUD layer: its boxes are asked for at
/// each present ([`d3d12::publish`]).
pub fn install_layer<G: LayerPieces>(start: &mut Start<'_, G>) -> Result<(), Lack> {
    install::<G>(start)?;
    if let Some(state) = STATE.locked().as_mut() {
        state.boxes = Some(G::boxes);
        state.see_through = G::SEE_THROUGH;
    }
    Ok(())
}

/// A layer-crops game's boxes this frame, in a layer of `size`: each piece shown there or hidden.
fn take_boxes(size: (u32, u32)) {
    let Some((boxes, count)) = STATE.locked().as_ref().and_then(|s| Some((s.boxes?, s.pieces.len()))) else { return };
    let mut found = vec![None; count];
    boxes(&mut found);
    let layout = (size.0 as f32, size.1 as f32);
    for (piece, found) in found.into_iter().enumerate() {
        match found {
            Some(bounds) => show(piece, bounds, layout),
            None => hide(piece),
        }
    }
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
    d3d12::release();
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
    let fraction = fraction(bounds, layout);
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

/// A box (left, top, right, bottom) of a layout of `layout` as fractions of it.
fn fraction(bounds: [f32; 4], layout: (f32, f32)) -> [f32; 4] {
    [bounds[0] / layout.0, bounds[1] / layout.1, bounds[2] / layout.0, bounds[3] / layout.1]
}

/// The rectangle of the layout piece `piece`'s panel shows (x, y, width, height as fractions of the
/// layout), while it shows.
fn rect(state: &State, piece: usize) -> Option<[f32; 4]> {
    let shown = state.shown.get(piece)?.filter(fresh)?;
    Some(rect_of(&shown.found, shown.layout, state.pieces[piece].size, state.margin))
}

/// A box (fractions of a layout of `layout`) grown by `margin` and made the panel's shape, as a
/// rectangle of fractions.
fn rect_of(found: &[f32; 4], (w, h): (f32, f32), size: (u32, u32), margin: f32) -> [f32; 4] {
    let r = fit([found[0] * w, found[1] * h, found[2] * w, found[3] * h], margin, size);
    [r[0] / w, r[1] / h, r[2] / w, r[3] / h]
}

/// What a frame's piece draws need: the run's channel, the pieces, and each one's rectangle while
/// it shows (the first [`PANEL_PIECES`], the ones the viewer is told of; asked at every present,
/// so nothing here allocates).
struct Frame {
    channel: Arc<ChannelName>,
    pieces: &'static [Piece],
    masked: bool,
    see_through: bool,
    rects: [Option<[f32; 4]>; PANEL_PIECES],
}

fn frame() -> Option<Frame> {
    let state = STATE.locked();
    let state = state.as_ref()?;
    let rects = std::array::from_fn(|i| if i < state.pieces.len() { rect(state, i) } else { None });
    Some(Frame { channel: state.channel.clone(), pieces: state.pieces, masked: state.masked, see_through: state.see_through, rects })
}

/// A rectangle of fractions of a layout of `size` in its pixels (x, y, width, height).
fn pixels(rect: [f32; 4], size: (u32, u32)) -> [f32; 4] {
    let (w, h) = (size.0 as f32, size.1 as f32);
    [rect[0] * w, rect[1] * h, rect[2] * w, rect[3] * h]
}

/// Logs piece `name`'s panel changing to `out`.
fn note_change(name: &str, out: PanelOut) {
    monaka_producer::log!("HUD panel {name}: {}", if out == PanelOut::Shown { "drawn: on the hand" } else { "gone: cleared" });
}

/// The shown pieces' rectangles at a present (x, y, width, height in the layer's pixels), at most
/// [`PANEL_PIECES`]; read as a slice.
#[derive(Clone, Copy, Debug, Default)]
pub struct Rects {
    list: [[f32; 4]; PANEL_PIECES],
    len: usize,
}

impl Rects {
    fn of(rects: [Option<[f32; 4]>; PANEL_PIECES]) -> Self {
        let mut made = Self::default();
        for rect in rects.into_iter().flatten() {
            made.list[made.len] = rect;
            made.len += 1;
        }
        made
    }
}

impl std::ops::Deref for Rects {
    type Target = [[f32; 4]];

    fn deref(&self) -> &[[f32; 4]] {
        &self.list[..self.len]
    }
}

/// What goes out of a piece's panel at a frame's end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelOut {
    /// The piece as drawn this frame.
    Shown,
    /// Empty: the piece stopped showing.
    Cleared,
}

/// The panels' states across frames, the same for every backend: which pieces' panels were made
/// ready this frame (cleared at its first piece draw, [`PanelStates::start`]), and which went out
/// shown last. A panel goes out every frame its piece shows, and once, cleared, when it stops; a
/// panel whose clear did not go out is cleared again the next frame.
#[derive(Debug, Default)]
pub struct PanelStates {
    started: bool,
    ready: Vec<bool>,
    published_shown: Vec<bool>,
}

impl PanelStates {
    /// The frame's first piece draw, of `pieces` pieces: true the first time each frame, when the
    /// caller readies each shown piece's panel ([`PanelStates::ready`]).
    pub fn start(&mut self, pieces: usize) -> bool {
        self.ready.resize(pieces, false);
        self.published_shown.resize(pieces, false);
        !std::mem::replace(&mut self.started, true)
    }

    /// Piece `piece`'s panel is cleared for this frame's draws.
    pub fn ready(&mut self, piece: usize) {
        if let Some(ready) = self.ready.get_mut(piece) {
            *ready = true;
        }
    }

    /// Whether piece `piece`'s draws go into its panel this frame.
    pub fn is_ready(&self, piece: usize) -> bool {
        self.ready.get(piece).copied().unwrap_or(false)
    }

    /// What goes out of piece `piece`'s panel, `shown` or not this frame (none: nothing).
    pub fn out(&self, piece: usize, shown: bool) -> Option<PanelOut> {
        match (shown, self.published_shown.get(piece).copied().unwrap_or(false)) {
            (true, _) => Some(PanelOut::Shown),
            (false, true) => Some(PanelOut::Cleared),
            (false, false) => None,
        }
    }

    /// The frame's end for a draw-level backend: by piece, what goes out (the pieces readied this
    /// frame shown), and the frame's marks gone.
    pub fn end(&mut self) -> Vec<Option<PanelOut>> {
        self.started = false;
        let ready = std::mem::take(&mut self.ready);
        let outs = ready.iter().enumerate().map(|(piece, &shown)| self.out(piece, shown)).collect();
        self.ready = vec![false; ready.len()];
        outs
    }

    /// `out` of piece `piece`'s panel went out. Whether the piece changed between shown and gone
    /// (worth a log line).
    pub fn went_out(&mut self, piece: usize, out: PanelOut) -> bool {
        if piece >= self.published_shown.len() {
            self.published_shown.resize(piece + 1, false);
        }
        let shown = out == PanelOut::Shown;
        std::mem::replace(&mut self.published_shown[piece], shown) != shown
    }
}
