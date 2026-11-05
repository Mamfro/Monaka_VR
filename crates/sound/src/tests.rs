use super::*;
use monaka_core::math::{IDENTITY, Vec3, yaw_rotation};

const HEAD: Vec3 = [0.0, 1.6, 0.0];
const LEFT: Vec3 = [-5.0, 1.6, 0.0];

#[test]
fn a_sound_on_the_left_is_on_the_left_until_the_head_turns_round() {
    assert!((hearing(LEFT, IDENTITY, HEAD, 5.0).direction[0] + 1.0).abs() < 1e-4);
    assert!((hearing(LEFT, yaw_rotation(std::f32::consts::PI), HEAD, 5.0).direction[0] - 1.0).abs() < 1e-4);
    assert!(hearing([0.0, 1.6, -5.0], IDENTITY, HEAD, 5.0).direction[0].abs() < 1e-4, "ahead: between the ears");
}

#[test]
fn nearer_is_louder_within_reason() {
    assert!(hearing(LEFT, IDENTITY, [-3.0, 1.6, 0.0], 5.0).gain > hearing(LEFT, IDENTITY, HEAD, 5.0).gain);
    assert!(hearing(LEFT, IDENTITY, [-4.99, 1.6, 0.0], 5.0).gain <= 1.4, "not deafening up close");
}

/// The mix of one click on the left channel (the right silent), heard as `side` says: each ear's
/// samples.
fn click(direction: Vec3) -> [Vec<f32>; 2] {
    let rate = 22_050;
    let mut samples = vec![0i16; rate * 2];
    samples[2 * 1000] = 30_000;
    let mut mixer = mix::Mixer::new(rate as u32);
    let heard = [Hearing { direction, gain: 1.0 }, Hearing { direction: [0.0, 0.0, -1.0], gain: 1.0 }];
    // Once to settle where it is heard from (the click 1000 recording frames in is 2000 mix
    // frames in), then the block with the click in it.
    let mut out = vec![0.0f32; 2 * 1900];
    mixer.render(&samples, 2, heard, &mut out);
    let mut out = vec![0.0f32; 2 * 600];
    mixer.render(&samples, 2, heard, &mut out);
    [0, 1].map(|ear| out.iter().skip(ear).step_by(2).copied().collect())
}

fn first_loud(ear: &[f32]) -> usize {
    let peak = ear.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    ear.iter().position(|v| v.abs() > peak * 0.5).unwrap_or(usize::MAX)
}

#[test]
fn a_sound_on_the_left_reaches_the_left_ear_first_and_louder() {
    let [left, right] = click([-1.0, 0.0, 0.0]);
    assert!(first_loud(&left) < first_loud(&right), "left ear first: {} vs {}", first_loud(&left), first_loud(&right));
    let energy = |ear: &[f32]| ear.iter().map(|v| v * v).sum::<f32>();
    assert!(energy(&left) > energy(&right) * 1.5, "louder on the left");
    // About 0.6 ms later round the head: some 25 samples at 44.1 kHz.
    let late = first_loud(&right) - first_loud(&left);
    assert!((15..=40).contains(&late), "{late} samples later");
}

#[test]
fn a_sound_ahead_reaches_both_ears_alike() {
    let [left, right] = click([0.0, 0.0, -1.0]);
    assert!(left.iter().zip(&right).all(|(l, r)| (l - r).abs() < 0.05), "the same in both ears, but for the echoes' small differences");
    assert_eq!(first_loud(&left), first_loud(&right));
}

#[test]
fn the_ears_tell_ahead_from_behind() {
    let ears = mix::Ears::get();
    let (ahead, behind) = (ears.pair([0.0, 0.0, -1.0]), ears.pair([0.0, 0.0, 1.0]));
    let apart: f32 = ahead[0].iter().zip(&behind[0]).map(|(a, b)| (a - b).abs()).sum();
    assert!(apart > 0.5, "behind sounds unlike ahead: {apart}");
    // Ahead keeps its level.
    assert!((ahead[0].iter().map(|v| v * v).sum::<f32>() - 1.0).abs() < 1e-3);
    // The left is the right mirrored.
    let (left, right) = (ears.pair([-1.0, 0.0, 0.0]), ears.pair([1.0, 0.0, 0.0]));
    assert_eq!(left[0], right[1]);
}
