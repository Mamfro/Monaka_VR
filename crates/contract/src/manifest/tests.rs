use super::*;

const TEST: Manifest = Manifest {
    id: "t",
    name: "Test",
    status: Status::Alpha,
    executables: &["A.exe", "B.exe"],
    steam_app: 1,
    producer: "monaka_t",
    log: "monaka_t.log",
    options_file: "t-options.txt",
    run_prefix: "t",
    launch: Launch::Auto { settle_seconds: 5 },
    refused_arguments: &[],
    before: "",
    options: &[aim(&[AIM_MOTION, AIM_LOOK, AIM_POINT, AIM_MOUSE], "hand_rig"), AIM_HAND, RENDER_SIZE, HUD_SIZE.default_number(0.36), FINGER_TRACKING, LATENCY, EXTRA, LAUNCH, settle(60.0, 5.0)],
    licences: &[],
    left_hand_aims: true,
    weapon_ring: true,
    controls: &[],
};

const ANY: Manifest = Manifest {
    id: "any",
    name: "Any game",
    executables: &[ANY_EXECUTABLE],
    launch: Launch::Manual,
    options: &[
        opt(GAME_OPTION, "Game", "", choice(&[("A.exe", "A"), (OTHER_GAME, "Other")], "A.exe")),
        opt(OTHER_GAME_OPTION, "Other game", "", Kind::Text).only_when(GAME_OPTION, OTHER_GAME),
    ],
    ..TEST
};

