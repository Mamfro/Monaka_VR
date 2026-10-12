//! The eye site's world-drawn detector against Dying Light 1's own, which it replaced.

use super::*;

/// Dying Light 1's: the present of the last view setup that wrote an eye camera; the world drawn
/// while no more than eight presents have gone by since.
fn drawn_before(now: u64, last_view: u64) -> bool {
    now.saturating_sub(last_view) <= 8
}

#[test]
fn the_world_is_drawn_as_dying_light_1_judged_it() {
    for last_view in [0, 1, 7, 100] {
        for now in [0, 1, 8, 9, 50, 108, 109] {
            assert_eq!(drawn_recently(now, last_view), drawn_before(now, last_view), "drawn at {last_view}, now {now}");
        }
    }
}
