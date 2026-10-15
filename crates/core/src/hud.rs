//! The HUD in the headset, the arithmetic every producer shares.
//!
//! **Laying the HUD over an eye.** A game draws its HUD as a flat layer for a monitor. In the
//! headset that layer floats a set distance straight ahead of the head ([`Placement`]): each
//! eye sees it at that eye's straight-ahead point moved toward the nose by half the eye
//! separation over the distance (so both eyes converge on it there), its height a share of the
//! eye's vertical field (`hud_scale`), its own shape kept in angle. Whatever the engine
//! (a viewport moved around the game's own HUD draws, a compute pass laying a captured layer
//! over the eye image) and whatever the image's frustum, the rectangle comes from here;
//! [`HLSL`] holds the shader side (the layer pixel an eye pixel shows, the over operators) for
//! the compute passes.
//!
//! **Pieces on the hands** (the dynamic HUD: the minimap, quests, weapon, health and tool on
//! the hands, [`PIECES`]): the arithmetic a producer needs to show a piece's box on a panel of a
//! fixed shape. Boxes and rectangles are in pixels of whatever image the pieces are cut from.

use crate::camera::Frustum;

/// The HUD lay-over's shader functions, put before a compute shader's own source
/// ([`with_hlsl`]).
pub const HLSL: &str = include_str!("hud.hlsl");

/// `body` with the shared HUD functions before it, for the shader compiler.
pub fn with_hlsl(body: &str) -> String {
    format!("{HLSL}\n{body}")
}

/// The pieces of the dynamic HUD, in the viewer's order: each a channel of its own
/// ([`monaka_contract::protocol::ChannelName::panel`]), on a fixed place on a hand.
pub const PIECES: [&str; 5] = ["minimap", "weapon", "quests", "health", "tool"];

/// A HUD layout read longer ago than this (ms) is stale: the game stopped laying out its HUD.
pub const LAYOUT_FRESH_MS: u64 = 500;

/// A rectangle in pixels, x and y from the top left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self { x, y, width, height }
    }

    /// `width` x `height` at the origin.
    pub fn sized(width: f32, height: f32) -> Self {
        Self::new(0.0, 0.0, width, height)
    }

    pub fn array(self) -> [f32; 4] {
        [self.x, self.y, self.width, self.height]
    }

    pub fn centre(self) -> [f32; 2] {
        [self.x + self.width * 0.5, self.y + self.height * 0.5]
    }

    /// Width over height.
    pub fn aspect(self) -> f32 {
        self.width / self.height.max(1e-6)
    }
}

/// Where the HUD floats in front of one eye: in the eye's image, drawn with `frustum`, the
/// layer's centre lies at the straight-ahead point moved toward the nose so that both eyes see
/// it `distance` metres away.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub frustum: Frustum,
    /// The layer's centre in tangent units (x right, y up; straight ahead is the origin).
    pub centre: [f32; 2],
}

impl Placement {
    /// For eye `eye` (0 left, 1 right), eyes `ipd` metres apart, the HUD `distance` metres away.
    pub fn new(frustum: Frustum, eye: usize, ipd: f32, distance: f32) -> Self {
        let toward_nose = if eye == 0 { 1.0 } else { -1.0 };
        Self { frustum, centre: [toward_nose * 0.5 * ipd / distance.max(0.05), 0.0] }
    }

    /// Sideways pixels the layer's centre lies from straight ahead in an image `width` wide
    /// (the HUD's disparity, signed for this eye).
    pub fn shift_pixels(&self, width: f32) -> f32 {
        self.centre[0] / (self.frustum.right - self.frustum.left) * width
    }

    /// The layer's rectangle in `image` (the eye's image, or the viewport its HUD is drawn
    /// into): `aspect` (the layer's width over its height, as on a monitor) wide for its
    /// height, `scale` of the eye's vertical field high, about the centre. With non-square
    /// pixels the layer keeps its shape in angle, as the world does.
    pub fn rect(&self, image: Rect, aspect: f32, scale: f32) -> Rect {
        let f = &self.frustum;
        let (span_x, span_y) = (f.right - f.left, f.up - f.down);
        let (px, py) = (image.width / span_x, image.height / span_y);
        let height = scale * span_y;
        let width = height * aspect;
        let [cx, cy] = self.centre;
        Rect { x: image.x + (cx - width * 0.5 - f.left) * px, y: image.y + (f.up - cy - height * 0.5) * py, width: width * px, height: height * py }
    }
}

/// The box holding both (left, top, right, bottom).
pub fn union(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]
}

/// The rectangle (x, y, width, height) a panel of `panel` pixels shows for the box `b` (left,
/// top, right, bottom): the box grown by `margin` of its larger side on each side, then widened
/// or heightened about its centre to the panel's shape.
pub fn fit(b: [f32; 4], margin: f32, panel: (u32, u32)) -> [f32; 4] {
    let (l, t, r, bottom) = (b[0], b[1], b[2], b[3]);
    let grow = (r - l).max(bottom - t) * margin;
    let (mut w, mut h) = (r - l + 2.0 * grow, bottom - t + 2.0 * grow);
    let aspect = panel.0 as f32 / panel.1.max(1) as f32;
    if w / h.max(1e-6) < aspect {
        w = h * aspect;
    } else {
        h = w / aspect;
    }
    let (cx, cy) = ((l + r) * 0.5, (t + bottom) * 0.5);
    [cx - w * 0.5, cy - h * 0.5, w, h]
}

#[cfg(test)]
mod tests;
