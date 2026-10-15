use super::*;
use monaka_contract::manifest::{choice, opt};

const MODE: Opt = opt("mode", "Mode", "How it runs.", choice(&[("fast", "Fast"), ("sharp", "Sharp")], "fast")).turns_off("sharp", "boost", "Sharp has its own.");
const BOOST: Opt = opt("boost", "Boost", "More frames.", Kind::Switch { default: true });
const GAME: Manifest = Manifest { id: "g", name: "G", options: &[MODE, BOOST], ..Manifest::GAME };

#[test]
fn settings_for_testers_show_only_in_developer_mode() {
    const TRIAL: Opt = opt("trial", "Trial", "For testers.", Kind::Switch { default: false }).dev();
    const TRIED: Manifest = Manifest { options: &[MODE, TRIAL], ..GAME };
    let pack = Box::leak(Box::new(monaka_launch::packs::Pack {
        dll: "monaka_g.dll".into(),
        contract: monaka_contract::pack::CONTRACT,
        about: Default::default(),
        requires: Vec::new(),
        uses: Vec::new(),
        manifests: Vec::new(),
    }));
    let game = Game { manifest: &TRIED, origin: monaka_launch::games::Origin::Pack(pack) };
    let ids = |dev: bool| rows(&game, false, dev).iter().map(|r| r.opt.id).collect::<Vec<_>>();
    assert!(!ids(false).contains(&"trial") && ids(true).contains(&"trial"));
    // The switch itself is always there, among Monaka's own.
    assert!(ids(false).contains(&monaka_contract::manifest::DEV_MODE.id));
}

#[test]
fn values_are_named_as_the_window_shows_them() {
    assert_eq!(value_name(&BOOST, "1"), "on");
    assert_eq!(value_name(&BOOST, "0"), "off");
    assert_eq!(value_name(&MODE, "sharp"), "Sharp");
    assert_eq!(value_name(&MODE, "!sharp"), "not Sharp");
    assert_eq!(value_name(&MODE, "other"), "other", "a value with no choice is shown as it is");
    assert_eq!(set_to(&BOOST, "1"), "Turning it on");
    assert_eq!(set_to(&MODE, "sharp"), "Set to Sharp,");
    assert_eq!(set_to(&MODE, "!sharp"), "Set to anything but Sharp,");
}

#[test]
fn the_help_line_says_what_an_option_turns_off_or_why_it_is_off() {
    let values = Values::default();
    assert_eq!(help_text(&GAME, &values, &MODE), "How it runs. Set to Sharp, turns off Boost.");
    let mut sharp = Values::default();
    sharp.set("mode", "sharp".to_owned());
    assert_eq!(help_text(&GAME, &sharp, &BOOST), "Off while Mode is Sharp. Sharp has its own.");
}
