//! The ready-made against each game's lockpicking before it, read by read.

use super::*;

fn hand(orientation: Quat) -> HandPose {
    HandPose { orientation, position: [0.0; 3], valid: 1, received_tick: 0 }
}

/// A hand pointing ahead rolled `angle` radians about where it points, tipped `tip` radians up.
fn rolled(angle: f32, tip: f32) -> Quat {
    let roll = [0.0, 0.0, (angle / 2.0).sin(), (angle / 2.0).cos()];
    let pitch = [(tip / 2.0).sin(), 0.0, 0.0, (tip / 2.0).cos()];
    monaka_core::math::multiply(pitch, roll)
}

/// Dying Light 1's rules: the pick 1 per radian, the screwdriver fully at 0.8 radians.
const DL1: Lockpicking = Lockpicking { twist: Twist::FromStart, pick_gain: 1.0, full_turn: 0.8, hand: (Point::Controller, At::Frame) };
/// The Chrome games': the pick twice the wrist, the screwdriver fully at 60 degrees.
const CHROME: Lockpicking = Lockpicking { twist: Twist::AgainstUp, pick_gain: 2.0, full_turn: std::f32::consts::FRAC_PI_3, hand: (Point::Palm, At::Latest) };

/// Dying Light 1's source before: each hand's own reference, taken at its action's fresh read (or
/// its first).
fn old_dl1(references: &mut [Option<Quat>; 2], action: Action, reading: Reading, hands: [Option<HandPose>; 2]) -> Option<f32> {
    let side = side(action);
    let now = hands[side]?.orientation;
    if reading.fresh {
        references[side] = None;
    }
    let reference = *references[side].get_or_insert(now);
    let turn = roll_between(reference, now);
    Some(if action == Action::Pick { turn * 1.0 } else { (turn / 0.8).clamp(-1.0, 1.0) })
}

/// The Chrome games' before: both wrists' rolls (degrees) taken at the pick's fresh read (an
/// unreadable right wrist as 0), the pick twice the twist, the turn over 60 degrees.
fn old_chrome(starts: &mut Option<[f32; 2]>, action: Action, reading: Reading, hands: [Option<HandPose>; 2]) -> Option<f32> {
    let wrist = |side: usize| hands[side].and_then(|h| wrist_roll(h.orientation, UP)).map(f32::to_degrees);
    let now = wrist(side(action))?;
    if reading.fresh && action == Action::Pick {
        *starts = Some([now, wrist(RIGHT_HAND).unwrap_or(0.0)]);
    }
    let starts = (*starts)?;
    Some(match action {
        Action::Pick => (wrap_degrees(now - starts[0]) * 2.0).to_radians(),
        _ => (wrap_degrees(now - starts[1]) / 60.0).clamp(-1.0, 1.0),
    })
}

/// A run of reads: both hands rolling, now and then untracked or pointing straight up, the pick's
/// and the turn's fresh reads where each game's minigame gives them.
fn timeline(dl1_fresh_turns: bool) -> Vec<(Action, Reading, [Option<HandPose>; 2])> {
    let mut reads = Vec::new();
    for i in 0..600 {
        let t = i as f32 * 0.037;
        let left = (i % 97 >= 5).then(|| hand(rolled(2.8 * (t * 0.7).sin(), if (300..320).contains(&i) { 1.57 } else { 0.3 * t.cos() })));
        let right = (i % 61 >= 4).then(|| hand(rolled(1.9 * (t * 1.1).cos() - 0.4, 0.2 * (t * 0.5).sin())));
        let lock_start = i % 150 == 0;
        reads.push((Action::Pick, Reading { fresh: lock_start }, [left, right]));
        reads.push((Action::Turn, Reading { fresh: dl1_fresh_turns && lock_start }, [left, right]));
    }
    reads
}

#[test]
fn dying_light_1s_values_are_the_ones_it_answered_before() {
    let (mut old, mut new) = ([None; 2], Picking::new());
    // Dying Light 1's minigame gives each action its own fresh read; one not answered (the hand
    // untracked) stays fresh until one is.
    let mut fresh = [false; 2];
    let mut answered = 0;
    for (i, (action, reading, hands)) in timeline(true).into_iter().enumerate() {
        let which = usize::from(action == Action::Turn);
        fresh[which] |= reading.fresh;
        let reading = Reading { fresh: fresh[which] };
        let was = old_dl1(&mut old, action, reading, hands);
        let now = new.value(&DL1, action, reading, hands);
        assert_eq!(now, was, "read {i}");
        if now.is_some() {
            fresh[which] = false;
            answered += 1;
        }
    }
    assert!(answered > 1000, "{answered}");
}

#[test]
fn the_chrome_games_values_are_the_ones_they_answered_before() {
    let (mut old, mut new) = (None, Picking::new());
    let mut answered = 0;
    // The Chrome games' screwdriver is never read fresh: the pick's fresh read starts both.
    for (i, (action, reading, hands)) in timeline(false).into_iter().enumerate() {
        let was = old_chrome(&mut old, action, reading, hands);
        let now = new.value(&CHROME, action, reading, hands);
        // Before the first lock the old turn had no start and no value; the new one takes its own
        // (reads the minigame never makes: no lock, no read).
        if old.is_none() {
            continue;
        }
        match (was, now) {
            (Some(was), Some(now)) => assert!((was - now).abs() < 1e-5, "read {i}: {was} {now}"),
            _ => assert_eq!(now.is_some(), was.is_some(), "read {i}"),
        }
        answered += now.is_some() as usize;
    }
    assert!(answered > 700, "{answered}");
    assert_eq!(new.locks(), 2, "the locks whose first pick read had a left wrist to read");
}
