use super::*;
use crate::manifest::*;

/// The contract covers the channel layouts: a layout's version changes only with the contract's
/// major. When this fails, raise [`CONTRACT`]'s major (a pack built before reads the old layout),
/// then write both down here.
#[test]
fn a_protocol_version_moves_only_with_the_contract() {
    use crate::protocol::*;
    let versions = [
        ("fence channel", FENCE_VERSION),
        ("pose block", POSE_VERSION),
        ("hand block", HANDS_VERSION),
        ("finger block", FINGERS_VERSION),
        ("reticle block", RETICLES_VERSION),
        ("debug block", crate::debug_draw::DEBUG_VERSION),
        ("page block", crate::page::PAGE_VERSION),
        ("console block", crate::console::CONSOLE_VERSION),
        ("panels block", crate::panels::PANELS_VERSION),
    ];
    let pinned = [
        ("fence channel", 3),
        ("pose block", 2),
        ("hand block", 3),
        ("finger block", 1),
        ("reticle block", 1),
        ("debug block", 8),
        ("page block", 1),
        ("console block", 2),
        ("panels block", 1),
    ];
    assert_eq!((CONTRACT.major, versions), (3, pinned), "a channel layout changed without the contract's major");
}

const AWKWARD: &[Opt] = &[
    opt("odd", "A \\ label\twith\nescapes", "Help, with a tab\there.", Kind::Number { min: -0.1, max: 1e9, step: 0.01, default: 1.0 / 3.0 }).on(Shown::Game),
    opt("pick", "Pick", "", choice(&[("a", "First"), ("b", "Second\tone")], "b")).only_when("odd", "1").turns_off("a", "flag", "Because.").viewer_args("b", &["--x", "1"]).eye_size_note("At {size}."),
    opt("flag", "Flag", "", Kind::Switch { default: true }).launcher().needs_eye_size().needs_steam_files(SteamFiles { game_folder: "Other Game", path: r"bin\x64", file: "a.dll", line: "a_dlls", missing: "No A." }),
    line("free", "Free", "", Kind::Text),
    opt("size", "Size", "", Kind::Size),
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

#[test]
fn the_text_reads_back_exactly() {
    let text = text(&[GAME, MANUAL, AT_START], "Someone <a@b.c>", "https://example.org/x\ty", "1.2.3");
    let parsed = parse(&text).unwrap();
    assert_eq!(parsed.contract, CONTRACT);
    let about = About { author: "Someone <a@b.c>".into(), source: "https://example.org/x\ty".into(), version: "1.2.3".into(), monaka: MONAKA_VERSION.into() };
    assert_eq!(parsed.about, about);
    let games = parsed.manifests;
    assert_eq!(games.len(), 3);
    assert!(games[0] == GAME && games[1] == MANUAL && games[2] == AT_START, "{text}");
    // The compile-time bytes are the same text, NUL-terminated.
    const BYTES: [u8; text_len(&[GAME, MANUAL, AT_START], "Someone <a@b.c>", "https://example.org/x\ty", "1.2.3")] = text_bytes(&[GAME, MANUAL, AT_START], "Someone <a@b.c>", "https://example.org/x\ty", "1.2.3");
    assert_eq!(&BYTES[..BYTES.len() - 1], text.as_bytes());
    assert_eq!(BYTES[BYTES.len() - 1], 0);
}

#[test]
fn a_monaka_setting_a_game_reads_survives_the_text() {
    // A game listing one of Monaka's own settings its producer reads passes the check after the
    // round trip: what it lists still is Monaka's, unchanged.
    const READS: &[Opt] = &[crate::manifest::DISABLE_VIEW_BOBBING];
    const READER: Manifest = Manifest { options: READS, launch: Launch::Manual, controls: &[], ..GAME };
    let parsed = parse(&text(&[READER], "", "", "1.2.3")).unwrap();
    assert!(parsed.manifests[0] == READER);
    assert!(parsed.manifests[0].check().is_ok(), "{:?}", parsed.manifests[0].check());
}

#[test]
fn known_bugs_from_an_older_pack_are_read_past() {
    let old = text(&[MANUAL], "", "", "1.2.3").replace("status in_progress\n", "status in_progress\nbug One.\nbug Two.\n");
    assert!(parse(&old).unwrap().manifests[0] == MANUAL);
}

#[test]
fn a_pack_from_before_1_1_names_nobody() {
    // 1.0 wrote no author or source (nor 2.1's version and monaka).
    let new = text(&[MANUAL], "", "", "1.2.3").replace(&format!("monaka-manifest {CONTRACT}"), "monaka-manifest 1.0");
    let about = ["author ", "source ", "version ", "monaka "];
    let old: String = new.lines().filter(|l| !about.iter().any(|k| l.starts_with(k))).map(|l| format!("{l}\n")).collect();
    let parsed = parse(&old).unwrap();
    assert_eq!(parsed.contract, Contract { major: 1, minor: 0 });
    assert_eq!(parsed.about, About::default());
    assert!(parsed.manifests[0] == MANUAL);
    assert!(parse(&text(&[MANUAL], "", "", "1.2.3").replace("source \n", "source \nbogus 1\n")).unwrap_err().contains("bogus outside a game"));
}

#[test]
fn a_host_runs_its_major_and_older_minors() {
    let host = Contract { major: 2, minor: 3 };
    assert!(host.runs(Contract { major: 2, minor: 3 }).is_ok());
    assert!(host.runs(Contract { major: 2, minor: 0 }).is_ok());
    assert!(host.runs(Contract { major: 2, minor: 4 }).unwrap_err().contains("update Monaka"));
    assert!(host.runs(Contract { major: 3, minor: 0 }).unwrap_err().contains("update Monaka"));
    assert!(host.runs(Contract { major: 1, minor: 9 }).unwrap_err().contains("get a version of the game pack"));
    assert!(host.needs_newer_host(Contract { major: 2, minor: 4 }) && host.needs_newer_host(Contract { major: 3, minor: 0 }));
    assert!(!host.needs_newer_host(Contract { major: 2, minor: 3 }) && !host.needs_newer_host(Contract { major: 1, minor: 9 }));
    assert_eq!(Contract::from_packed(host.packed()), host);
    assert_eq!(host.to_string(), "2.3");
}

#[test]
fn broken_text_says_where() {
    assert!(parse("hello").unwrap_err().contains("not a Monaka manifest"));
    let sideways = text(&[GAME], "", "", "1.2.3").replace("status in_progress", "status sideways");
    assert!(parse(&sideways).unwrap_err().contains("unknown status"));
    let unended = text(&[MANUAL], "", "", "1.2.3").replace("end\n", "");
    assert!(parse(&unended).unwrap_err().contains("no end"));
}
