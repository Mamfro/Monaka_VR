//! `monaka_play GAME [option=value ...]`: plays a game in VR from the command line, as the `monaka`
//! window does (same options, same saved settings when none are given on the line), plus the
//! options only the command line takes (`mode=auto`, `launch=off`, ...). The `tools\Play-*.ps1`
//! scripts are thin wrappers over it. The PC checks without a headset run from here too, with the
//! same option files (`tools\Harness-*.ps1`, `Capture-*.ps1` and `Probe-*.ps1` wrap them):
//!
//! ```text
//! monaka_play dl1 aim=hand_rig render_size=headset
//! monaka_play list                                      the games and their options
//! monaka_play harness dl2 --script tools\harness\x.txt [--eye WxH] [--delay S] [option=value ...]
//! monaka_play capture dl1 [--seconds N] [--delay S] [--eye WxH] [--save N] [probe arguments] [option=value ...]
//! monaka_play probe beast [--seconds N] [--delay S] [option=value ...]
//! monaka_play packs                                     the game packs added, and any left out
//! monaka_play add FILE.zip [--yes]                      adds a game pack (or a lone FILE.dll)
//! monaka_play remove PACK [--yes]                       removes one (its folder name or a game's id)
//! ```
//!
//! For the checks, an `option=value` the game's table does not know goes to the producer as an
//! extra (`probe_view=1`, `synthetic_yaw=30`, ...); the rest are checked as for a play session.
//! Ctrl+C stops VR; quitting the game ends it.

use monaka_launch::check::{self, Check};
use monaka_launch::games::{self, Game, Kind, Shown, Values};
use monaka_launch::packs;
use monaka_launch::session::{self, Console, Event, Stop};
use monaka_launch::settings::Settings;
use monaka_launch::Paths;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::OnceLock;
use windows::Win32::Foundation::{FALSE, TRUE};
use windows::Win32::System::Console::{CTRL_BREAK_EVENT, CTRL_C_EVENT, SetConsoleCtrlHandler};
use windows::core::BOOL;

static STOP: OnceLock<Stop> = OnceLock::new();

unsafe extern "system" fn on_control(event: u32) -> BOOL {
    if event == CTRL_C_EVENT || event == CTRL_BREAK_EVENT {
        if let Some(stop) = STOP.get() {
            stop.request();
        }
        TRUE
    } else {
        FALSE
    }
}

fn list() {
    for game in games::list() {
        let from = if game.removable() { format!("  [game pack {}]", game.pack().folder().display()) } else { format!("  [local build {}]", game.pack().dll.display()) };
        println!("{} ({}): {}{from}", game.id, game.status.label(), game.name);
        for opt in game.options {
            let only = if opt.shown == Shown::Line { "  (command line only)" } else { "" };
            println!("    {}={}  {}{only}", opt.id, games::describe(opt), opt.label);
            if let Kind::Choice { choices, window, .. } = opt.kind {
                for (i, (id, label)) in choices.iter().enumerate() {
                    let only = if i >= window && only.is_empty() { "  (command line only)" } else { "" };
                    println!("        {id}: {label}{only}");
                }
            }
        }
    }
}

const USAGE: &str = "Usage: monaka_play GAME [option=value ...]\n       monaka_play harness GAME --script FILE [--eye WxH] [--delay S] [option=value ...]\n       monaka_play capture GAME [--seconds N] [--delay S] [--eye WxH] [--save N] [probe arguments] [option=value ...]\n       monaka_play probe GAME [--seconds N] [--delay S] [option=value ...]\n       monaka_play list   (the games and their options)
       monaka_play pack FILE.dll   (what a game pack declares, read from the file)
       monaka_play packs   (the game packs added, and any left out)
       monaka_play add FILE.zip|FILE.dll [--yes]
       monaka_play remove PACK|GAME [--yes]";

fn usage(why: &str) -> ExitCode {
    eprintln!("{why}\n{USAGE}");
    ExitCode::from(1)
}

/// `option=value` into `values`; for a check, an unknown option becomes an extra for the producer.
fn take_option(game: &Game, values: &mut Values, arg: &str, lenient: bool) -> Result<(), String> {
    let (id, value) = arg.split_once('=').ok_or_else(|| format!("Expected option=value, got {arg}"))?;
    if lenient && game.option(id).is_none() {
        let extra = values.text(game, "extra");
        values.set("extra", format!("{extra} {arg}").trim().to_owned());
        return Ok(());
    }
    values.set_checked(game, id, value)
}

