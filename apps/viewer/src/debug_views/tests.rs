use super::*;

#[test]
fn only_the_depth_tuning_is_kept_between_runs_not_the_panels() {
    let settings = Settings {
        wanted: Categories::NONE.with(Category::Skeleton, true).with(Category::HideHud, true),
        view: View::DepthEdges,
        depth_pass: 3,
        depth_buffer: 2,
        target: 4,
        depth_near: monaka_overlay::menu::DepthNear::One,
        depth_range: 25.0,
        depth_log: true,
        capture_overlays: true,
        recording: true,
        frozen: true,
        panels: monaka_overlay::panels::Panels::NONE.with(monaka_overlay::panels::PanelId::Stats, true),
    };
    let kept = parse(&text_of(&settings));
    assert_eq!(kept, Settings { depth_pass: 3, depth_buffer: 2, target: 4, depth_near: monaka_overlay::menu::DepthNear::One, depth_range: 25.0, depth_log: true, capture_overlays: true, ..Settings::default() });
    assert_eq!(parse("skeleton=on\nview=Depth\nrecord=on\ndepth_range=7"), Settings::default(), "an old file's drawings, view and recording, and a range not offered, are not taken");
}

#[test]
fn a_pose_matrix_has_its_axes_as_columns() {
    let half = std::f32::consts::FRAC_PI_4;
    // Turned 90 degrees left: its x axis points ahead (-Z), its z axis right (+X).
    let m = matrix([0.0, half.sin(), 0.0, half.cos()], [1.0, 2.0, 3.0]);
    let close = |a: f32, b: f32| (a - b).abs() < 1e-5;
    assert!(close(m[0], 0.0) && close(m[8], -1.0), "x column: {m:?}");
    assert!(close(m[2], 1.0) && close(m[10], 0.0), "z column: {m:?}");
    assert_eq!([m[3], m[7], m[11]], [1.0, 2.0, 3.0]);
}
