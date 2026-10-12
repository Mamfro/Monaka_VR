//! The ready-made against each game's physical melee before it: the blade the look places, and
//! Dying Light 1's edge rule.

use super::*;
use crate::held::Line;
use monaka_core::camera::Mat34;
use monaka_core::math::unit;

/// The Chrome games' blade before: the holder's axes made unit, the blade's way in the model carried
/// out on them, from the holder's place.
fn chrome_blade(holder: &Mat34, along_in_model: Vec3, length: f32) -> Option<(Vec3, Vec3)> {
    let axes = [0, 1, 2].map(|c| unit([holder[c], holder[4 + c], holder[8 + c]]));
    let [Some(x), Some(y), Some(z)] = axes else { return None };
    let along = [0, 1, 2].map(|k| x[k] * along_in_model[0] + y[k] * along_in_model[1] + z[k] * along_in_model[2]);
    let palm = [holder[3], holder[7], holder[11]];
    Some((palm, [0, 1, 2].map(|k| palm[k] + along[k] * length)))
}

/// Dying Light 1's blade before: from `R_HandHolder` along its y column, the weapon's length.
fn dl1_blade(m: &Mat34, length: f32) -> (Vec3, Vec3) {
    let palm = [m[3], m[7], m[11]];
    (palm, [0, 1, 2].map(|k| palm[k] + [m[1], m[5], m[9]][k] * length))
}

/// Dying Light 1's edge lead before: the tip one unit up the holder's y, its way over the last two
/// looks in the holder's own axes.
fn dl1_lead(before: &Mat34, now: &Mat34) -> Option<Vec3> {
    let tip = |m: &Mat34| [m[3] + m[1], m[7] + m[5], m[11] + m[9]];
    let (a, b) = (tip(before), tip(now));
    let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let length = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    if length.is_nan() || length <= 0.002 {
        return None;
    }
    Some([0, 1, 2].map(|c| (d[0] * now[c] + d[1] * now[4 + c] + d[2] * now[8 + c]) / length))
}

const DL1_MELEE: Line = Line { from: [0.0; 3], along: [0.0, 1.0, 0.0], length: None, edge: Some([0.0, 0.0, 1.0]) };

/// A rigid holder turned `angle` about `axis`, at `at`.
fn holder(axis: Vec3, angle: f32, at: Vec3) -> Mat34 {
    let a = unit(axis).unwrap();
    let s = (angle / 2.0).sin();
    let q = [a[0] * s, a[1] * s, a[2] * s, (angle / 2.0).cos()];
    let [x, y, z] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]].map(|v| monaka_core::math::rotate(q, v));
    [x[0], y[0], z[0], at[0], x[1], y[1], z[1], at[1], x[2], y[2], z[2], at[2]]
}

#[test]
fn the_blade_from_the_held_items_line_is_the_one_placed_before() {
    let holders: [Mat34; 3] = [
        [0.744, 0.668, 0.030, 407.438, -0.350, 0.427, -0.834, 17.538, -0.569, 0.610, 0.551, 287.791],
        [-0.0, 0.0, 1.6, -12.5, 0.0, 0.6, -0.0, 1.25, -1.0, 0.0, 0.0, -0.0],
        [0.5, -0.5, 0.0, 0.0, 0.5, 0.5, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0],
    ];
    for m in holders {
        for along in [[0.0, 1.0, 0.0], [0.6, 0.0, -0.8], [-0.267, 0.534, 0.802]] {
            for length in [0.15, 1.1] {
                let line = Line { from: [0.0; 3], along, length: None, edge: None }.in_world(&m).unwrap();
                assert_eq!(Some((line.from, line.at(length))), chrome_blade(&m, along, length), "{m:?} {along:?}");
            }
        }
    }
}

#[test]
fn dying_light_1s_blade_and_edge_are_the_ones_read_before_for_a_rigid_holder() {
    let close = |a: Vec3, b: Vec3| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4);
    let mut compared = 0;
    for i in 0..200 {
        let t = i as f32 * 0.05;
        let before = holder([0.3, 1.0, 0.2 * t.sin()], 1.3 * t.sin(), [400.0 + 0.1 * t, 17.0, 280.0]);
        let now = holder([0.3, 1.0, 0.2 * (t + 0.05).sin()], 1.3 * (t + 0.05).sin() + if i % 3 == 0 { 0.0 } else { 0.02 }, [400.0 + 0.1 * t, 17.0 + 0.01 * (i % 5) as f32, 280.0]);
        let (old_palm, old_tip) = dl1_blade(&now, 1.1);
        let line = DL1_MELEE.in_world(&now).unwrap();
        assert!(close(line.from, old_palm) && close(line.at(1.1), old_tip), "{i}");
        let was = dl1_lead(&before, &now);
        let lead = lead(&DL1_MELEE.in_world(&before).unwrap(), &line);
        match (was, lead) {
            (Some(was), Some(lead)) => {
                // Cut or blunt as before (a lead within rounding of the rule's edge left aside).
                if (was[2] - EDGE_LEAD).abs() > 1e-3 {
                    assert_eq!(lead >= EDGE_LEAD, was[2] >= EDGE_LEAD, "{i}: {lead} {was:?}");
                }
                assert!((lead - was[2]).abs() < 1e-3, "{i}: {lead} {was:?}");
                compared += 1;
            }
            (was, lead) => assert_eq!(was.is_some(), lead.is_some(), "{i}"),
        }
    }
    assert!(compared > 100, "{compared}");
}
