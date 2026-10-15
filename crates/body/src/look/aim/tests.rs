use super::*;

#[test]
fn the_weapon_point_is_aimed_at_from_where_following_walked_the_camera() {
    // The reticle 10 m ahead of a palm held 0.4 m right of the head; the player walked 1 m
    // forward (-z) and the character followed: from the camera there, the point is 1 m nearer.
    let point = [0.4, 1.4, -10.0];
    assert_eq!(from_camera(point, [0.0, 0.0]), point, "no following: the origin is the camera");
    assert_eq!(from_camera(point, [0.0, -1.0]), [0.4, 1.4, -9.0]);
    // The yaw toward it differs: aimed from the origin it would miss to the side.
    let from_origin = direction_yaw_pitch(point).unwrap().0;
    let walked_sideways = direction_yaw_pitch(from_camera(point, [1.0, 0.0])).unwrap().0;
    assert!((from_origin - walked_sideways).abs() > 0.05, "{from_origin} {walked_sideways}");
}

/// A manifest without an `aim` option: the older switches decide.
const TEST_MANIFEST: monaka_contract::manifest::Manifest = monaka_contract::manifest::Manifest {
    id: "t",
    name: "Test",
    status: monaka_contract::manifest::Status::Experimental,
    executables: &["T.exe"],
    steam_app: 0,
    producer: "monaka_t",
    log: "monaka_t.log",
    options_file: "t-options.txt",
    run_prefix: "t",
    launch: monaka_contract::manifest::Launch::Manual,
    refused_arguments: &[],
    before: "",
    options: &[],
    licences: &[],
    left_hand_aims: true,
    weapon_ring: true,
    controls: &[],
};

#[test]
fn settings_come_from_options() {
    let aim = |text: &str| monaka_core::options::Aim::from_options(&Options::parse(text), &TEST_MANIFEST);
    let s = AimSettings::from_options(&Options::parse("hand_rig=1\naim_hand=Left"), aim("hand_rig=1\naim_hand=Left"), true);
    assert!(s.head_aim && s.hand_aim && s.hand_rig && s.melee_aim_head && s.aim_hand == LEFT_HAND);
    let off = AimSettings::from_options(&Options::default(), aim("head_aim=0\nhand_rig=1"), true);
    assert!(!off.hand_aim && !off.hand_rig, "hand aim and the rig ride on head aim");
    let impossible = AimSettings::from_options(&Options::default(), aim("head_aim=1\nhand_rig=1"), false);
    assert!(!impossible.head_aim && !impossible.hand_rig, "an option cannot turn on what the run cannot do");
    assert_eq!(AimSettings::from_options(&Options::default(), aim(""), false).aim_hand, RIGHT_HAND);
}

#[test]
fn the_head_aims_without_a_controller() {
    let chosen = |text: &str| monaka_core::options::Aim::from_options(&Options::parse(text), &TEST_MANIFEST);
    let s = AimSettings::from_options(&Options::default(), chosen("hand_aim=1"), true);
    assert!(s.tool_aim, "tool aim comes with hand aim");
    let aim = s.aim_at(0, [0.0, 0.0, 0.0, 1.0], |_| None);
    assert!(!aim.by_hand && !aim.tool && aim.yaw.abs() < 1e-6 && aim.pitch.abs() < 1e-6);
    let palm = HandPose { orientation: monaka_core::math::yaw_rotation(0.5), position: [0.0; 3], valid: 1, received_tick: 0 };
    let aim = s.aim_at(0, [0.0, 0.0, 0.0, 1.0], |_| Some(palm));
    assert!(aim.by_hand && !aim.tool && (aim.yaw - 0.5).abs() < 1e-3, "{aim:?}");
    // No pad read: the tool never aims; the left controller is never possible as the tool.
    assert!(!TOOL.aims(&s, 0, || true));
    let left = AimSettings::from_options(&Options::default(), chosen("hand_aim=1\naim_hand=left"), true);
    assert!(!TOOL.possible(&left) && !AimSettings::from_options(&Options::parse("tool_aim=0"), chosen("hand_aim=1"), true).tool_aim);
}

#[test]
fn the_tool_gate_opens_the_update_after_the_aim() {
    let tool = ToolAim::new();
    // Gated without the aim yet: nothing opens; the first update that writes it is remembered.
    tool.gate(true, false, 10);
    assert!(tool.from.lock().unwrap().is_none());
    tool.gate(true, true, 11);
    assert_eq!(*tool.from.lock().unwrap(), Some(11));
    tool.gate(true, true, 12);
    assert_eq!(*tool.from.lock().unwrap(), Some(11), "the first update stays");
    // The tool let go: forgotten; released: the gate is off.
    tool.gate(true, false, 13);
    assert!(tool.from.lock().unwrap().is_none());
    tool.release();
}
