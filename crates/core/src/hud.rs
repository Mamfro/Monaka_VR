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
/// ([`crate::protocol::ChannelName::panel`]), on a fixed place on a hand.
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
mod tests {
    use super::*;

    const SQUARE: Frustum = Frustum { left: -1.0, right: 1.0, up: 1.0, down: -1.0 };
    /// A 16:9 field: square pixels in a 3840x2160 image.
    const WIDE: Frustum = Frustum { left: -16.0 / 9.0, right: 16.0 / 9.0, up: 1.0, down: -1.0 };

    /// The layer's centre in the image's clip units (x right, y up, the image -1..1).
    fn clip_centre(p: &Placement) -> [f32; 2] {
        let f = &p.frustum;
        [(2.0 * p.centre[0] - (f.right + f.left)) / (f.right - f.left), (2.0 * p.centre[1] - (f.up + f.down)) / (f.up - f.down)]
    }

    #[test]
    fn the_hud_shrinks_toward_straight_ahead_and_moves_toward_the_nose() {
        let base = Rect::sized(3840.0, 2160.0);
        let centred = Placement::new(WIDE, 0, 0.0, 2.0);
        assert_eq!(centred.rect(base, base.aspect(), 1.0), base);
        assert_eq!(centred.rect(base, base.aspect(), 0.5), Rect::new(960.0, 540.0, 1920.0, 1080.0));
        assert_eq!(clip_centre(&centred), [0.0, 0.0]);
        // A left eye reaching further out (left) than in, and further down than up: straight
        // ahead is right of and above the image centre (0.2 and 0.1 clip units), and the HUD goes there.
        let off = Placement::new(Frustum { left: -1.2 * WIDE.right, right: 0.8 * WIDE.right, up: 0.9, down: -1.1 }, 0, 0.0, 2.0);
        let [ax, ay] = clip_centre(&off);
        assert!((ax - 0.2).abs() < 1e-5 && (ay - 0.1).abs() < 1e-5, "{ax} {ay}");
        let rect = off.rect(base, base.aspect(), 0.5);
        assert!((rect.x - (960.0 + 384.0)).abs() < 1e-2 && (rect.y - (540.0 - 108.0)).abs() < 1e-2, "{rect:?}");
        // Eyes 64 mm apart, the HUD two metres away: the left eye's HUD moves right by 16 mm
        // over two metres (0.008 tangent, 17.28 pixels of 3840 across 3.56), the right eye's left.
        let left = Placement::new(WIDE, 0, 0.064, 2.0);
        let right = Placement::new(WIDE, 1, 0.064, 2.0);
        assert!((left.shift_pixels(3840.0) - 17.28).abs() < 1e-2 && (right.shift_pixels(3840.0) + 17.28).abs() < 1e-2);
        assert!((left.rect(base, base.aspect(), 0.5).x - 977.28).abs() < 1e-2);
        assert!((right.rect(base, base.aspect(), 0.5).x - 942.72).abs() < 1e-2);
        // At a 106 degree field (tangent 1.33 each side) and 64 mm this is 0.024 clip units over
        // the distance in metres, Dying Light 1's rule of thumb.
        let wide = Placement::new(Frustum { left: -1.3333, right: 1.3333, up: 1.0, down: -1.0 }, 0, 0.064, 2.0);
        assert!((clip_centre(&wide)[0] - 0.012).abs() < 1e-4);
    }

    #[test]
    fn non_square_pixels_keep_the_layers_shape_in_angle() {
        // A square field drawn 1920x2160: the layer (a 16:9 monitor's) is half the height
        // and 1.778 tangent units wide of the two across 1920 pixels.
        let rect = Placement::new(SQUARE, 0, 0.0, 2.0).rect(Rect::sized(1920.0, 2160.0), 16.0 / 9.0, 0.5);
        assert_eq!((rect.y, rect.height), (540.0, 1080.0));
        assert!((rect.width - 1920.0 * 1.7778 / 2.0).abs() < 0.1 && (rect.x - (1920.0 - rect.width) * 0.5).abs() < 1e-3);
    }

    #[test]
    fn a_box_becomes_the_panel_shape_about_its_centre() {
        // A 200x100 box into a square panel with no margin: 200x200 about the same centre.
        let rect = fit([100.0, 50.0, 300.0, 150.0], 0.0, (512, 512));
        assert_eq!(rect, [100.0, 0.0, 200.0, 200.0]);
        // Twice as wide as tall, with a margin of a tenth of the larger side on each side.
        let rect = fit([0.0, 0.0, 100.0, 100.0], 0.1, (768, 384));
        assert!((rect[2] / rect[3] - 2.0).abs() < 1e-5 && rect[3] == 120.0 && rect[0] + rect[2] * 0.5 == 50.0, "{rect:?}");
        assert_eq!(union([0.0, 0.0, 1.0, 1.0], [-1.0, 0.5, 0.5, 2.0]), [-1.0, 0.0, 1.0, 2.0]);
    }
}
