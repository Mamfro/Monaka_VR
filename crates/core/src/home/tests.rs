use super::*;
use monaka_contract::manifest::{HUD_SIZE, Kind, MONAKA, WORLD_HUD, opt};

#[test]
fn games_read_back() {
    let games = [
        Listed { id: "dl1".into(), name: "Dying Light".into(), status: "Playable".into(), current: true, playing: false },
        Listed { id: "tb".into(), name: "Tab\tbed".into(), status: "Alpha".into(), current: false, playing: true },
    ];
    let read = parse_games(&games_text(&games));
    assert_eq!(read[0], games[0]);
    assert_eq!((read[1].name.as_str(), read[1].current, read[1].playing), ("Tab bed", false, true));
    assert!(parse_games("").is_empty());
}

#[test]
fn a_link_reads_back_and_an_empty_one_is_no_game() {
    let args = ["--option".to_owned(), "dpad_menus=left,right".to_owned()];
    assert_eq!(parse_link(&link_text(r"Local\monaka-1", &args)), Some((r"Local\monaka-1".to_owned(), args.to_vec())));
    assert_eq!(parse_link(""), None);
    assert_eq!(parse_link("\n--option\nx=1"), None);
}

#[test]
fn values_read_back_with_defaults_filled_in() {
    const GAME: Manifest = Manifest { id: "g", options: &[WORLD_HUD, HUD_SIZE, opt("mine", "Mine", "", Kind::Switch { default: true })], ..Manifest::GAME };
    let mut saved = Values::default();
    saved.set("hud_scale", "0.4");
    let read = parse_values(&values_text(&[&GAME, &MONAKA], &saved));
    assert_eq!(read.text(&GAME, "hud_scale"), "0.4");
    assert_eq!(read.text(&GAME, "mine"), "1", "the default where none was saved");
    assert!(read.0.contains_key("controller_smoothing"), "Monaka's own too");
}
