//! The command delivery against the look through the input command it replaced
//! (`command_look`), and that against the Wolfensteins' own, which it was lifted from.

use super::*;
use crate::look::tests::serial;
use crate::look::StandDown;
use monaka_producer::game::Adapter;

/// The Wolfensteins' share for a command as it was: held over while resuming and aiming, the
/// head's (or the held one without a pose) while aiming and no other camera has the view, else none.
fn next_before(aiming: bool, resuming: bool, controlled: bool, held: Share, head: Option<Share>) -> Share {
    if resuming && aiming {
        held
    } else if aiming && !controlled {
        match head {
            Some(head) => head,
            None => held,
        }
    } else {
        Share::default()
    }
}

/// Their swap of the share in the accumulated angles as it was.
fn reshare_before(pitch: f32, yaw: f32, held: Share, next: Share) -> (f32, f32) {
    let degrees = |s: Share| (s.yaw.to_degrees(), -s.pitch.to_degrees());
    let (held_yaw, held_pitch) = degrees(held);
    let (next_yaw, next_pitch) = degrees(next);
    (pitch - held_pitch + next_pitch, yaw - held_yaw + next_yaw)
}

/// Their measure of the player's own pitch as it was.
fn own_pitch_before(pitch_up: f32, baked: Share) -> f32 {
    let own_down = -(pitch_up - baked.pitch.to_degrees());
    let usable = own_down.is_finite() && baked.pitch.to_degrees().abs() < 30.0;
    if usable { own_down } else { f32::NAN }
}

#[test]
fn each_command_gets_what_it_got_before() {
    let shares = [Share::default(), Share { yaw: 0.4, pitch: -0.2 }, Share { yaw: -1.2, pitch: 0.6 }];
    for aiming in [false, true] {
        for resuming in [false, true] {
            for controlled in [false, true] {
                for held in shares {
                    for head in [None, Some(shares[1]), Some(shares[2])] {
                        assert_eq!(next_share(aiming, resuming, controlled, held, head), next_before(aiming, resuming, controlled, held, head));
                    }
                    for next in shares {
                        assert_eq!(reshare(240.0, -35.5, held, next), reshare_before(240.0, -35.5, held, next));
                    }
                }
            }
        }
    }
    for (pitch_up, baked) in [(25.0, shares[1]), (-40.0, Share { yaw: 0.0, pitch: -40f32.to_radians() }), (5.0, shares[2])] {
        let now = own_pitch(pitch_up, baked);
        let before = own_pitch_before(pitch_up, baked);
        assert!(now.to_bits() == before.to_bits() || (now.is_nan() && before.is_nan()), "{pitch_up} {baked:?}");
    }
}

/// `command_look` as it was: its state, its view's measure, and its `command_built`.
struct Before {
    aiming: bool,
    holding: bool,
    levelling: bool,
    own_pitch: f32,
    held: Share,
    last_command: Option<Instant>,
    holding_after_pause: u32,
    pause_hold: bool,
}

impl Before {
    fn built(&mut self, pitch: f32, yaw: f32, taken: bool, head: Option<HeadPose>) -> Option<(f32, f32)> {
        let aiming = self.aiming;
        if !aiming && !self.holding {
            return None;
        }
        if !pitch.is_finite() || !yaw.is_finite() {
            return None;
        }
        let now = Instant::now();
        if self.last_command.is_some_and(|last| now.duration_since(last) > PAUSE_GAP) && self.pause_hold {
            self.holding_after_pause = PAUSE_HOLD_COMMANDS;
        }
        self.last_command = Some(now);
        let resuming = self.holding_after_pause > 0;
        self.holding_after_pause = self.holding_after_pause.saturating_sub(1);
        let head = if aiming && !resuming && !taken { head.map(|pose| head_share(pose.orientation)) } else { None };
        let next = next_share(aiming, resuming, taken, self.held, head);
        let (mut pitch, yaw) = reshare(pitch, yaw, self.held, next);
        let own = std::mem::replace(&mut self.own_pitch, f32::NAN);
        if aiming && !resuming && self.levelling && !taken && own.is_finite() {
            let change = LEVEL.step(own);
            if change != 0.0 {
                pitch += change;
            }
        }
        self.held = next;
        self.holding = aiming;
        Some((pitch, yaw))
    }

    fn baked(&self, taken: bool) -> Option<Share> {
        (self.holding && !taken).then_some(self.held)
    }
}

