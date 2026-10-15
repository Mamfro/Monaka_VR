use super::*;

const FRAME: i64 = 11_111_111;

fn at(position: Vec3, orientation: Quat) -> HandPose {
    HandPose { orientation, position, valid: 1, received_tick: 0 }
}

fn about_y(angle: f32) -> Quat {
    [0.0, (angle * 0.5).sin(), 0.0, (angle * 0.5).cos()]
}

#[test]
fn off_passes_poses_through() {
    let mut off = Smoother::new(0.0);
    let pose = at([0.1, 1.0, -0.3], about_y(0.2));
    for frame in 0..3 {
        assert_eq!(off.smooth(1, frame * FRAME, pose, pose), (pose, pose));
    }
}

#[test]
fn jitter_at_rest_is_steadied() {
    let mut smoother = Smoother::new(3.0);
    let mut worst = 0.0f32;
    for frame in 0..200 {
        // Half a millimetre and a tenth of a degree of shake about a hand held still.
        let shake = if frame % 2 == 0 { 0.0005 } else { -0.0005 };
        let raw = at([shake, 1.0, -0.3], about_y(shake * 3.5));
        let (_, grip) = smoother.smooth(0, frame * FRAME, raw, raw);
        if frame > 100 {
            worst = worst.max(grip.position[0].abs());
        }
    }
    assert!(worst < 0.0002, "shake left: {worst}");
}

#[test]
fn a_swing_keeps_up() {
    let mut smoother = Smoother::new(3.0);
    let mut lag = 0.0f32;
    for frame in 0..60 {
        // 3 m/s across, after a moment held still.
        let x = if frame < 20 { 0.0 } else { (frame - 20) as f32 * 3.0 / 90.0 };
        let raw = at([x, 1.0, -0.3], math::IDENTITY);
        let (_, grip) = smoother.smooth(1, frame * FRAME, raw, raw);
        if frame > 30 {
            lag = lag.max(x - grip.position[0]);
        }
    }
    // Under 2.5 cm behind at 3 m/s: under 10 ms.
    assert!(lag < 0.025, "lag {lag}");
}

#[test]
fn the_aim_keeps_its_offset_from_the_grip() {
    let mut smoother = Smoother::new(10.0);
    for frame in 0..30 {
        let turn = about_y(frame as f32 * 0.05);
        let grip = at([0.0, 1.0, -0.3 - frame as f32 * 0.01], turn);
        let offset = [0.0, 0.02, -0.05];
        let aim_at = math::rotate(turn, offset);
        let aim = at([0, 1, 2].map(|k| grip.position[k] + aim_at[k]), multiply(turn, about_y(0.7)));
        let (aim_out, grip_out) = smoother.smooth(0, frame * FRAME, aim, grip);
        let back = math::rotate(conjugate(grip_out.orientation), [0, 1, 2].map(|k| aim_out.position[k] - grip_out.position[k]));
        assert!(math::distance(back, offset) < 1e-4, "offset {back:?}");
        assert!(angle_between(multiply(conjugate(grip_out.orientation), aim_out.orientation), about_y(0.7)) < 1e-3);
    }
}

#[test]
fn a_gap_or_a_lost_controller_starts_afresh() {
    let mut smoother = Smoother::new(10.0);
    let here = at([0.0, 1.0, 0.0], math::IDENTITY);
    let there = at([1.0, 1.0, 0.0], math::IDENTITY);
    smoother.smooth(0, 0, here, here);
    assert_eq!(smoother.smooth(0, 1_000_000_000, there, there).1, there, "a second later");
    let lost = HandPose::default();
    assert_eq!(smoother.smooth(0, 1_000_000_000 + FRAME, lost, lost), (lost, lost));
    assert_eq!(smoother.smooth(0, 1_000_000_000 + 2 * FRAME, here, here).1, here);
}
