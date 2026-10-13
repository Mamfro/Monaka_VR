//! The layer crops against the Chrome Engine games' own crops they were lifted from: a piece's box
//! in the layer (as the gui tree reads it) gives the same rectangle of the layer for its panel, at
//! the same margin and panel sizes, whatever the layer's size.

use super::*;

/// The old crops' margin: a piece's box grown by this share of its larger side on each side.
const OLD_MARGIN: f32 = 0.02;

/// The old crops' rectangle for a box `b` in layer pixels and a panel of `size`, as it was
/// computed (the widget's box taken as it is, fitted to the panel's shape).
fn old(b: [f32; 4], size: (u32, u32)) -> [f32; 4] {
    monaka_core::hud::fit(b, OLD_MARGIN, size)
}

/// The rectangle the ready-made crops for box `b` shown alone in a layer of `layer`.
fn new(b: [f32; 4], layer: (u32, u32), size: (u32, u32)) -> [f32; 4] {
    let layout = (layer.0 as f32, layer.1 as f32);
    pixels(rect_of(&fraction(b, layout), layout, size, OLD_MARGIN), layer)
}

#[test]
fn a_box_gives_the_rectangle_the_old_crops_gave() {
    // Boxes the gui tree read in play (3840x2160 frames), and boxes across a square eye.
    let boxes = [
        [1488.0, 74.0, 2352.0, 250.0],
        [1488.0, 106.0, 2352.0, 282.0],
        [22.0, 1728.0, 817.0, 2089.0],
        [-104.0, 375.0, 494.0, 626.0],
        [82.0, 123.0, 1217.0, 276.0],
        [1606.0, 1537.0, 2234.0, 1571.0],
        [126.0, 1610.0, 446.0, 1790.0],
    ];
    // The panel sizes both Chrome games gave their pieces, old and new.
    let sizes = [(1280, 96), (896, 192), (768, 384), (1024, 352), (1280, 352), (1152, 480), (768, 256), (1152, 160), (768, 96), (384, 384)];
    for layer in [(3840, 2160), (2016, 2240), (2880, 2880)] {
        for b in boxes {
            for size in sizes {
                let (was, now) = (old(b, size), new(b, layer, size));
                let close = was.iter().zip(now).all(|(a, b)| (a - b).abs() < 2e-3 * a.abs().max(1.0));
                assert!(close, "{b:?} in {layer:?} onto {size:?}: {now:?} against {was:?}");
            }
        }
    }
}
