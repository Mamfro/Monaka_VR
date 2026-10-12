//! The owned facing against Dying Light 1's own head aim, which it was lifted from: its state's
//! steps, update by update.

use super::*;

/// One update's inputs (radians), as the hooks and the pad give them.
#[derive(Clone, Copy)]
struct Input {
    character: usize,
    now: u64,
    camera_yaw: f32,
    head_yaw: f32,
    aim_yaw: f32,
    cutscene: bool,
    spot: Option<f32>,
    climbing: bool,
    climb_stick: Option<f32>,
    stick: Option<f32>,
}

/// What an update did: the facing it left, the push while climbing, and the yaw the camera of
/// that update carries.
#[derive(Debug, PartialEq)]
enum Out {
    Movie,
    Climb { facing: f32, push: f32, off: f32, thumb: f32, began: bool },
    Steer { facing: f32, ended: bool },
}

/// DL1's head aim state as it was, its own update.
struct Before {
    character: usize,
    baked: BakedYaw,
    climb: Option<Climb>,
    facing: Option<f32>,
    movie_head: Option<f32>,
    turned_at: u64,
    turn_latch: TurnLatch,
    movie_on: bool,
    frames: u64,
}

impl Before {
    fn new() -> Self {
        Self { character: 0, baked: BakedYaw::EMPTY, climb: None, facing: None, movie_head: None, turned_at: 0, turn_latch: TurnLatch::new(), movie_on: false, frames: 0 }
    }

    fn update(&mut self, i: Input, speed: f32) -> Out {
        let Input { character, now, camera_yaw, head_yaw, aim_yaw, .. } = i;
        self.frames += 1;
        let frame = self.frames;
        let step = now.saturating_sub(self.turned_at).min(MAX_STEP_MS) as f32 / 1000.0;
        self.turned_at = now;
        if self.character != character {
            self.character = character;
            self.facing = None;
            self.climb = None;
        }
        let movie = i.cutscene;
        let changed = std::mem::replace(&mut self.movie_on, movie) != movie;
        if changed {
            if movie {
                self.movie_head = Some(head_yaw);
            } else if let Some(began) = self.movie_head.take() {
                self.facing = Some(wrapped(camera_yaw - began));
            }
        }
        if movie {
            let began = *self.movie_head.get_or_insert(head_yaw);
            self.baked.record(frame, began);
            return Out::Movie;
        }
        let mut facing = *self.facing.get_or_insert(camera_yaw);
        if let Some(spot) = i.spot {
            facing = spot;
        }
        if i.climbing {
            let began = self.climb.is_none();
            let mut climb = self.climb.unwrap_or(Climb { camera: camera_yaw, push: 0.0, traced: 0 });
            let turned = wrapped(camera_yaw - climb.camera);
            climb.camera = camera_yaw;
            let thumb = i.climb_stick.map_or(0.0, |s| s - climb.push);
            let off = wrapped(facing + aim_yaw - camera_yaw);
            if thumb.abs() > STICK_TURNS {
                facing = wrapped(facing + turned);
                climb.push = 0.0;
            } else {
                climb.push = push_for(off);
            }
            self.facing = Some(facing);
            self.baked.record(frame, wrapped(camera_yaw - facing));
            self.climb = Some(climb);
            return Out::Climb { facing, push: climb.push, off, thumb, began };
        }
        let ended = self.climb.take().is_some();
        if let Some(x) = i.stick {
            facing = wrapped(facing - StickTurn::smooth(speed).turn(x, step, &mut self.turn_latch).to_radians());
        }
        self.facing = Some(facing);
        self.baked.record(frame, wrapped(camera_yaw - facing));
        Out::Steer { facing, ended }
    }
}

/// The lifted state, stepped as [`steer`] steps it.
struct After {
    state: State,
    movie_on: bool,
    frames: u64,
}

impl After {
    fn update(&mut self, i: Input, speed: f32) -> Out {
        self.frames += 1;
        let frame = self.frames;
        let state = &mut self.state;
        let step = state.stepped(i.now);
        let asked = StandDown { now: i.cutscene, changed: std::mem::replace(&mut self.movie_on, i.cutscene) != i.cutscene };
        state.meet(i.character, asked, i.head_yaw, i.camera_yaw);
        if asked.now {
            state.hold_movie(frame, i.head_yaw);
            return Out::Movie;
        }
        let mut facing = *state.facing.get_or_insert(i.camera_yaw);
        if let Some(spot) = i.spot {
            facing = spot;
        }
        if i.climbing {
            let Climbing { climb, began, facing, off, thumb } = state.climb(frame, i.camera_yaw, facing, i.aim_yaw, i.climb_stick);
            state.climb = Some(climb);
            return Out::Climb { facing, push: climb.push, off, thumb, began };
        }
        let ended = state.climb.take().is_some();
        let facing = state.turn(frame, i.camera_yaw, facing, i.stick, speed, step);
        Out::Steer { facing, ended }
    }
}

#[test]
fn each_update_faces_as_dl1s_did() {
    let mut before = Before::new();
    let mut after = After { state: State::new(), movie_on: false, frames: 0 };
    let base = Input { character: 0x10, now: 1000, camera_yaw: 0.4, head_yaw: 0.0, aim_yaw: 0.0, cutscene: false, spot: None, climbing: false, climb_stick: None, stick: None };
    let mut inputs = vec![
        base,
        Input { now: 1011, head_yaw: 0.3, aim_yaw: 0.3, ..base },
        Input { now: 1022, camera_yaw: 0.7, head_yaw: 0.3, aim_yaw: 0.5, stick: Some(0.8), ..base },
        Input { now: 1300, camera_yaw: 0.7, stick: Some(-1.0), ..base },
        Input { now: 1311, camera_yaw: -2.0, head_yaw: 0.6, cutscene: true, ..base },
        Input { now: 1322, camera_yaw: -2.2, head_yaw: 0.1, cutscene: true, ..base },
        Input { now: 1333, camera_yaw: -2.5, head_yaw: 0.1, ..base },
        Input { now: 1344, camera_yaw: -2.5, spot: Some(1.0), ..base },
    ];
    // Climbing: the head pushes, then the player's own stick turns, then the climb ends.
    for (k, (camera, stick)) in [(1.0, None), (1.1, Some(0.1)), (1.3, Some(0.9)), (1.6, Some(0.95)), (1.6, Some(0.0))].into_iter().enumerate() {
        inputs.push(Input { now: 1400 + 11 * k as u64, camera_yaw: camera, head_yaw: 0.4, aim_yaw: 0.4, climbing: true, climb_stick: stick, ..base });
    }
    inputs.push(Input { now: 1500, camera_yaw: 1.6, stick: Some(0.5), ..base });
    // A new character (a load) faces where its camera does.
    inputs.push(Input { character: 0x20, now: 1511, camera_yaw: 3.0, ..base });
    inputs.push(Input { character: 0x20, now: 1522, camera_yaw: 3.1, head_yaw: -0.2, aim_yaw: -0.25, stick: Some(0.3), ..base });
    for (n, input) in inputs.into_iter().enumerate() {
        let (a, b) = (after.update(input, 90.0), before.update(input, 90.0));
        assert_eq!(a, b, "update {n}");
        assert_eq!(after.state.facing, before.facing, "the facing after update {n}");
        assert_eq!(after.state.climb, before.climb.map(|c| Climb { traced: 0, ..c }), "the climb after update {n}");
        let frame = n as u64 + 1;
        assert_eq!(after.state.baked.lookup(frame), before.baked.lookup(frame), "the camera's yaw of update {n}");
    }
}
