use super::*;
use monaka_contract::manifest::{self, Launch, Status};

#[test]
fn sizes_and_angles() {
    let options = Options::parse("eye_size=2644x2644\nsynthetic_yaw=-30\nfar=270");
    assert_eq!(options.size("eye_size"), Some((2644, 2644)));
    assert_eq!(options.angle("synthetic_yaw", 180.0), Some(-30.0));
    assert_eq!(options.angle("far", 180.0), None);
    assert_eq!(options.keys_read(), ["eye_size", "synthetic_yaw", "far"]);
}

#[test]
fn parses_numbers_switches_and_lists() {
    let options = Options::parse("# comment\nhud_scale = 0.36\nlatency=3\nlatency=2\nsynthetic=on\nbad=x\n\nnot a pair\nhold=0.01, 0,-0.02\naim_hand=Left\nhead_position=0");
    assert_eq!(options.number("hud_scale", 0.2, 1.0, 1.0), 0.36);
    assert_eq!(options.number("hud_scale", 0.5, 1.0, 1.0), 1.0, "out of range falls back");
    assert_eq!(options.value::<u64>("latency"), Some(2), "the later line wins");
    assert!(options.switch("synthetic", false));
    assert!(options.switch("missing", true));
    assert_eq!(options.number("bad", 0.0, 1.0, 0.5), 0.5);
    assert_eq!(options.floats::<3>("hold", 1.0), Some([0.01, 0.0, -0.02]));
    assert_eq!(options.floats::<3>("hold", 0.001), None);
    assert_eq!(options.floats::<2>("hold", 1.0), None);
    assert_eq!(options.hand("aim_hand"), LEFT_HAND);
    assert_eq!(options.hand("missing"), RIGHT_HAND);
    assert_eq!(options.head_position("head_position", HeadPosition::Lean), HeadPosition::Off);
    assert_eq!(options.choice("latency", &[("2", 'b'), ("3", 'c')], 'z'), 'b');
}

const GAME: Manifest = Manifest {
    id: "g",
    name: "Game",
    status: Status::Alpha,
    executables: &["G.exe"],
    steam_app: 1,
    producer: "monaka_g",
    log: "monaka_g.log",
    options_file: "g-options.txt",
    run_prefix: "g",
    launch: Launch::Manual,
    refused_arguments: &[],
    before: "",
    options: &[manifest::aim(&[manifest::AIM_MOTION, manifest::AIM_LOOK], "hand_rig"), manifest::HUD_SIZE.default_number(0.36), manifest::LATENCY.default_number(1.0), manifest::WORLD_HUD.default_switch(false)],
    licences: &[],
    left_hand_aims: false,
    weapon_ring: false,
    controls: &[],
};

#[test]
fn shared_takes_the_manifests_defaults() {
    let shared = Shared::from_options(&Options::parse(""), &GAME);
    assert_eq!(shared.schedule.latency, 1);
    assert_eq!(shared.hud.scale, 0.36);
    assert!(!shared.hud.world && shared.hud.markers && shared.hud.shown);
    assert_eq!(shared.hud.distance, 2.0);
    assert_eq!(shared.aim, Aim { head: true, hand: true, rig: true, side: RIGHT_HAND }, "the manifest's aim default");
    assert!(shared.head_tracking && shared.headset_fov && shared.controller_pad && shared.fingers.tracking && !shared.fingers.binary);
    assert_eq!(shared.head_position, HeadPosition::Lean);
    assert_eq!(shared.synthetic, None);
    let shared = Shared::from_options(&Options::parse("aim=Controller\naim_hand=left\nlatency=9\nsynthetic_pitch=10\nheadset_projection=0\neye_separation=0.5\ntest_separation=0.3"), &GAME);
    assert_eq!(shared.aim, Aim { head: true, hand: true, rig: false, side: LEFT_HAND });
    assert_eq!(shared.schedule.latency, 1, "out of range keeps the game's own");
    assert_eq!(shared.synthetic, Some((0.0, 10.0)));
    assert!(!shared.headset_fov, "the older name still works");
    assert_eq!(shared.eye_separation, 0.064, "out of range falls back");
    assert_eq!(shared.separation_for(&HeadPose::default()), 0.3);
    assert_eq!(Shared::from_options(&Options::parse("aim=off"), &GAME).aim, Aim::OFF);
    // The older switches, without an `aim`: the rig implies hand aim, nothing without head aim.
    let old = Shared::from_options(&Options::parse("hand_rig=1"), &GAME).aim;
    assert!(old.head && old.hand && old.rig);
    assert_eq!(Shared::from_options(&Options::parse("hand_rig=1\nhead_aim=0"), &GAME).aim, Aim::OFF);
}

#[test]
fn rendered_frustums() {
    let shared = Shared::from_options(&Options::parse(""), &GAME);
    let pose = shared.synthetic_pose(0.0, 0.0);
    let own = shared.rendered_frustum(&pose, 0, false, None).unwrap();
    assert!(own.straight_ahead()[0] > 0.0, "the headset's own off-centre frustum");
    let centred = shared.rendered_frustum(&pose, 0, true, Some([1.4, 0.9])).unwrap();
    assert_eq!(centred.straight_ahead(), [0.0, 0.0]);
    assert!((centred.right - 0.87f32.tan()).abs() < 1e-5, "the outer edge decides: {centred:?}");
    let game = shared.rendered_frustum(&HeadPose::default(), 1, true, Some([1.3963, 0.882])).unwrap();
    let angles = full_angles(&game);
    assert!((angles[0] - 1.3963).abs() < 1e-4 && (angles[1] - 0.882).abs() < 1e-4, "{angles:?}");
    assert!(shared.rendered_frustum(&HeadPose::default(), 0, false, None).is_none());
    assert_eq!(shared.separation_for(&HeadPose { ipd: 0.07, ..pose }), 0.07);
    assert_eq!(shared.separation_for(&HeadPose { ipd: 0.2, ..pose }), 0.064);
    let yawed = shared.synthetic_pose(90.0, 0.0);
    assert!((crate::math::yaw(yawed.orientation) - core::f32::consts::FRAC_PI_2).abs() < 1e-5);
}

#[test]
fn unread_options_are_found() {
    const GAME: manifest::Manifest = manifest::Manifest {
        options: &[manifest::aim(&[manifest::AIM_MOTION, manifest::AIM_POINT], "hand_rig"), manifest::AIM_HAND, manifest::HUD_SIZE, manifest::FINGER_TRACKING, manifest::LATENCY],
        ..manifest::Manifest::GAME
    };
    let unread = unread_options(&GAME, |o| {
        o.text("aim");
        o.number("hud_scale", 0.0, 1.0, 0.5);
    });
    assert_eq!(unread, ["aim_hand", "finger_tracking", "latency"]);
}
