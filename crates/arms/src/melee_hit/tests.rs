use super::*;
use std::time::Duration;

#[test]
fn a_body_sized_box_the_blade_comes_near_is_reached() {
    let look = ReachLook::new();
    let sweep = look.sweep_at([0.0, 1.2, 0.0], [0.0, 1.2, -1.0], Instant::now()).unwrap();
    assert!(sweep.touches(&[-0.3, 0.0, -1.5, 0.3, 1.8, -1.05]), "within the margin of the tip");
    assert!(!sweep.touches(&[-0.3, 0.0, -1.5, 0.3, 0.3, -1.05]), "too low for a body");
    assert!(!sweep.touches(&[2.0, 0.0, -1.5, 3.0, 1.8, -1.05]), "off to the side");
    assert!(!sweep.touches(&[f32::NAN, 0.0, -1.5, 0.3, 1.8, -1.05]));
}

#[test]
fn the_sweep_looks_ahead_as_far_as_the_blade_moves() {
    let look = ReachLook::new();
    let start = Instant::now();
    look.sweep_at([0.0; 3], [0.0, 0.0, -1.0], start).unwrap();
    // Moved 0.05 m right in 50 ms: 0.15 s ahead it is 0.15 m further.
    let sweep = look.sweep_at([0.05, 0.0, 0.0], [0.05, 0.0, -1.0], start + Duration::from_millis(50)).unwrap();
    let farthest = sweep.points.iter().map(|p| p[0]).fold(f32::MIN, f32::max);
    assert!((farthest - 0.2).abs() < 1e-4, "{farthest}");
    // A look long after the last: nothing to go on, the blade where it is.
    let sweep = look.sweep_at([0.5, 0.0, 0.0], [0.5, 0.0, -1.0], start + Duration::from_millis(500)).unwrap();
    assert!(sweep.points.iter().all(|p| (p[0] - 0.5).abs() < 1e-6));
}

fn blade(at: Vec3, along: Vec3, edge: Vec3) -> held::WorldLine {
    held::WorldLine { from: at, along, length: None, edge: Some(edge) }
}

#[test]
fn a_hit_leads_with_the_edge_when_the_tip_moves_that_way() {
    let before = blade([0.0; 3], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]);
    let edge_first = blade([0.0, 0.0, 0.1], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]);
    assert_eq!(lead(&before, &edge_first), Some(1.0));
    let back_first = blade([0.0, 0.0, -0.1], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]);
    assert_eq!(lead(&before, &back_first), Some(-1.0));
    assert_eq!(lead(&before, &before), None, "no motion");
    assert_eq!(lead(&before, &held::WorldLine { edge: None, ..edge_first }), None, "no edge");
}
