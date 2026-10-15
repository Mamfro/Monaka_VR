use super::*;
use crate::host;
use crate::manifest::*;

/// The contract covers the channel's records: a record's major changes only with the contract's
/// major (a field appended raises its minor, which needs nothing new of the contract). When this
/// fails, raise [`CONTRACT`]'s major (a pack built before reads the old layout), then write both
/// down here.
#[test]
fn a_records_major_moves_only_with_the_contract() {
    use crate::protocol::*;
    let majors = [
        ("fence channel", FENCE_MAJOR),
        ("pose block", POSE_MAJOR),
        ("hand block", HANDS_MAJOR),
        ("finger block", FINGERS_MAJOR),
        ("reticle block", RETICLES_MAJOR),
        ("depth block", DEPTH_MAJOR),
        ("debug block", crate::debug_draw::DEBUG_MAJOR),
        ("page block", crate::page::PAGE_MAJOR),
        ("console block", crate::console::CONSOLE_MAJOR),
        ("panels block", crate::panels::PANELS_MAJOR),
    ];
    let pinned = [
        ("fence channel", 4),
        ("pose block", 4),
        ("hand block", 5),
        ("finger block", 3),
        ("reticle block", 3),
        ("depth block", 1),
        ("debug block", 9),
        ("page block", 2),
        ("console block", 3),
        ("panels block", 2),
    ];
    assert_eq!((CONTRACT.major, majors), (4, pinned), "a record's layout changed in place without the contract's major");
}

const AWKWARD: &[Opt] = &[
    opt("odd", "A \\ label\twith\nescapes", "Help, with a tab\there.", Kind::Number { min: -0.1, max: 1e9, step: 0.01, default: 1.0 / 3.0 }).on(Shown::Game),
    opt("pick", "Pick", "", choice(&[("a", "First"), ("b", "Second\tone")], "b")).only_when("odd", "1").turns_off("a", "flag", "Because.").viewer_lines("b", &["x=1", "y=2\t3"]).to_viewer().eye_size_note("At {size}."),
    opt("flag", "Flag", "", Kind::Switch { default: true }).launcher().needs_eye_size().needs_steam_files(SteamFiles { game_folder: "Other Game", path: r"bin\x64", file: "a.dll", line: "a_dlls", missing: "No A." }),
    line("free", "Free", "", Kind::Text),
    opt("size", "Size", "", Kind::Size).dev(),
];

const GAME: Manifest = Manifest {
    id: "t",
    name: "Test",
    status: Status::InProgress,
    executables: &["a.exe", "b.exe"],
    steam_app: 4294967295,
    producer: "monaka_t",
    log: "t.log",
    options_file: "t-options.txt",
    run_prefix: "t",
    launch: Launch::Auto { settle_seconds: 60 },
    refused_arguments: &[("/3dtv", "breaks it")],
    before: "Nothing.",
    options: AWKWARD,
    licences: &["X-LICENSE.txt"],
    left_hand_aims: true,
    weapon_ring: false,
    controls: &[
        HowTo { action: "Do\tit \\ now", ways: &[way("A way,\twith a tab.", &[("pick", "!a"), ("flag", "1")]), always("")], options: &["flag", "size"] },
        HowTo { action: "Nothing to tune", ways: &[], options: &[] },
    ],
};

const MANUAL: Manifest = Manifest { id: "m", launch: Launch::Manual, options: &[], controls: &[], ..GAME };
const AT_START: Manifest = Manifest { id: "s", launch: Launch::AtStart, ..MANUAL };

const NEEDS: Needs = Needs { requires: &[host::PANELS], uses: &[(host::DEPTH, true), (host::FINGERS, false), (host::PANELS, true), ("teleport", true)] };

#[test]
fn the_text_reads_back_exactly() {
    let text = text(&[GAME, MANUAL, AT_START], "Someone <a@b.c>", "https://example.org/x\ty", "1.2.3", &NEEDS);
    let parsed = parse(&text).unwrap();
    assert_eq!(parsed.contract, CONTRACT);
    let about = About { author: "Someone <a@b.c>".into(), source: "https://example.org/x\ty".into(), version: "1.2.3".into(), monaka: MONAKA_VERSION.into() };
    assert_eq!(parsed.about, about);
    let games = parsed.manifests;
    assert_eq!(games.len(), 3);
    assert!(games[0] == GAME && games[1] == MANUAL && games[2] == AT_START, "{text}");
    // The compile-time bytes are the same text, NUL-terminated.
    const BYTES: [u8; text_len(&[GAME, MANUAL, AT_START], "Someone <a@b.c>", "https://example.org/x\ty", "1.2.3", &NEEDS)] =
        text_bytes(&[GAME, MANUAL, AT_START], "Someone <a@b.c>", "https://example.org/x\ty", "1.2.3", &NEEDS);
    assert_eq!(&BYTES[..BYTES.len() - 1], text.as_bytes());
    assert_eq!(BYTES[BYTES.len() - 1], 0);
}

