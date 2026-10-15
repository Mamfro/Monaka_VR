//! The checks around a carried update: poses that cannot be a tracked hand or head, placements
//! that cannot be the player's arms (nothing is written then), arms handed back nowhere near the
//! camera, and arms the game did not animate again since they were moved (put back as written).
//! Each logs its first few.

use crate::skeleton::Skeleton;
use monaka_core::camera::{Mat34, position};
use monaka_contract::protocol::{HandPose, HeadPose};
use monaka_producer::log;
use std::sync::atomic::{AtomicU64, Ordering::*};

/// A controller pose that can be a tracked hand: finite, a rotation, within a room.
pub(super) fn plausible_hand(hand: &HandPose) -> bool {
    plausible_pose(hand.orientation, hand.position)
}

pub(super) fn plausible_head(head: &HeadPose) -> bool {
    plausible_pose(head.orientation, head.position)
}

/// Tracking space is the room: nothing tracked is further than this from its origin (metres).
pub(super) const ROOM: f32 = 10.0;

pub(super) fn plausible_pose(orientation: [f32; 4], position: [f32; 3]) -> bool {
    let norm = orientation.iter().map(|v| v * v).sum::<f32>().sqrt();
    let distance = position.iter().map(|v| v * v).sum::<f32>().sqrt();
    (0.9..1.1).contains(&norm) && distance < ROOM
}

/// A placed bone is never further than this from the camera (metres): the arms reach about one,
/// the feet stand under it, and a climb backs the view off the wall.
pub(super) const BONE_REACH: f32 = 4.0;

/// A bone placed from `world` (as animated) by a rigid move: finite, near the camera `game`, and
/// scaled as the animation scaled it (column lengths, within half and double).
pub(super) fn plausible_placement(placed: &Mat34, world: &Mat34, game: &Mat34) -> bool {
    let column = |m: &Mat34, c: usize| (m[c] * m[c] + m[4 + c] * m[4 + c] + m[8 + c] * m[8 + c]).sqrt();
    placed.iter().all(|v| v.is_finite())
        && monaka_core::math::distance(position(placed), position(game)) < BONE_REACH
        && (0..3).all(|c| {
            let ratio = column(placed, c) / column(world, c);
            (0.5..2.0).contains(&ratio)
        })
}

pub(super) fn note_implausible(name: &str, placed: &Mat34, game: &Mat34) {
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let n = COUNT.fetch_add(1, Relaxed) + 1;
    if log::seldom(n, 5) {
        log!("hand rig: {n}x a bone would have been placed implausibly; nothing written (first: {name} at {placed:?}, camera {game:?})");
    }
}

/// Arms handed back further than this from the camera are not animated arms (metres).
pub(super) const ARMS_REACH: f32 = 20.0;

/// Whether every readable element is within [`ARMS_REACH`] of the camera `game`.
pub(super) fn near(worlds: &[Mat34], game: &Mat34) -> bool {
    let camera = position(game);
    worlds.iter().filter(|w| w[0].is_finite()).all(|w| monaka_core::math::distance(position(w), camera) < ARMS_REACH)
}

/// Logs the first few animated wrists that are not rigid (column lengths off 1 by more than 5%).
pub(super) fn note_scaled(what: &str, m: &Mat34) {
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let lengths: [f32; 3] = std::array::from_fn(|c| (m[c] * m[c] + m[4 + c] * m[4 + c] + m[8 + c] * m[8 + c]).sqrt());
    if lengths.iter().any(|l| (l - 1.0).abs() > 0.05) {
        let n = COUNT.fetch_add(1, Relaxed) + 1;
        if log::seldom(n, 5) {
            log!("hand rig: {n}x an animated wrist not rigid ({what}: axes {lengths:.2?})");
        }
    }
}

/// Bones handed back within this of what was written (largest rotation entry; metres) are the
/// rig's own, swayed by the game: a fresh animation puts the arms back in the game's pose, far
/// from the controllers'.
pub(super) const SWAY_TURN: f32 = 0.02;
pub(super) const SWAY_SHIFT: f32 = 0.01;

/// How far the bones handed back are from what was written, over the bones written: the largest
/// difference of a rotation entry, and of a position (metres).
pub(super) fn written_apart(written: &[Mat34], handed: &[Mat34]) -> (f32, f32) {
    let mut apart = (0.0f32, 0.0f32);
    for (w, h) in written.iter().zip(handed).filter(|(w, _)| w[0].is_finite()) {
        for (i, (a, b)) in w.iter().zip(h).enumerate() {
            let d = (a - b).abs();
            let d = if d.is_nan() { f32::INFINITY } else { d };
            if i % 4 == 3 {
                apart.1 = apart.1.max(d);
            } else {
                apart.0 = apart.0.max(d);
            }
        }
    }
    apart
}

pub(super) fn note_swayed(turn: f32, shift: f32) {
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let n = COUNT.fetch_add(1, Relaxed) + 1;
    if log::seldom(n, 5) {
        log!("hand rig: the arms came back swayed off what was written ({turn:.4} rotation, {:.1} mm) {n}x; put back as written", shift * 1000.0);
    }
}

/// Diagnostic: arms handed back mostly as written, a few bones not: the game changed only those
/// since the last move (paused, something of its own still at work), which defeats the check for
/// arms not animated again. Logs which bones and how far.
pub(super) fn note_mostly_ours(skeleton: &dyn Skeleton, written: &[Mat34], handed: &[Mat34]) {
    static LOGGED: AtomicU64 = AtomicU64::new(0);
    let finite = written.iter().filter(|w| w[0].is_finite()).count();
    let changed: Vec<(usize, f32)> = written
        .iter()
        .zip(handed)
        .enumerate()
        .filter(|(_, (w, _))| w[0].is_finite())
        .filter_map(|(element, (w, h))| {
            let apart = w.iter().zip(h).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
            (apart > 0.0).then_some((element, apart))
        })
        .collect();
    // A fresh animation changes nearly every bone; a handful changed is the game touching up ours.
    if changed.is_empty() || changed.len() * 5 > finite || LOGGED.fetch_add(1, Relaxed) >= 12 {
        return;
    }
    let shown: Vec<String> = changed.iter().take(8).map(|&(e, apart)| format!("{} {apart:.3}", skeleton.name(e as i32))).collect();
    log!("hand rig: the arms came back as written but for {} of {finite} bones: {}", changed.len(), shown.join(", "));
}

/// Counts the calls that found the arms not animated again since they were moved (paused, or a
/// second call in one frame); logs the first few.
pub(super) fn note_reanimation_missed() {
    static MISSED: AtomicU64 = AtomicU64::new(0);
    let n = MISSED.fetch_add(1, Relaxed) + 1;
    if log::seldom(n, 3) {
        log!("hand rig: the game had not animated the arms since they were moved {n}x; put back as written");
    }
}

pub(super) fn log_first_missing_arm() {
    static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !LOGGED.swap(true, Relaxed) {
        log!("hand rig: the skeleton lacks an upperarm/forearm/hand element; arms left to the game");
    }
}

