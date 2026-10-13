//! The lay-over against the code it was lifted from (The Beast's present request): for every frame
//! (the HUD kept out of the frame or drawn in, a menu or play, a place over the eye or none, pieces
//! on the hands or none) the eye gets the same step, and a laid-over layer the same params.

use super::present12::{Step, over_params, step};
use crate::hudover12::Params;
use crate::providers::whole_frame;

/// What the old present request did with the eye: the lay-over's params, the pieces taken out, or
/// nothing.
#[derive(Debug)]
enum Old {
    Lay(Params),
    PiecesOut,
    Nothing,
}

/// The old present request's branch, copied as it was (`redirecting` the HUD layer's, `over` the
/// game's place for the frame about to present, `rects` the panels' rectangles).
fn old(redirecting: bool, menu: bool, over: Option<Params>, rects: &[[f32; 4]], (width, height): (u32, u32)) -> Old {
    if redirecting {
        let whole = whole_frame((width, height));
        let over = if menu { whole } else { over.unwrap_or(whole).leaving_out(rects) };
        Old::Lay(over)
    } else if !menu {
        Old::PiecesOut
    } else {
        Old::Nothing
    }
}

fn same(a: &Params, b: &Params) -> bool {
    a.rect == b.rect && a.aim == b.aim && a.cross_radius == b.cross_radius && a.piece_count == b.piece_count && a.pieces == b.pieces
}

#[test]
fn every_frame_gets_what_the_present_request_gave_it() {
    let size = (2016, 2240);
    let placed = Params { rect: [400.0, 500.0, 1100.0, 620.0], aim: [12.0, -7.0], cross_radius: 67.2, ..Default::default() };
    let pieces: [&[[f32; 4]]; 3] = [&[], &[[22.0, 1728.0, 795.0, 361.0]], &[[1.0, 2.0, 3.0, 4.0], [5.0, 6.0, 7.0, 8.0], [9.0, 10.0, 11.0, 12.0], [0.0, 0.0, 1.0, 1.0], [2.0, 2.0, 2.0, 2.0], [3.0, 3.0, 3.0, 3.0]]];
    let mut checked = 0;
    for kept_out in [false, true] {
        for menu in [false, true] {
            for over in [None, Some(placed)] {
                for rects in pieces {
                    match (old(kept_out, menu, over, rects, size), step(kept_out, menu)) {
                        (Old::Lay(was), Step::LayOver) => {
                            let now = over_params(size, menu, over, rects);
                            assert!(same(&was, &now), "kept out, menu {menu}, {over:?}, {rects:?}: {now:?} against {was:?}");
                        }
                        (Old::PiecesOut, Step::PiecesOut) | (Old::Nothing, Step::AsDrawn) => {}
                        (was, now) => panic!("kept out {kept_out}, menu {menu}: {now:?} against {was:?}"),
                    }
                    checked += 1;
                }
            }
        }
    }
    assert_eq!(checked, 24);
}
