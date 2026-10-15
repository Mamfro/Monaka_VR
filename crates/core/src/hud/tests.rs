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
