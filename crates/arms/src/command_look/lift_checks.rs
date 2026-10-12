//! Head aim through the input command against the Wolfensteins' own, which it was lifted from.

use super::*;

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
