use super::*;
use monaka_core::manifest::*;
use monaka_core::pack::{About, CONTRACT};

/// A game with one of each thing the plan reads: the aim and its hand, a render size, a
/// switch that needs the eye size and another game's files, a choice that passes viewer
/// arguments, a note shown with the eye size, and a command-line-only extra.
const GAME: Manifest = Manifest {
    id: "t",
    name: "Test",
    status: Status::Experimental,
    executables: &["t.exe"],
    steam_app: 1,
    producer: "monaka_t",
    log: "t.log",
    options_file: "t-options.txt",
    run_prefix: "t",
    launch: Launch::Manual,
    refused_arguments: &[],
    before: "",
    options: &[
        aim(&[AIM_MOTION, AIM_LOOK, AIM_POINT, AIM_MOUSE], aims::HAND_RIG),
        AIM_HAND,
        RENDER_SIZE,
        opt("upscale", "Upscale", "Needs both.", Kind::Switch { default: false }).on(Shown::Rendering).needs_eye_size().needs_steam_files(SteamFiles {
            game_folder: "Other Game",
            path: r"bin\x64",
            file: "a.dll",
            line: "a_dlls",
            missing: "No A.",
        }),
        opt("mode", "Mode", "", choice(&[("plain", "Plain"), ("deep", "Deep")], "plain")).viewer_args("deep", &["--panel-nearest", "0.5"]).eye_size_note("Rendering at {size}."),
        EXTRA,
        OTHER_BUILDS,
    ],
    licences: &["T-LICENSE.txt"],
    left_hand_aims: true,
    weapon_ring: true,
    controls: &[],
};

fn parsed(args: &[&str]) -> Values {
    let mut values = Values::default();
    for arg in args {
        let (id, value) = arg.split_once('=').unwrap();
        values.set_checked(&GAME, id, value).unwrap_or_else(|why| panic!("{why}"));
    }
    values
}

#[test]
fn the_plan_writes_what_the_manifest_declares() {
    GAME.check().unwrap();
    let plan = plan(&Values::default(), &GAME, Some((2000, 2000)));
    assert_eq!(plan.files, [("t-options.txt".to_owned(), "aim=hand_rig\nupscale=0\nmode=plain\nother_builds=0\neye_size=2000x2000".to_owned())]);
    assert_eq!(plan.licences, ["T-LICENSE.txt"]);
    assert_eq!(plan.hints, ["Rendering at 2000x2000."]);
    // Extra pairs go last, junk dropped.
    let text = &super::plan(&parsed(&["extra=probe=1 junk x=0,1"]), &GAME, None).files[0].1;
    assert!(text.ends_with("other_builds=0\nprobe=1\nx=0,1"), "{text}");
}

#[test]
fn a_switch_is_written_off_when_its_needs_are_not_met() {
    let on = parsed(&["upscale=on"]);
    let found = |_: &SteamFiles| Some(PathBuf::from(r"D:\Other Game\bin\x64"));
    let both = plan_with(&on, &GAME, Some((2000, 2000)), found);
    assert!(both.files[0].1.contains("upscale=1") && both.files[0].1.ends_with(r"a_dlls=D:\Other Game\bin\x64"), "{}", both.files[0].1);
    let missing = plan_with(&on, &GAME, Some((2000, 2000)), |_| None);
    assert!(missing.files[0].1.contains("upscale=0") && missing.hints.contains(&"No A.".to_owned()), "{:?}", missing);
    let no_eye = plan_with(&on, &GAME, None, found);
    assert!(no_eye.files[0].1.contains("upscale=0") && no_eye.hints[0].contains("headset resolution"), "{:?}", no_eye.hints);
}

#[test]
fn rings_follow_the_aim() {
    let args = |a: &[&str]| plan(&parsed(a), &GAME, None).viewer_args;
    assert_eq!(args(&["aim=hand_rig"]), ["--reticle", "right", "--reticle", "left", "--reticle-from", "palm"]);
    assert_eq!(args(&["aim=controller"]), ["--reticle", "right", "--reticle", "left"]);
    assert_eq!(args(&["aim=controller", "aim_hand=left"]), ["--reticle", "left"]);
    assert!(args(&["aim=head"]).is_empty() && args(&["aim=off"]).is_empty());
    // A choice's viewer arguments.
    assert_eq!(args(&["aim=head", "mode=deep"]), ["--panel-nearest", "0.5"]);
    // Monaka's own settings reach the viewer in every game's run, at any value but the default.
    let mut smoother = parsed(&["aim=head"]);
    smoother.set_checked(&MONAKA, "controller_smoothing", "6").unwrap();
    assert_eq!(plan(&smoother, &GAME, None).viewer_args, ["--controller-smoothing", "6"]);
    assert!(!plan(&smoother, &GAME, None).files[0].1.contains("smoothing"), "not the producer's");
    // And to the headset's Monaka menu, at any value.
    assert_eq!(plan(&smoother, &GAME, None).live, ["controller_smoothing=6"]);
    assert_eq!(plan(&parsed(&["aim=head"]), &GAME, None).live, ["controller_smoothing=3"]);
}