#[test]
fn a_pack_for_any_game_runs_as_the_one_picked() {
    ANY.check().unwrap();
    assert_eq!(TEST.runs_as(&Values::default()), ["A.exe", "B.exe"]);
    let mut values = Values::default();
    assert_eq!(ANY.runs_as(&values), ["A.exe"]);
    values.set(GAME_OPTION, OTHER_GAME);
    assert!(ANY.runs_as(&values).is_empty(), "nothing typed yet");
    values.set(OTHER_GAME_OPTION, r#" "D:\Games\Some Game\Binaries\Win32\Game.exe" "#);
    assert_eq!(ANY.runs_as(&values), ["Game.exe"]);
    let without_picker = Manifest { options: &[], ..ANY };
    assert!(without_picker.check().is_err());
}

#[test]
fn a_text_option_reaches_the_producer_only_when_it_holds_some() {
    const NAMES: Manifest = Manifest { options: &[line("hide", "Hide", "", Kind::Text)], ..TEST };
    let mut values = Values::default();
    assert!(values.producer_lines(&NAMES).is_empty());
    values.set("hide", " Arms,Legs ");
    assert_eq!(values.producer_lines(&NAMES), ["hide=Arms,Legs"]);
}

#[test]
fn values_read_as_defaults_and_make_lines() {
    TEST.check().unwrap();
    let mut values = Values::default();
    assert_eq!(values.text(&TEST, "aim"), "hand_rig");
    assert_eq!(values.text(&TEST, "hud_scale"), "0.36");
    assert!(values.set_checked(&TEST, "hud_scale", "7").is_err());
    assert!(values.set_checked(&TEST, "nope", "1").is_err());
    values.set_checked(&TEST, "aim", "CONTROLLER").unwrap();
    values.set_checked(&TEST, "extra", "probe_x=1 junk").unwrap();
    assert_eq!(values.producer_lines(&TEST), ["aim=controller", "aim_hand=right", "hud_scale=0.36", "latency=2", "probe_x=1"]);
    // Finger tracking only with the rig; the aiming hand only when a controller aims.
    values.set("aim", "hand_rig");
    assert_eq!(values.producer_lines(&TEST)[..3], ["aim=hand_rig", "hud_scale=0.36", "finger_tracking=1"]);
    assert_eq!(host_text(&[TEST]), "A.exe;B.exe");
    const N: usize = host_len(&[TEST]);
    assert_eq!(&host_bytes::<N>(&[TEST]), b"A.exe;B.exe\0");
}

#[test]
fn a_developer_option_is_at_its_default_without_developer_mode() {
    const TRIAL: Opt = opt("trial", "Trial", "", Kind::Switch { default: false }).dev();
    const TRIED: Manifest = Manifest { options: &[TRIAL, WORLD_HUD], ..TEST };
    let mut saved = Values::default();
    saved.set("trial", "1");
    saved.set("world_hud", "0");
    let run = saved.for_run(&[&TRIED, &MONAKA]);
    assert!(!run.switch(&TRIED, "trial"), "hidden, at its default");
    assert!(!run.switch(&TRIED, "world_hud"), "other options as saved");
    saved.set(DEV_MODE.id, "1");
    assert!(saved.for_run(&[&TRIED, &MONAKA]).switch(&TRIED, "trial"), "developer mode: as saved");
}

#[test]
fn sizes_read_either_way() {
    assert_eq!(parse_size("2644x2644"), Some((2644, 2644)));
    assert_eq!(parse_size(" 2416 2644\n"), Some((2416, 2644)));
    assert_eq!(parse_size("100x2644"), None);
    assert_eq!(parse_size("2644"), None);
}

#[test]
fn checks_catch_bad_tables() {
    // A switch turned off by another option: off in the values and in the file, whatever it is
    // set to; the rule must name a switch.
    const RULED: &[Opt] = &[WORLD_HUD, opt("fast", "Fast", "", Kind::Switch { default: false }).turns_off("1", "world_hud", "Not with fast.")];
    let ruled = Manifest { options: RULED, launch: Launch::Manual, ..TEST };
    ruled.check().unwrap();
    let mut values = Values::default();
    assert!(values.switch(&ruled, "world_hud"));
    values.set("fast", "1");
    assert!(!values.switch(&ruled, "world_hud") && values.turned_off(&ruled, "world_hud").is_some_and(|(cause, _)| cause.id == "fast"));
    assert!(values.producer_lines(&ruled).contains(&"world_hud=0".to_owned()));
    assert!(ruled.turned_off("world_hud", |id| (id == "fast").then(|| "on".to_owned())).is_some());
    const WRONG: &[Opt] = &[HUD_SIZE, opt("fast", "Fast", "", Kind::Switch { default: false }).turns_off("1", "hud_scale", "")];
    assert!(Manifest { options: WRONG, launch: Launch::Manual, ..TEST }.check().unwrap_err().contains("not a switch"));
    const BAD_OPTIONS: &[Opt] = &[HUD_SIZE.default_number(7.0)];
    let bad = Manifest { options: BAD_OPTIONS, ..TEST };
    assert!(bad.check().unwrap_err().contains("hud_scale"));
    let manual = Manifest { launch: Launch::Manual, ..TEST };
    assert!(manual.check().unwrap_err().contains("launch"));
    // Monaka's own settings belong to no game.
    assert!(Manifest { id: "monaka", ..TEST }.check().unwrap_err().contains("Monaka"));
    const OWN: &[Opt] = &[CONTROLLER_SMOOTHING];
    assert!(Manifest { options: OWN, launch: Launch::Manual, ..TEST }.check().unwrap_err().contains("controller_smoothing"));
    // One its producer reads, listed as Monaka has it, is the game's to read; changed, it is not.
    const READS: &[Opt] = &[DISABLE_VIEW_BOBBING];
    assert!(Manifest { options: READS, launch: Launch::Manual, ..TEST }.check().is_ok());
    const CHANGED: &[Opt] = &[DISABLE_VIEW_BOBBING.default_switch(false)];
    assert!(Manifest { options: CHANGED, launch: Launch::Manual, ..TEST }.check().unwrap_err().contains("disable_view_bobbing"));
}

/// A game with the Controls tab: what the player holds, the aims, the pad, and its how-tos.
const PLAYS: Manifest = Manifest {
    options: &[PLAY_WITH, aim(&[AIM_MOTION, AIM_LOOK, AIM_POINT, AIM_MOUSE], aims::HAND_RIG), CONTROLLER_PAD, CONTROLLER_PROXY, ROOM_SCALE],
    launch: Launch::Manual,
    controls: &[HOW_BUTTONS, HOW_WALK],
    ..TEST
};

#[test]
fn what_the_player_holds_decides_the_aims_and_the_pad() {
    PLAYS.check().unwrap();
    let aim = PLAYS.option("aim").unwrap();
    let mut values = Values::default();
    assert_eq!(values.shown_choices(&PLAYS, aim).len(), 4);
    assert!(values.switch(&PLAYS, "controller_pad"));
    // A mouse and keyboard make neither motion controls nor a pointing controller: the aim the
    // player set gives way to the first the device makes, and their VR controllers are no pad.
    values.set_checked(&PLAYS, "play_with", "keyboard").unwrap();
    assert_eq!(values.shown_choices(&PLAYS, aim).iter().map(|(id, _)| *id).collect::<Vec<_>>(), [aims::HEAD, aims::OFF]);
    assert_eq!(values.text(&PLAYS, "aim"), aims::HEAD);
    values.set_checked(&PLAYS, "aim", "off").unwrap();
    assert_eq!(values.text(&PLAYS, "aim"), aims::OFF);
    assert!(!values.switch(&PLAYS, "controller_pad"));
    assert!(values.producer_lines(&PLAYS).contains(&"controller_pad=0".to_owned()));
    assert!(!values.producer_lines(&PLAYS).iter().any(|l| l.starts_with("play_with")), "the launcher's own");
    // Back on the VR controllers, the aim set is theirs again.
    values.set_checked(&PLAYS, "aim", "controller").unwrap();
    assert_eq!(values.text(&PLAYS, "aim"), aims::HEAD);
    values.set("play_with", "vr");
    assert_eq!(values.text(&PLAYS, "aim"), aims::CONTROLLER);
    // A game without the choice aims as it always did.
    assert_eq!(values.text(&TEST, "aim"), aims::CONTROLLER);
}

#[test]
fn a_how_to_shows_the_ways_that_hold() {
    let mut values = Values::default();
    let buttons = values.ways(&PLAYS, &HOW_BUTTONS);
    assert!(buttons.len() == 2 && buttons[0].contains("LB/RB") && buttons[1].contains("thumbrest"), "{buttons:?}");
    values.set("controller_pad", "0");
    assert!(values.ways(&PLAYS, &HOW_BUTTONS).is_empty());
    values.set("play_with", "gamepad");
    assert_eq!(values.ways(&PLAYS, &HOW_BUTTONS), ["As in the game."]);
    assert!(values.ways(&PLAYS, &HOW_WALK).is_empty());
    assert!(matches("keyboard", "!vr") && !matches("vr", "!vr") && matches("vr", "vr"));
    // Every option a how-to names is the game's, under one how-to only; every condition one of
    // its values.
    let unknown = Manifest { options: &[PLAY_WITH, CONTROLLER_PAD], ..PLAYS };
    assert!(unknown.check().unwrap_err().contains("unknown option"));
    const TWICE: &[HowTo] = &[HOW_WALK, HowTo { action: "Again", ways: &[], options: &["room_scale"] }];
    assert!(Manifest { controls: TWICE, ..PLAYS }.check().unwrap_err().contains("under two controls"));
    const ODD: &[HowTo] = &[HowTo { action: "Odd", ways: &[way("x", &[("play_with", "!feet")])], options: &[] }];
    assert!(Manifest { controls: ODD, ..PLAYS }.check().unwrap_err().contains("play_with=!feet"));
}

#[test]
fn monaka_settings_read_as_options() {
    let mut values = Values::default();
    assert_eq!(values.text(&MONAKA, "controller_smoothing"), "3");
    values.set_checked(&MONAKA, "controller_smoothing", "0").unwrap();
    assert_eq!(values.number(&MONAKA, "controller_smoothing"), 0.0);
    assert!(values.set_checked(&MONAKA, "controller_smoothing", "11").is_err());
    assert!(MONAKA.options.iter().all(|o| o.shown == Shown::Monaka));
    // A game that reads one gets Monaka's value, the one saved for every game.
    values.set_checked(&MONAKA, "disable_view_bobbing", "0").unwrap();
    const READS: &[Opt] = &[DISABLE_VIEW_BOBBING];
    let game = Manifest { options: READS, ..TEST };
    assert_eq!(values.producer_lines(&game), ["disable_view_bobbing=0"]);
}
