//! The step measured at the next one against Dying Light 1's own rules before it, step by step.

use super::*;

/// Dying Light 1's walk body before: the body's position at the previous step and what that step
/// asked.
struct OldLast {
    body: Option<[f32; 3]>,
    ask: Option<([f32; 3], [f32; 2], f32)>,
}

/// Dying Light 1's step before, with the game's reads (`now`: the body's position, `None` when it
/// had no body) and following's calls handed in.
fn old_before_step(
    last: &mut OldLast,
    now: Option<[f32; 3]>,
    dt: f32,
    gate: impl FnOnce() -> Option<&'static str>,
    follow: impl FnOnce(Option<&'static str>) -> [f32; 2],
    credit: impl FnOnce([f32; 3], [f32; 2], f32),
) -> [f32; 2] {
    let now = now.filter(|p| p.iter().all(|c| c.is_finite()));
    if let (Some(now), Some(before), Some((game, follow, last_dt))) = (now, last.body, last.ask) {
        let beyond = [now[0] - before[0] - game[0] * last_dt, 0.0, now[2] - before[2] - game[2] * last_dt];
        credit(beyond, follow, last_dt);
    }
    last.body = now;
    let follow = follow(gate());
    last.ask = Some(([0.0; 3], follow, dt));
    follow
}

fn old_asked(last: &mut OldLast, game: [f32; 3]) {
    if let Some(ask) = last.ask.as_mut() {
        ask.0 = if game.iter().all(|c| c.is_finite()) { game } else { [0.0; 3] };
    }
}

/// Dying Light 1's reasons before, as the walk body now gives them.
fn reason(wait: Wait) -> &'static str {
    match wait {
        Wait::NotRunning => "not running",
        Wait::ScriptedMove => "scripted move",
        Wait::PositionLocked => "position locked",
        Wait::PhysicsOff => "physics off",
        Wait::Cutscene => "cutscene",
        Wait::Attached | Wait::Animated | Wait::NoTime => unreachable!("not one of Dying Light 1's"),
    }
}

/// One recorded step: the body's position as it began, its length, why following waited, the
/// game's own velocity.
struct Step {
    body: Option<[f32; 3]>,
    dt: f32,
    wait: Option<Wait>,
    game: [f32; 3],
}

/// A walk: standing, following a step, a wall stopping it, the body unreadable a step, a
/// cutscene and a scripted move, the game's velocity unreadable, the mod not drawing.
fn recorded() -> Vec<Step> {
    let mut steps = Vec::new();
    let mut at = [10.0f32, 1.0, -4.0];
    for k in 0..60 {
        let dt = if k % 7 == 3 { 0.025 } else { 0.02 };
        let wait = match k {
            20..=24 => Some(Wait::Cutscene),
            30..=31 => Some(Wait::ScriptedMove),
            40 => Some(Wait::PositionLocked),
            41 => Some(Wait::PhysicsOff),
            55.. => Some(Wait::NotRunning),
            _ => None,
        };
        let game = match k {
            10..=19 => [1.5, -0.2, 0.5],
            45 => [f32::NAN, 0.0, 0.0],
            _ => [0.0; 3],
        };
        let body = if k == 33 { None } else if k == 34 { Some([f32::INFINITY, 0.0, 0.0]) } else { Some(at) };
        steps.push(Step { body, dt, wait, game });
        // The body goes on with the game's velocity plus some of following's, stopped by a wall
        // from step 47.
        let moving = if game.iter().all(|c| c.is_finite()) { game } else { [0.0; 3] };
        if k < 47 {
            at = [at[0] + moving[0] * dt + 0.011, at[1], at[2] + moving[2] * dt + 0.004 - 0.001 * (k % 3) as f32];
        }
    }
    steps
}

/// Following as a recorded run: a velocity per step, none while it waits.
fn follow_for(k: usize, waiting: bool) -> [f32; 2] {
    if waiting { [0.0; 2] } else { [0.4 + 0.01 * k as f32, -0.1] }
}

#[test]
fn every_step_follows_and_credits_as_before() {
    let steps = recorded();
    let (mut old, mut new) = (OldLast { body: None, ask: None }, LastStep::new());
    for (k, step) in steps.iter().enumerate() {
        let (mut old_credit, mut new_credit) = (None, None);
        let (mut old_asked_wait, mut new_asked_wait) = (None, None);
        let old_follow = old_before_step(
            &mut old,
            step.body,
            step.dt,
            || step.wait.map(reason),
            |gate| {
                old_asked_wait = Some(gate.is_some());
                follow_for(k, gate.is_some())
            },
            |b, f, dt| old_credit = Some((b, f, dt)),
        );
        old_asked(&mut old, step.game);
        let new_follow = new.begin(
            step.body,
            step.dt,
            || step.wait,
            |wait| {
                new_asked_wait = Some(wait.is_some());
                assert_eq!(wait.map(reason), step.wait.map(reason), "step {k}: the same reason");
                follow_for(k, wait.is_some())
            },
            |b, f, dt| new_credit = Some((b, f, dt)),
        );
        new.game(step.game);
        assert_eq!(new_follow, old_follow, "step {k}: the velocity to follow");
        assert_eq!(new_asked_wait, old_asked_wait, "step {k}: waiting");
        assert_eq!(format!("{new_credit:?}"), format!("{old_credit:?}"), "step {k}: what the previous step moved");
    }
}
