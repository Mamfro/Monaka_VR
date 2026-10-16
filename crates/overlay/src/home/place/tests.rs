use super::*;

fn near(a: Vec3, b: Vec3) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4)
}

#[test]
fn the_front_panel_stands_ahead_and_the_others_to_its_sides() {
    let ring = Ring::at(math::IDENTITY, [0.0, 1.6, 0.0]);
    let (front, centre) = ring.place(1, 1, 3);
    assert!(near(centre, [0.0, 1.6 - BELOW_EYES, -RADIUS]));
    assert!(near(math::rotate(front, [0.0, 0.0, 1.0]), [0.0, 0.0, 1.0]), "it faces the head");
    // Page 0 to the left (-X), page 2 to the right (+X), as far out as each other.
    let (_, left) = ring.place(0, 1, 3);
    let (_, right) = ring.place(2, 1, 3);
    assert!(left[0] < -0.5 && right[0] > 0.5 && (left[0] + right[0]).abs() < 1e-4);
    assert!(near(math::sub(left, [0.0, 1.6 - BELOW_EYES, 0.0]).map(f32::abs), math::sub(right, [0.0, 1.6 - BELOW_EYES, 0.0]).map(f32::abs)));
}

#[test]
fn turning_the_ring_brings_another_panel_in_front() {
    let ring = Ring::at(math::IDENTITY, [0.0, 0.0, 0.0]);
    // Page 2 in front: page 1 to its left, page 0 round to its right.
    assert!(near(ring.place(2, 2, 3).1, [0.0, -BELOW_EYES, -RADIUS]));
    assert!(ring.place(1, 2, 3).1[0] < 0.0 && ring.place(0, 2, 3).1[0] > 0.0);
}

#[test]
fn the_ring_follows_the_way_the_head_faced_not_its_pitch() {
    // Turned 90 degrees left and looking down: the front panel to the left, level.
    let (sin, cos) = (-std::f32::consts::FRAC_PI_8).sin_cos();
    let turned = math::multiply(math::yaw_rotation(std::f32::consts::FRAC_PI_2), [sin, 0.0, 0.0, cos]);
    let (_, centre) = Ring::at(turned, [0.0, 1.0, 0.0]).place(0, 0, 3);
    assert!(near(centre, [-RADIUS, 1.0 - BELOW_EYES, 0.0]), "{centre:?}");
}

#[test]
fn a_ray_at_a_panel_finds_where_on_it() {
    let size = [WIDTH, height()];
    let centre = [0.0, 0.0, -1.0];
    // Straight at the centre, from a metre away.
    let (along, at) = hit(math::IDENTITY, centre, size, [0.0, 0.0, 0.0], [0.0, 0.0, -1.0]).unwrap();
    assert!((along - 1.0).abs() < 1e-5 && near([at[0], at[1], 0.0], [0.5, 0.5, 0.0]));
    // Up and to the right: toward the top right corner.
    let (_, at) = hit(math::IDENTITY, centre, size, [0.3, 0.2, 0.0], [0.0, 0.0, -1.0]).unwrap();
    assert!(at[0] > 0.8 && at[1] < 0.3, "{at:?}");
    // Past its edge, away from it, along it, and from behind it.
    assert!(hit(math::IDENTITY, centre, size, [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]).is_none());
    assert!(hit(math::IDENTITY, centre, size, [0.0, 0.0, 0.0], [0.0, 0.0, 1.0]).is_none());
    assert!(hit(math::IDENTITY, centre, size, [0.0, 0.0, 0.0], [1.0, 0.0, 0.0]).is_none());
    assert!(hit(math::IDENTITY, centre, size, [0.0, 0.0, -2.0], [0.0, 0.0, 1.0]).is_none());
}

#[test]
fn a_controller_points_at_a_placed_panel() {
    let ring = Ring::at(math::IDENTITY, [0.0, 1.6, 0.0]);
    let (orientation, centre) = ring.place(2, 1, 3);
    // From the head toward the right panel's centre.
    let to = math::sub(centre, [0.0, 1.6, 0.0]);
    let length = math::dot(to, to).sqrt();
    let direction = math::scale(to, 1.0 / length);
    let (along, at) = hit(orientation, centre, [WIDTH, height()], [0.0, 1.6, 0.0], direction).unwrap();
    assert!((along - length).abs() < 1e-3 && (at[0] - 0.5).abs() < 1e-3 && (at[1] - 0.5).abs() < 1e-3);
    assert!(near(pointing(math::IDENTITY), [0.0, 0.0, -1.0]));
}

#[test]
fn a_turn_moves_every_panel_the_same_way_and_wraps_the_last_round() {
    let rest = [0, 1, 2].map(|page| slot_angle(page, 1, 3));
    assert_eq!(rest, [-SIDE_ANGLE, 0.0, SIDE_ANGLE]);
    // RB: the ring turns left, the panel on the right (page 2) comes in front.
    let turning = Turning::new(&rest, 2, 1);
    assert_eq!(turning.at(0.0), rest);
    let end = turning.at(1.0);
    let full = std::f32::consts::TAU;
    assert!((end[2] - 0.0).abs() < 1e-5 && (end[1] + SIDE_ANGLE).abs() < 1e-5);
    // Page 0 leaves on the left and goes on round, behind, to the right.
    assert!((end[0] - (SIDE_ANGLE - full)).abs() < 1e-5, "{end:?}");
    let halfway = turning.at(0.5);
    assert!((0..3).all(|page| halfway[page] < rest[page]), "all moving left: {halfway:?}");
    // LB back: the mirror.
    let back = Turning::new(&rest, 0, -1).at(1.0);
    assert!((back[0] - 0.0).abs() < 1e-5 && (back[2] - (full - SIDE_ANGLE)).abs() < 1e-5, "{back:?}");
}

#[test]
fn a_panel_at_an_angle_stands_on_the_ring() {
    let ring = Ring::at(math::IDENTITY, [0.0, 0.0, 0.0]);
    for angle in [0.0, 1.0, -2.5, 3.0] {
        let (_, centre) = ring.at_angle(angle);
        let flat = (centre[0] * centre[0] + centre[2] * centre[2]).sqrt();
        assert!((flat - RADIUS).abs() < 1e-4 && (centre[0] > 0.0) == (angle.sin() > 0.0), "{angle}: {centre:?}");
    }
}