fn report_line(event: Event, failed: &mut bool) {
    match event {
        Event::Info(line) => println!("{line}"),
        Event::Warning(line) => println!("Warning: {line}"),
        Event::Failed(line) => {
            *failed = true;
            eprintln!("{line}");
        }
        Event::Detail(line) => println!("  {line}"),
        Event::RunFolder(folder) => println!("Run folder: {}", folder.display()),
        Event::State(_) => {}
    }
}

/// `harness`, `capture` or `probe`: the arguments after the game.
fn check(kind: &str, args: &[String]) -> ExitCode {
    let Some(first) = args.first() else { return usage("Which game?") };
    let Some(game) = games::game(first) else { return usage(&format!("Unknown game {first}; monaka_play list shows the games.")) };
    let mut values = Values::default();
    let (mut script, mut eye, mut delay, mut seconds) = (None, None, 0u64, 8u64);
    let mut probe_args: Vec<String> = Vec::new();
    let mut rest = args[1..].iter();
    let number = |what: &str, value: Option<&String>| value.and_then(|v| v.parse::<u64>().ok()).ok_or_else(|| format!("{what} takes a number"));
    while let Some(arg) = rest.next() {
        let outcome: Result<(), String> = match arg.as_str() {
            "--script" => rest.next().map(|s| script = Some(PathBuf::from(s))).ok_or("--script takes a file".to_owned()),
            "--eye" => rest.next().and_then(|s| monaka_core::options::parse_size(s)).map(|size| eye = Some(size)).ok_or("--eye takes WxH (256 to 8192 each)".to_owned()),
            "--delay" => number("--delay", rest.next()).map(|n| delay = n),
            "--seconds" => number("--seconds", rest.next()).map(|n| seconds = n),
            "--save" => number("--save", rest.next()).map(|n| probe_args.extend(["--save".to_owned(), n.to_string()])),
            flag if flag.starts_with("--") => {
                // The probe's own flags, with a value when one follows (`--hand 30`, `--pad-hold start`).
                probe_args.push(flag.to_owned());
                if let Some(value) = rest.clone().next().filter(|v| !v.starts_with("--") && !v.contains('=')) {
                    probe_args.push(value.clone());
                    rest.next();
                }
                Ok(())
            }
            option => take_option(game, &mut values, option, true),
        };
        if let Err(why) = outcome {
            return usage(&why);
        }
    }
    let check = match kind {
        "harness" => match script {
            Some(script) => Check::Harness { script },
            None => return usage("harness needs --script FILE"),
        },
        "capture" => Check::Capture { seconds, probe_args },
        _ => Check::Probe { seconds },
    };
    let mut failed = false;
    match check::run(game, &values, &Paths::locate(), &check, delay, eye, &mut |event| report_line(event, &mut failed)) {
        Ok(folder) => {
            println!("Run folder: {}", folder.display());
            ExitCode::SUCCESS
        }
        Err(why) => {
            eprintln!("{why}");
            ExitCode::from(2)
        }
    }
}

/// `monaka_play pack FILE.dll`: the contract and games a producer DLL declares.
fn show_pack(file: Option<&String>) -> ExitCode {
    let Some(file) = file else { return usage("Which DLL?") };
    match monaka_launch::packs::read(std::path::Path::new(file)) {
        Ok(pack) => {
            let fit = pack.runs().map_or_else(|why| format!("this Monaka cannot run it: {why}"), |()| "this Monaka runs it".to_owned());
            println!("{}: pack contract {} ({fit}), {}", pack.dll.display(), pack.contract, packs::credit(&pack.about));
            for game in &pack.manifests {
                println!("  {} ({}): {} options, runs as {}", game.name, game.id, game.options.len(), game.executables.join(", "));
            }
            ExitCode::SUCCESS
        }
        Err(why) => {
            eprintln!("{why}");
            ExitCode::from(1)
        }
    }
}

/// `monaka_play packs`: the packs in the games folder and the ones left out.
fn show_packs() -> ExitCode {
    let paths = Paths::locate();
    let catalogue = games::rescan(&paths);
    println!("Games folder: {}", paths.packs.display());
    let mut shown: Vec<&std::path::Path> = Vec::new();
    for pack in catalogue.games.iter().filter(|g| g.removable()).map(Game::pack) {
        if shown.contains(&pack.folder()) {
            continue;
        }
        shown.push(pack.folder());
        let name = pack.folder().file_name().unwrap_or_default().to_string_lossy();
        println!("  {name}: {} (contract {}, {})", games::names_in(pack.folder()), pack.contract, packs::credit(&pack.about));
    }
    if shown.is_empty() {
        println!("  No game packs added.");
    }
    for problem in &catalogue.problems {
        println!("Left out: {problem}");
    }
    ExitCode::SUCCESS
}

