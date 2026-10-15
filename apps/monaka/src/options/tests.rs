use super::*;
use monaka_core::manifest::{choice, opt};

const MODE: Opt = opt("mode", "Mode", "How it runs.", choice(&[("fast", "Fast"), ("sharp", "Sharp")], "fast")).turns_off("sharp", "boost", "Sharp has its own.");
const BOOST: Opt = opt("boost", "Boost", "More frames.", Kind::Switch { default: true });
const GAME: Manifest = Manifest { id: "g", name: "G", options: &[MODE, BOOST], ..Manifest::GAME };

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
