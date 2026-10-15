//! What the launcher leaves in a folder for the headset's home (the viewer's Games, Settings and
//! Log panels): the game's manifest as its pack text ([`MANIFEST_FILE`]), every setting's value
//! ([`VALUES_FILE`]) and the games on offer ([`GAMES_FILE`]). The launcher and the viewer ship
//! together, so this is theirs alone, not part of the pack contract.
//!
//! The Monaka window keeps one viewer open from its start ([`HOME_ARG`]): the home with no game,
//! in a folder of its own that the window rewrites whenever what it shows changes, so both show
//! the same. A run links its game in through [`LINK_FILE`] and lets it go by emptying it. The
//! headset asks the window for what it cannot do itself with the lines it prints
//! ([`SELECT_LINE`], [`PLAY_LINE`], [`STOP_LINE`]).

use monaka_contract::manifest::{Manifest, Values};

/// The viewer's argument naming the run folder.
pub const RUN_FOLDER_ARG: &str = "--run-folder";
/// The viewer's argument for the home with no game yet, its files in the folder named by
/// [`RUN_FOLDER_ARG`].
pub const HOME_ARG: &str = "--home";

/// The game the home's viewer shows: its channel on the first line (none: no game), then the
/// viewer's arguments for it, one per line.
pub const LINK_FILE: &str = "home-link.txt";
/// What the headset's home prints to have the window show another game (`<line> <game id>`), play
/// the game shown, or stop the game that plays.
pub const SELECT_LINE: &str = "Monaka select:";
pub const PLAY_LINE: &str = "Monaka play";
pub const STOP_LINE: &str = "Monaka stop";

/// The game's manifest, as its pack's text (`monaka_contract::pack`).
pub const MANIFEST_FILE: &str = "home-manifest.txt";
/// Every setting of the game and of Monaka with its value, `id=value` per line.
pub const VALUES_FILE: &str = "home-values.txt";
/// The games on offer, one per line ([`games_text`]).
pub const GAMES_FILE: &str = "home-games.txt";

/// A game on offer, as the launcher listed it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Listed {
    pub id: String,
    pub name: String,
    pub status: String,
    /// The game whose settings show (the one picked in the window).
    pub current: bool,
    /// The game that plays now.
    pub playing: bool,
}

/// [`GAMES_FILE`]'s text: each game's id, name and status label, then `1` or `0` for whether it
/// is the one shown and the one that plays, tab-separated.
pub fn games_text(games: &[Listed]) -> String {
    let clean = |text: &str| text.replace('\t', " ");
    games
        .iter()
        .map(|g| format!("{}\t{}\t{}\t{}\t{}", clean(&g.id), clean(&g.name), clean(&g.status), u8::from(g.current), u8::from(g.playing)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The games [`games_text`] wrote.
pub fn parse_games(text: &str) -> Vec<Listed> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let id = parts.next().filter(|n| !n.is_empty())?.to_owned();
            let name = parts.next().unwrap_or_default().to_owned();
            let status = parts.next().unwrap_or_default().to_owned();
            Some(Listed { id, name, status, current: parts.next() == Some("1"), playing: parts.next() == Some("1") })
        })
        .collect()
}

/// [`LINK_FILE`]'s text: the game's channel and the viewer's arguments for it.
pub fn link_text(channel: &str, args: &[String]) -> String {
    std::iter::once(channel).chain(args.iter().map(String::as_str)).collect::<Vec<_>>().join("\n")
}

/// The channel and arguments [`link_text`] wrote; `None` for no game.
pub fn parse_link(text: &str) -> Option<(String, Vec<String>)> {
    let mut lines = text.lines();
    let channel = lines.next().map(str::trim).filter(|c| !c.is_empty())?.to_owned();
    Some((channel, lines.map(str::to_owned).collect()))
}

/// [`VALUES_FILE`]'s text: every setting of `owners` (the game, then Monaka) with its value in
/// `values`, its default where none was saved.
pub fn values_text(owners: &[&Manifest], values: &Values) -> String {
    owners.iter().flat_map(|owner| owner.options.iter().map(move |opt| format!("{}={}", opt.id, values.text(owner, opt.id)))).collect::<Vec<_>>().join("\n")
}

/// The values [`values_text`] wrote.
pub fn parse_values(text: &str) -> Values {
    let mut values = Values::default();
    for (id, value) in text.lines().filter_map(|line| line.split_once('=')) {
        values.set(id, value);
    }
    values
}

#[cfg(test)]
mod tests;
