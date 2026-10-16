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
/// The viewer's switch for the debug menu (`monaka_contract::manifest::DEBUG_MENU`): without it
/// Back held is plain Back.
pub const DEBUG_MENU_ARG: &str = "--debug-menu";
/// The viewer's switch for the 2D window: the run shown in a desktop window with the debug menu
/// and its panels, no headset (the launcher's "Debug without VR").
pub const DESKTOP_ARG: &str = "--desktop";
/// The viewer's exit code when the player closed the 2D window or left VR in the headset: the
/// session stops, as for Stop VR (any other end while the game runs starts the run again).
pub const CLOSED_BY_PLAYER: i32 = 10;

/// The viewer's line asking for the settings changed in the headset to apply now: the launcher
/// starts the producer again with them, the game and the viewer running on.
pub const APPLY_LINE: &str = "Monaka apply";
/// How a viewer line starts when the player changed a setting in the headset, followed by
/// ` id=value`: the launcher saves it as the window would.
pub const SETTING_LINE: &str = "Monaka setting:";
/// How a viewer line starts when the run cannot go on, followed by why: the launcher shows that as
/// the run's failure, not the viewer's exit code.
pub const FAILURE_LINE: &str = "Monaka failure:";

/// The viewer's argument for the home with no game yet, its files in the folder named by
/// [`RUN_FOLDER_ARG`].
pub const HOME_ARG: &str = "--home";

/// The eye image size the headset asks for, as the home's viewer learned it at its start
/// ([`eye_size_text`]): a run renders its game at it without asking the runtime again.
pub const EYE_SIZE_FILE: &str = "home-eye-size.txt";

/// The game the home's viewer shows: its channel on the first line (none: no game), then the
/// viewer's arguments for it, one per line.
pub const LINK_FILE: &str = "home-link.txt";
/// What the headset's home prints to have the window show another game (`<line> <game id>`), play
/// the game shown, or stop the game that plays.
pub const SELECT_LINE: &str = "Monaka select:";
pub const PLAY_LINE: &str = "Monaka play";
pub const STOP_LINE: &str = "Monaka stop";
/// What the headset's home prints to have the window start the game shown through Steam.
pub const START_GAME_LINE: &str = "Monaka start game";
/// What the viewer prints when the player closes the home: back to the game, which the run brings
/// to the front if something else took it.
pub const HOME_CLOSED_LINE: &str = "Monaka home closed";

/// The game's manifest, as its pack's text (`monaka_contract::pack`).
pub const MANIFEST_FILE: &str = "home-manifest.txt";
/// Every setting of the game and of Monaka with its value, `id=value` per line.
pub const VALUES_FILE: &str = "home-values.txt";
/// The games on offer, one per line ([`games_text`]).
pub const GAMES_FILE: &str = "home-games.txt";
/// What Play would do, and how the run goes ([`status_text`]): the window's own view only.
pub const STATUS_FILE: &str = "home-status.txt";

/// A game on offer, as the launcher listed it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Listed {
    pub id: String,
    pub name: String,
    pub status: String,
    /// The game whose settings show (the one picked in the window).
    pub current: bool,
    /// The game shown in the headset now (its run linked in).
    pub in_vr: bool,
    /// The game's process runs (with or without VR).
    pub running: bool,
}

/// [`GAMES_FILE`]'s text: each game's id, name and status label, then `1` or `0` for whether it
/// is the one shown, the one in VR and running, tab-separated.
pub fn games_text(games: &[Listed]) -> String {
    let clean = |text: &str| text.replace('\t', " ");
    games
        .iter()
        .map(|g| format!("{}\t{}\t{}\t{}\t{}\t{}", clean(&g.id), clean(&g.name), clean(&g.status), u8::from(g.current), u8::from(g.in_vr), u8::from(g.running)))
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
            let mut flag = || parts.next() == Some("1");
            Some(Listed { id, name, status, current: flag(), in_vr: flag(), running: flag() })
        })
        .collect()
}

/// Play as the window has it: whether it can be pressed, why not (or what it will do), whether the
/// game can be started through Steam first, and the latest word on a run (starting, waiting for
/// the game).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    pub can_play: bool,
    pub why: String,
    pub can_start_game: bool,
    pub line: String,
}

/// [`STATUS_FILE`]'s text: one `key=value` per line.
pub fn status_text(status: &Status) -> String {
    let clean = |text: &str| text.replace(['\n', '\r'], " ");
    format!("play={}\nwhy={}\nstart_game={}\nline={}", u8::from(status.can_play), clean(&status.why), u8::from(status.can_start_game), clean(&status.line))
}

/// The status [`status_text`] wrote.
pub fn parse_status(text: &str) -> Status {
    let mut status = Status::default();
    for (key, value) in text.lines().filter_map(|line| line.split_once('=')) {
        match key {
            "play" => status.can_play = value == "1",
            "why" => status.why = value.to_owned(),
            "start_game" => status.can_start_game = value == "1",
            "line" => status.line = value.to_owned(),
            _ => {}
        }
    }
    status
}

/// [`EYE_SIZE_FILE`]'s text: the width and height.
pub fn eye_size_text((width, height): (u32, u32)) -> String {
    format!("{width} {height}")
}

/// The size [`eye_size_text`] wrote; `None` for anything else (none yet, or a size of nothing).
pub fn parse_eye_size(text: &str) -> Option<(u32, u32)> {
    let mut parts = text.split_whitespace().map(|p| p.parse::<u32>().ok());
    let size = (parts.next()??, parts.next()??);
    (size.0 > 0 && size.1 > 0).then_some(size)
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