/// Asks a yes/no question on the console (no answer is no).
fn confirmed(question: &str) -> bool {
    println!("{question} [y/N]");
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer).is_ok() && answer.trim().eq_ignore_ascii_case("y")
}

/// `monaka_play add FILE [--yes]`: adds a game pack, after saying what it is.
fn add_pack(args: &[String]) -> ExitCode {
    let Some(file) = args.iter().find(|a| !a.starts_with("--")) else { return usage("Which pack file?") };
    let yes = args.iter().any(|a| a == "--yes");
    let paths = Paths::locate();
    let staged = match packs::stage(std::path::Path::new(file), &paths.packs) {
        Ok(staged) => staged,
        Err(why) => {
            eprintln!("Not added: {why}");
            return ExitCode::from(1);
        }
    };
    let clashes = games::clashes(staged.manifests(), staged.replaces().as_deref());
    if !clashes.is_empty() {
        eprintln!("Not added: {}", clashes.join(" "));
        return ExitCode::from(1);
    }
    println!("{} (game pack {}).", staged.names(), staged.credit());
    if let Some(old) = staged.replaces() {
        println!("It replaces the installed {}.", old.display());
    }
    println!("Game packs run their own code inside the game. Only add packs from people you trust.");
    if !yes && !confirmed("Add it?") {
        println!("Not added.");
        return ExitCode::from(1);
    }
    let names = staged.names();
    match staged.install() {
        Ok(folder) => {
            println!("Added {names}: {}", folder.display());
            ExitCode::SUCCESS
        }
        Err(why) => {
            eprintln!("Not added: {why}");
            ExitCode::from(1)
        }
    }
}

/// `monaka_play remove PACK [--yes]`: removes a game pack, named by its folder or one of its games.
fn remove_pack(args: &[String]) -> ExitCode {
    let Some(name) = args.iter().find(|a| !a.starts_with("--")) else { return usage("Which pack?") };
    let yes = args.iter().any(|a| a == "--yes");
    let paths = Paths::locate();
    let pack = games::rescan(&paths).games.iter().filter(|g| g.removable()).map(|g| (g, g.pack())).find_map(|(game, pack)| {
        let folder = pack.folder().file_name().unwrap_or_default().to_string_lossy().into_owned();
        (folder.eq_ignore_ascii_case(name) || game.id.eq_ignore_ascii_case(name)).then_some(pack)
    });
    let Some(pack) = pack else {
        eprintln!("No game pack {name}; monaka_play packs lists them.");
        return ExitCode::from(1);
    };
    let names = games::names_in(pack.folder());
    println!("{} ({names}): {}", pack.folder().display(), packs::credit(&pack.about));
    if !yes && !confirmed("Remove it? Your options for its games are kept.") {
        println!("Not removed.");
        return ExitCode::from(1);
    }
    match packs::remove(pack.folder(), &paths.packs) {
        Ok(()) => {
            println!("Removed {names}.");
            ExitCode::SUCCESS
        }
        Err(why) => {
            eprintln!("{why}");
            ExitCode::from(1)
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(first) = args.first() else { return usage("Which game?") };
    match first.as_str() {
        "list" => {
            list();
            return ExitCode::SUCCESS;
        }
        "harness" | "capture" | "probe" => return check(first, &args[1..]),
        "pack" => return show_pack(args.get(1)),
        "packs" => return show_packs(),
        "add" => return add_pack(&args[1..]),
        "remove" => return remove_pack(&args[1..]),
        _ => {}
    }
    let Some(game) = games::game(first) else {
        return usage(&format!("Unknown game {first}; monaka_play list shows the games."));
    };
    // Options on the line replace the saved ones; with none, the saved ones (the window's) apply.
    let mut values = if args.len() > 1 { Default::default() } else { Settings::load().values(game.id) };
    for arg in &args[1..] {
        if let Err(why) = take_option(game, &mut values, arg, false) {
            return usage(&why);
        }
    }

    let stop = match Stop::new() {
        Ok(stop) => STOP.get_or_init(|| stop),
        Err(why) => {
            eprintln!("{why}");
            return ExitCode::from(3);
        }
    };
    // SAFETY: registers a handler that only signals the stop.
    let _ = unsafe { SetConsoleCtrlHandler(Some(on_control), true) };
    println!("{} ({}). Before playing: {}", game.name, game.status.label(), game.before);
    println!("Ctrl+C here stops VR; quitting the game ends it.");
    let mut failed = false;
    session::run(game, &values, &Paths::locate(), Console::Shared, stop, &mut |event| report_line(event, &mut failed));
    if failed { ExitCode::from(2) } else { ExitCode::SUCCESS }
}