/// A game looking through its command: the head and whether another camera has the view, as the
/// sequence sets them.
struct Commanded;
static HEAD: Mutex<Option<HeadPose>> = Mutex::new(None);
static TAKEN: AtomicBool = AtomicBool::new(false);
/// Whether the game's own pad is on (the Wolfensteins levelled only then).
static PAD: AtomicBool = AtomicBool::new(true);
impl monaka_producer::game::Game for Commanded {
    const MANIFESTS: &'static [monaka_core::manifest::Manifest] = &[];
    const ADAPTER: Adapter<Self> = Adapter { components: &[] };
    type Build = ();
    type Frame = ();
    fn prepare(_: &'static monaka_producer::Session, _: &'static monaka_core::manifest::Manifest, _: &monaka_core::options::Options) -> Result<(), monaka_producer::Rejection> {
        Ok(())
    }
}
impl Looks for Commanded {
    const DELIVERY: Delivery<Self> = Delivery::Command { head: || *HEAD.locked(), levels: || PAD.load(Relaxed) };
    fn stand_down(_: usize) -> StandDown {
        StandDown { now: TAKEN.load(Relaxed), changed: false }
    }
}

#[test]
fn a_run_of_commands_writes_the_angles_and_bakes_the_shares_it_did() {
    let _serial = serial();
    let head = |yaw: f32, pitch: f32| Some(HeadPose::synthetic(yaw, pitch, HeadPose::SYNTHETIC_FOV, 0.064));
    *GAME.locked() = Some(GameFns { camera_taken: camera_taken::<Commanded>, head: || *HEAD.locked() });
    PAUSE_HOLD.store(true, Relaxed);
    LEVELLING.store(true, Relaxed);
    AIMING.store(true, Relaxed);
    *STATE.locked() = State { held: Share::default(), commands: 0, aimed: 0, last_command: None, holding_after_pause: 0, pauses: 0 };
    let mut before = Before { aiming: true, holding: false, levelling: true, own_pitch: f32::NAN, held: Share::default(), last_command: None, holding_after_pause: 0, pause_hold: true };
    // Commands: (the game's accumulated pitch and yaw, the head, another camera, the camera's pitch
    // at the view before, a pause before it, aiming).
    let commands = [
        (240.0, -35.0, head(20.0, 10.0), false, Some(12.0), false, true),
        (241.0, -30.0, head(25.0, -5.0), false, Some(-3.0), false, true),
        (241.0, -30.0, head(25.0, -5.0), true, None, false, true),
        (238.0, -31.0, head(-60.0, 20.0), false, Some(40.0), false, true),
        (238.0, -31.0, head(-62.0, 21.0), false, Some(1.0), true, true),
        (238.0, -31.0, head(-64.0, 22.0), false, None, false, true),
        (238.0, -31.0, head(-66.0, 23.0), false, Some(0.5), false, true),
        (238.0, -31.0, None, false, Some(0.5), false, true),
        (238.0, f32::NAN, head(10.0, 0.0), false, None, false, true),
        (238.0, -31.0, head(10.0, 0.0), false, None, false, false),
        (238.0, -31.0, head(10.0, 0.0), false, None, false, false),
    ];
    for (i, (pitch, yaw, pose, taken, camera_pitch, pause, aiming)) in commands.into_iter().enumerate() {
        *HEAD.locked() = pose;
        TAKEN.store(taken, Relaxed);
        AIMING.store(aiming, Relaxed);
        before.aiming = aiming;
        if let Some(pitch_up) = camera_pitch {
            let baked = command_baked();
            assert_eq!(baked, before.baked(taken), "the share the view of command {i} swapped");
            note_camera_pitch(pitch_up, baked.unwrap_or_default());
            before.own_pitch = own_pitch(pitch_up, before.baked(taken).unwrap_or_default());
        }
        if pause {
            std::thread::sleep(PAUSE_GAP + Duration::from_millis(50));
        }
        let (now, then) = (built(pitch, yaw), before.built(pitch, yaw, taken, pose));
        assert_eq!(now.map(|(p, y)| (p.to_bits(), y.to_bits())), then.map(|(p, y)| (p.to_bits(), y.to_bits())), "the angles written at command {i}");
        assert_eq!(command_baked(), before.baked(taken), "the share after command {i}");
    }
    AIMING.store(false, Relaxed);
}

#[test]
fn steering_levels_only_where_the_wolfensteins_did() {
    let _serial = serial();
    for pad in [false, true] {
        PAD.store(pad, Relaxed);
        arm::<Commanded>();
        // The Wolfensteins' arm: levelling with their pad on and the head alone pitching.
        let before = pad && monaka_pad::feature::pitch_from_head();
        assert_eq!(LEVELLING.load(Relaxed), before, "pad {pad}");
        quiet();
    }
    PAD.store(true, Relaxed);
}

/// The share the look's eye site reads for the command game.
fn command_baked() -> Option<Share> {
    crate::look::baked::<Commanded>(None)
}
