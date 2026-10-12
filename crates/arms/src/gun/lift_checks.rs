//! The ready-made against Dying Light 1's shooting from the barrel before it.

use super::*;
use crate::held::Line;
use crate::launch::Flight;
use monaka_core::camera::Mat34;
use monaka_core::math::{cross, dot, length, unit};

/// `v` turned by the rotation taking unit `from` to unit `to`, as the fire hook turned pellets
/// before.
fn old_turned(v: Vec3, from: Vec3, to: Vec3) -> Vec3 {
    let axis = cross(from, to);
    let (sin, cos) = (length(axis), dot(from, to));
    let Some(k) = unit(axis).filter(|_| sin > 1e-6) else { return v };
    let kv = cross(k, v);
    let kd = dot(k, v) * (1.0 - cos);
    [0, 1, 2].map(|i| v[i] * cos + kv[i] * sin + k[i] * kd)
}

/// Dying Light 1's shot before: from the right holder's +z, the muzzle 0.25 m along it, each
/// projectile turned from the aim onto it at its own speed.
fn old_shot(holder: &Mat34, aim: Vec3, velocity: Vec3) -> Option<(Vec3, Vec3)> {
    let z = [holder[2], holder[6], holder[10]];
    let l = length(z);
    if !(l.is_finite() && l > 1e-6) {
        return None;
    }
    let barrel = z.map(|c| c / l);
    let start = [holder[3] + barrel[0] * 0.25, holder[7] + barrel[1] * 0.25, holder[11] + barrel[2] * 0.25];
    let along = unit(velocity)?;
    let speed = length(velocity);
    Some((start, old_turned(barrel, aim, along).map(|c| c * speed)))
}

#[test]
fn the_turn_is_the_one_the_fire_hook_used() {
    for (v, from, to) in [([0.3, 0.1, -1.0], [0.0, 0.0, -1.0], [0.6, 0.0, -0.8]), ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 1.0, 0.0]), ([0.0, 2.0, 1.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0])] {
        assert_eq!(math::turned(v, from, to), old_turned(v, from, to));
    }
}

#[test]
fn each_projectile_leaves_where_dying_light_1_shot_it() {
    // Dying Light 1's gun line in its holder: along +z, the muzzle 0.25 m out.
    let line = Line { from: [0.0; 3], along: [0.0, 0.0, 1.0], length: Some(0.25), edge: None };
    let holders: [Mat34; 3] = [
        [1.0, 0.0, 0.0, 400.0, 0.0, 1.0, 0.0, 17.5, 0.0, 0.0, 1.0, 280.0],
        [0.0, 0.0, 2.0, 1.0, 0.0, 2.0, 0.0, 2.0, -2.0, 0.0, 0.0, 3.0],
        [0.8, -0.6, 0.0, -5.0, 0.6, 0.8, 0.0, 0.5, 0.0, 0.0, 1.0, 9.0],
    ];
    let aims = [[0.0, 0.0, -1.0], unit([1.0, 0.0, -1.0]).unwrap(), unit([0.1, 0.3, -1.0]).unwrap()];
    for holder in holders {
        for aim in aims {
            for velocity in [[0.0; 3], [0.0, 0.0, -300.0], [3.0, -1.0, -280.0], [-40.0, 2.0, 1.0]] {
                let was = old_shot(&holder, aim, velocity);
                let world = line.in_world(&holder).unwrap();
                let fired = Fired { weapon: Weapon::Gun, game: Flight { from: [1.0, 2.0, 3.0], velocity }, aim: Some(aim) };
                let now = from_barrel(&fired, world.end().unwrap(), world.along).and_then(|shot| shot.flight(&fired.game));
                match (now, was) {
                    (Some(now), Some(was)) => {
                        let close = |a: Vec3, b: Vec3| a.iter().zip(b).all(|(x, y)| (x - y).abs() <= 1e-4 * (1.0 + y.abs()));
                        assert!(close(now.from, was.0) && close(now.velocity, was.1), "{holder:?} {aim:?} {velocity:?}: {now:?} {was:?}");
                    }
                    (now, was) => assert_eq!(now.is_some(), was.is_some(), "{holder:?} {velocity:?}"),
                }
            }
        }
    }
}