#[test]
fn a_pack_names_the_host_features_it_needs() {
    // What it requires; what it uses, but not twice and not what it requires; and what its
    // manifests need (GAME's how-tos on the Controls tab, AT_START joined as it starts).
    let parsed = parse(&text(&[GAME, AT_START], "", "", "1.2.3", &NEEDS)).unwrap();
    assert_eq!(parsed.requires, [host::PANELS]);
    assert_eq!(parsed.uses, [host::DEPTH, "teleport", host::CONTROLS_TAB, host::LAUNCH_AT_START]);
    // A host without a required feature cannot run it; without a used one it runs with less.
    assert!(parsed.missing_required().is_empty());
    assert_eq!(parsed.missing_used(), ["teleport"]);
    let needy = parse(&text(&[MANUAL], "", "", "1.2.3", &Needs { requires: &["teleport", host::DEPTH], uses: &[] })).unwrap();
    assert_eq!(needy.missing_required(), ["teleport"]);
    // A pack that names nothing writes no lines for it.
    let plain = text(&[MANUAL], "", "", "1.2.3", &Needs::NONE);
    assert!(!plain.contains("requires") && !plain.contains("uses"), "{plain}");
    assert!(parse(&plain).unwrap().uses.is_empty());
}

#[test]
fn a_newer_packs_additions_are_read_past() {
    // Words this reader does not know, where a newer pack could put them: before the games, among
    // a game's fields and among an option's (which must not close the option).
    let newer = text(&[GAME, MANUAL], "", "", "1.2.3", &Needs::NONE)
        .replacen("version 1.2.3\n", "version 1.2.3\nsigned_by Someone\n", 1)
        .replacen("before Nothing.\n", "before Nothing.\nicon game.png\n", 1)
        .replacen("label Pick\n", "label Pick\ntooltip_image pick.png\n", 1)
        .replacen("kind switch 1\n", "kind switch 1\nlive 1\n", 1);
    let parsed = parse(&newer).unwrap();
    assert!(parsed.manifests[0] == GAME && parsed.manifests[1] == MANUAL, "{newer}");
}

#[test]
fn a_newer_packs_words_read_as_the_nearest_known() {
    let words = |from: &str, to: &str| parse(&text(&[GAME], "", "", "1.2.3", &Needs::NONE).replacen(from, to, 1)).unwrap().manifests.remove(0);
    // An unknown tab puts the option on the Game tab.
    assert_eq!(words("shown main\n", "shown hands\n").options[1].shown, Shown::Game);
    // An unknown kind shows the option as text; its value still goes to the producer.
    let colour = words("kind size\n", "kind colour ff00ff\n");
    assert_eq!((colour.options[4].kind, colour.options[4].producer), (Kind::Text, true));
    // An unknown status reads as experimental, an unknown launch as manual.
    assert_eq!(words("status in_progress\n", "status released\n").status, Status::Experimental);
    assert_eq!(words("launch auto 60\n", "launch steam_deck\n").launch, Launch::Manual);
}

#[test]
fn an_older_major_reads_with_its_number() {
    // Before 4 the header was `major.minor`; 3.x packs listed known bugs, which are read past.
    let old = text(&[MANUAL], "", "", "1.2.3", &Needs::NONE)
        .replace(&format!("monaka-manifest {CONTRACT}"), "monaka-manifest 3.0")
        .replace("status in_progress\n", "status in_progress\nbug One.\nbug Two.\n");
    let parsed = parse(&old).unwrap();
    assert_eq!(parsed.contract, Contract { major: 3 });
    assert!(parsed.manifests[0] == MANUAL);
}

#[test]
fn a_monaka_setting_a_game_reads_survives_the_text() {
    // A game listing one of Monaka's own settings its producer reads passes the check after the
    // round trip: what it lists still is Monaka's, unchanged.
    const READS: &[Opt] = &[crate::manifest::DISABLE_VIEW_BOBBING];
    const READER: Manifest = Manifest { options: READS, launch: Launch::Manual, controls: &[], ..GAME };
    let parsed = parse(&text(&[READER], "", "", "1.2.3", &Needs::NONE)).unwrap();
    assert!(parsed.manifests[0] == READER);
    assert!(parsed.manifests[0].check().is_ok(), "{:?}", parsed.manifests[0].check());
}

#[test]
fn a_shared_option_is_known_shared_after_the_text() {
    const SHARES: &[Opt] = &[HUD_SIZE, opt("mine", "Mine", "", Kind::Switch { default: false })];
    let parsed = parse(&text(&[Manifest { options: SHARES, controls: &[], ..MANUAL }], "", "", "1.2.3", &Needs::NONE)).unwrap();
    let options = parsed.manifests[0].options;
    assert!(options[0].shared && !options[1].shared);
}

#[test]
fn a_host_runs_its_own_major_only() {
    let host = Contract { major: 4 };
    assert!(host.runs(Contract { major: 4 }).is_ok());
    assert!(host.runs(Contract { major: 5 }).unwrap_err().contains("update Monaka"));
    assert!(host.runs(Contract { major: 3 }).unwrap_err().contains("get a version of the game pack"));
    assert!(host.needs_newer_host(Contract { major: 5 }) && !host.needs_newer_host(Contract { major: 3 }));
    assert_eq!(Contract::from_packed(host.packed()), host);
    // A 3.x pack's export held its minor in the low half.
    assert_eq!(Contract::from_packed(3 << 16 | 2), Contract { major: 3 });
    assert_eq!(host.to_string(), "4");
}

#[test]
fn broken_text_says_where() {
    assert!(parse("hello").unwrap_err().contains("not a Monaka manifest"));
    // A known field this reader cannot read is an error, not something to skip.
    let bent = text(&[GAME], "", "", "1.2.3", &Needs::NONE).replacen("left_hand_aims 1", "left_hand_aims maybe", 1);
    assert!(parse(&bent).unwrap_err().contains("not 0 or 1"));
    let unended = text(&[MANUAL], "", "", "1.2.3", &Needs::NONE).replace("end\n", "");
    assert!(parse(&unended).unwrap_err().contains("no end"));
}