#[test]
fn a_pointing_aim_without_a_hand_choice_rings_the_right_hand() {
    const NO_HAND: Manifest = Manifest { options: &[aim(&[AIM_POINT, AIM_MOUSE], aims::CONTROLLER)], left_hand_aims: false, ..GAME };
    let mut values = Values::default();
    values.set_checked(&NO_HAND, "aim", "controller").unwrap();
    assert_eq!(plan(&values, &NO_HAND, None).viewer_args, ["--reticle", "right"]);
}

#[test]
fn the_eye_size_is_the_players_or_the_render_sizes() {
    assert_eq!(eye_size(&Values::default(), &GAME), EyeSize::Headset);
    assert_eq!(eye_size(&parsed(&["render_size=game"]), &GAME), EyeSize::GameOwn);
    assert_eq!(eye_size(&parsed(&["render_size=2160x2160"]), &GAME), EyeSize::Fixed(2160, 2160));
    let without = Manifest { options: &[], ..GAME };
    assert_eq!(eye_size(&Values::default(), &without), EyeSize::GameOwn);
}

/// Stands in for reading a DLL: a file whose text is `pack` is a pack of [`GAME`]; anything
/// else is unreadable.
fn fake(dll: &Path) -> Result<Pack, String> {
    match std::fs::read_to_string(dll).map_err(|e| e.to_string())?.as_str() {
        "pack" => Ok(Pack { dll: dll.to_owned(), contract: CONTRACT, about: About { author: String::new(), source: "https://x".into(), ..About::default() }, manifests: vec![GAME] }),
        _ => Err(format!("{} is not a game pack", dll.display())),
    }
}

#[test]
fn local_builds_come_from_the_dev_list_and_packs_from_the_games_folder() {
    let dir = std::env::temp_dir().join(format!("monaka-catalogue-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let list = dir.join(r"local\dev-games.txt");
    let paths = Paths { binaries: dir.join("bin"), runs: dir.join("runs"), licences: dir.join("licenses"), packs: dir.join("games"), dev_games: Some(list.clone()) };
    std::fs::create_dir_all(&paths.binaries).unwrap();
    std::fs::create_dir_all(list.parent().unwrap()).unwrap();
    let scan_none = |_: &Path| packs::Scan::default();
    // No list, no games.
    assert!(catalogue_with(&paths, fake, scan_none).games.is_empty());
    std::fs::write(paths.binaries.join("monaka_t.dll"), "pack").unwrap();
    std::fs::write(paths.binaries.join("junk.dll"), "junk").unwrap();
    std::fs::write(&list, "# my builds\nmonaka_t.dll\n\njunk.dll\n").unwrap();
    let catalogue = catalogue_with(&paths, fake, scan_none);
    assert_eq!(catalogue.games.iter().map(|g| g.id).collect::<Vec<_>>(), ["t"]);
    assert_eq!(catalogue.problems.len(), 1, "{:?}", catalogue.problems);
    assert!(catalogue.problems[0].contains("junk.dll is not a game pack"));
    let t = &catalogue.games[0];
    assert!(!t.removable() && t.dll(&paths) == paths.binaries.join("monaka_t.dll") && t.licence_folder(&paths) == paths.licences);
    assert!(t.credit().starts_with("Built on this PC"), "{}", t.credit());
    // The same game from a pack is left out while the local build offers it.
    let pack_dir = paths.packs.join("monaka_t");
    std::fs::create_dir_all(&pack_dir).unwrap();
    std::fs::write(pack_dir.join("monaka_t.dll"), "pack").unwrap();
    let scan_one = |_: &Path| packs::Scan { packs: vec![fake(&pack_dir.join("monaka_t.dll")).unwrap()], ..Default::default() };
    let both = catalogue_with(&paths, fake, scan_one);
    assert_eq!(both.games.len(), 1);
    assert!(both.problems.iter().any(|p| p.contains("already offered by a local build")), "{:?}", both.problems);
    // Without the local build, the pack's game is offered, and can be removed.
    std::fs::write(&list, "").unwrap();
    let packed = catalogue_with(&paths, fake, scan_one);
    assert!(packed.games.len() == 1 && packed.games[0].removable() && packed.games[0].licence_folder(&paths) == pack_dir);
    let _ = std::fs::remove_dir_all(&dir);
}
