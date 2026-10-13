//! What a game crate declares about itself, once, for everyone else: the launcher window and
//! `monaka_play` (the game list, its options and their defaults, what the player sets up first),
//! `monaka_loader` (the executables, through the producer's `MONAKA_HOST` export, built from here),
//! the producer's own config (defaults and ranges), and the harness and capture runs.
//!
//! A game crate keeps its [`Manifest`]s in `src/manifest.rs`, built from this module's types
//! alone; the producer exports them as text (`monaka_core::pack`), which the launcher reads from
//! the DLL without loading it. One
//! producer DLL may serve several games (The New Order and The Old Blood): one manifest each, the
//! same `producer`, `log` and `options_file`.
//!
//! Options follow one vocabulary: the shared ones here ([`aim`], [`WORLD_HUD`], [`HUD_SIZE`],
//! [`HUD_DISTANCE`], [`LATENCY`], [`EYE_SEPARATION`], ...) mean the same in every game and are
//! read by `monaka_core::options::Shared`; a game adds its own. The launcher writes every producer
//! option as `id=value` into the game's options file (`Opt::producer`), so a producer reads the
//! launcher's own ids and nothing is translated on the way.

use crate::feature::Feature;
use crate::options::{Options, parse_size};
use std::collections::BTreeMap;

/// How finished a game's VR is, as the README's table says.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Playable,
    Beta,
    Alpha,
    PreAlpha,
    InProgress,
    Experimental,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Playable => "Playable",
            Status::Beta => "Beta",
            Status::Alpha => "Alpha",
            Status::PreAlpha => "Pre-alpha",
            Status::InProgress => "In Progress",
            Status::Experimental => "Experimental",
        }
    }

    pub fn meaning(self) -> &'static str {
        match self {
            Status::Playable => "You can play the whole game this way with no big problems.",
            Status::Beta => "You can probably play the whole game this way. Some rough edges remain.",
            Status::Alpha => "You can technically play it, but it isn't comfortable yet, and pieces may be missing.",
            Status::PreAlpha => "It runs, but it's probably a bad experience. Very rough edges remain.",
            Status::InProgress => "Works, but pieces are missing.",
            Status::Experimental => "Probably runs, but rough.",
        }
    }
}

/// When the game has to be running.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Launch {
    /// The launcher may start it through Steam, then waits this long for its menu.
    Auto { settle_seconds: u64 },
    /// The player starts it (and gets into gameplay) first.
    Manual,
}

/// One option's kind and default.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Switch { default: bool },
    /// `(id, label)` pairs; `default` is an id. The window offers the first `window` choices; the
    /// rest are taken on the command line only.
    Choice { choices: &'static [(&'static str, &'static str)], default: &'static str, window: usize },
    Number { min: f64, max: f64, step: f64, default: f64 },
    /// Free text (extra producer options).
    Text,
    /// A `WxH` size ([`parse_size`]), or nothing (the default).
    Size,
}

/// A choice the window offers in full.
pub const fn choice(choices: &'static [(&'static str, &'static str)], default: &'static str) -> Kind {
    Kind::Choice { choices, default, window: choices.len() }
}

/// Where an option is offered: a tab of the window ([`TABS`]), or the command line only.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shown {
    Main,
    /// How the player plays: what they hold ([`PLAY_WITH`]), what aims, and how to do each thing
    /// in the game ([`HowTo`]), each with the options that change it.
    Controls,
    Hud,
    /// The arms and hands: finger tracking.
    Skeleton,
    /// How the game renders in VR: the stereo method, frame generation, the upscaler per eye, depth
    /// to the headset, foveation, the render size ([`crate::rendering::place`] orders them).
    Rendering,
    /// What only this game has.
    Game,
    /// Monaka's own settings ([`MONAKA`]): the same for every game, saved once.
    Monaka,
    /// On the command line only (`monaka_play`, the `Play-*.ps1` scripts).
    Line,
}

/// The window's tabs, in order, and their names; a game shows the ones it has options in.
pub const TABS: [(Shown, &str); 7] = [
    (Shown::Main, "Main"),
    (Shown::Controls, "Controls"),
    (Shown::Hud, "HUD"),
    (Shown::Skeleton, "Skeleton"),
    (Shown::Rendering, "Rendering"),
    (Shown::Game, "Game"),
    (Shown::Monaka, "Monaka"),
];

/// Whether an option's `value` is what a condition `wanted` asks for: that value, or with a
/// leading `!` any value but it (`!vr`: any device but VR controllers).
pub fn matches(value: &str, wanted: &str) -> bool {
    match wanted.strip_prefix('!') {
        Some(not) => value != not,
        None => value == wanted,
    }
}

/// A condition's value without its `!`: the value of the option it names.
pub fn condition_value(wanted: &str) -> &str {
    wanted.strip_prefix('!').unwrap_or(wanted)
}

/// One option as the window shows it, the command line takes it (`id=value`) and the producer
/// reads it (`id=value` in its options file).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Opt {
    pub id: &'static str,
    pub label: &'static str,
    /// One line under the control.
    pub help: &'static str,
    pub kind: Kind,
    pub shown: Shown,
    /// Offered only while another option (`id`) has this value ([`Values::applies`]; `!value`:
    /// any other, [`matches`]).
    pub when: Option<(&'static str, &'static str)>,
    /// Written to the producer's options file (else the launcher consumes it itself: starting the
    /// game, the headset resolution, extra text).
    pub producer: bool,
    /// While this option has a value, another (a switch) is off: declared on the cause, so the
    /// setting that changes something says so ([`Opt::turns_off`]).
    pub turns_off: Option<TurnsOff>,
    /// A switch that works only with an eye size settled (the headset's or a fixed one): without
    /// one the launcher writes it off ([`Opt::needs_eye_size`]).
    pub needs_eye_size: bool,
    /// A switch that works only with files another installed Steam game brings
    /// ([`Opt::needs_steam_files`]).
    pub needs_files: Option<SteamFiles>,
    /// While this option has the first value, the viewer gets these arguments
    /// ([`Opt::viewer_args`]).
    pub viewer: Option<(&'static str, &'static [&'static str])>,
    /// Shown to the player when a run has an eye size settled, `{size}` replaced by it
    /// ([`Opt::eye_size_note`]).
    pub eye_size_note: Option<&'static str>,
    /// A shared option's feature: only the component that provides it may offer the option, since
    /// that component is what honours it ([`Opt::owned_by`], `monaka_producer::game`). `None` for
    /// a game's own options and the launcher's. The pack text does not carry it: the
    /// launcher has no use for it, and a parsed option gets it back from [`SHARED`] by id.
    pub owner: Option<Feature>,
}

/// Files another Steam game installs that an option needs: `file` in `path` under that game's
/// install folder (`steamapps\common\<game_folder>`), in any Steam library. Found, the launcher
/// writes `line=<that folder>` for the producer; not found, it writes the option off and tells the
/// player `missing`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SteamFiles {
    pub game_folder: &'static str,
    pub path: &'static str,
    pub file: &'static str,
    pub line: &'static str,
    pub missing: &'static str,
}

/// While the option it is declared on has `value` (`!value`: any other, [`matches`]), the switch
/// `target` is off whatever it is set to, for `reason` (one sentence the window adds under the
/// greyed-out target; empty when the rule says enough).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TurnsOff {
    pub value: &'static str,
    pub target: &'static str,
    pub reason: &'static str,
}

impl Opt {
    /// The option offered only while option `id` is `value`.
    pub const fn only_when(self, id: &'static str, value: &'static str) -> Opt {
        Opt { when: Some((id, value)), ..self }
    }

    /// While this option is `value`, the switch `target` is off, for `reason`. The window greys
    /// the target out and names the rule under both; the launcher writes it off; the producer reads it
    /// off ([`Manifest::turned_off`]).
    pub const fn turns_off(self, value: &'static str, target: &'static str, reason: &'static str) -> Opt {
        Opt { turns_off: Some(TurnsOff { value, target, reason }), ..self }
    }

    /// The switch works only with an eye size settled; without one it is written off.
    pub const fn needs_eye_size(self) -> Opt {
        Opt { needs_eye_size: true, ..self }
    }

    /// The switch works only with `files` from another installed Steam game.
    pub const fn needs_steam_files(self, files: SteamFiles) -> Opt {
        Opt { needs_files: Some(files), ..self }
    }

    /// While the option is `value`, the viewer gets `args`; `"*"`: at any value but the default, the
    /// viewer gets `args` and then the value.
    pub const fn viewer_args(self, value: &'static str, args: &'static [&'static str]) -> Opt {
        Opt { viewer: Some((value, args)), ..self }
    }

    /// Shown to the player when a run has an eye size settled (`{size}`: it, `WxH`).
    pub const fn eye_size_note(self, note: &'static str) -> Opt {
        Opt { eye_size_note: Some(note), ..self }
    }

    /// The option on tab `shown`.
    pub const fn on(self, shown: Shown) -> Opt {
        Opt { shown, ..self }
    }

    /// The option with another default (a choice id or a number, as the kind takes).
    pub const fn default_choice(self, default: &'static str) -> Opt {
        let Kind::Choice { choices, window, .. } = self.kind else { panic!("not a choice") };
        Opt { kind: Kind::Choice { choices, default, window }, ..self }
    }

    pub const fn default_number(self, default: f64) -> Opt {
        let Kind::Number { min, max, step, .. } = self.kind else { panic!("not a number") };
        Opt { kind: Kind::Number { min, max, step, default }, ..self }
    }

    pub const fn default_switch(self, default: bool) -> Opt {
        let Kind::Switch { .. } = self.kind else { panic!("not a switch") };
        Opt { kind: Kind::Switch { default }, ..self }
    }

    /// A shared option that only the component providing `feature` honours.
    pub const fn owned_by(self, feature: Feature) -> Opt {
        Opt { owner: Some(feature), ..self }
    }

    /// The option with this game's own help line (a shared option adjusted for what it means here).
    pub const fn help(self, help: &'static str) -> Opt {
        Opt { help, ..self }
    }

    /// A number option with this game's own range.
    pub const fn range(self, min: f64, max: f64) -> Opt {
        let Kind::Number { step, default, .. } = self.kind else { panic!("not a number") };
        Opt { kind: Kind::Number { min, max, step, default }, ..self }
    }

    /// The option offered whatever the other options say (a shared option's condition dropped: the
    /// finger options' "only with motion controls" in a game with no `aim` choice).
    pub const fn always(self) -> Opt {
        Opt { when: None, ..self }
    }

    /// The option the launcher consumes itself (never written for the producer).
    pub const fn launcher(self) -> Opt {
        Opt { producer: false, ..self }
    }

    /// The default as the producer reads it (`Options::number` and friends).
    pub fn default_text(&self) -> String {
        match self.kind {
            Kind::Switch { default } => (if default { "1" } else { "0" }).to_owned(),
            Kind::Choice { default, .. } => default.to_owned(),
            Kind::Number { default, .. } => number_text(default),
            Kind::Text | Kind::Size => String::new(),
        }
    }
}

pub const fn opt(id: &'static str, label: &'static str, help: &'static str, kind: Kind) -> Opt {
    Opt { id, label, help, kind, shown: Shown::Main, when: None, producer: true, turns_off: None, needs_eye_size: false, needs_files: None, viewer: None, eye_size_note: None, owner: None }
}

pub const fn line(id: &'static str, label: &'static str, help: &'static str, kind: Kind) -> Opt {
    Opt { id, label, help, kind, shown: Shown::Line, when: None, producer: true, turns_off: None, needs_eye_size: false, needs_files: None, viewer: None, eye_size_note: None, owner: None }
}

/// The `executables` of a pack that serves any game on its engine: the player picks the game with
/// the [`GAME_OPTION`] option, and the producer checks the engine itself when it starts.
pub const ANY_EXECUTABLE: &str = "*";
/// The option that picks the game of an [`ANY_EXECUTABLE`] pack: an executable's file name, or
/// [`OTHER_GAME`] for the one typed in the [`OTHER_GAME_OPTION`] option.
pub const GAME_OPTION: &str = "game";
pub const OTHER_GAME: &str = "other";
pub const OTHER_GAME_OPTION: &str = "game_other";

/// One thing the Controls tab says how to do in the game ("Tools and weapons wheels"): the ways
/// that hold with the player's settings, and under them the options that change it. Nothing here
/// rebinds anything: it tells the player what the game and Monaka already do.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HowTo {
    pub action: &'static str,
    /// Each a sentence or two, shown while its conditions hold, in order.
    pub ways: &'static [Way],
    /// The options that change it (their ids), shown under it.
    pub options: &'static [&'static str],
}

/// One way to do a [`HowTo`]: its text, shown while every condition `(option id, value)` holds
/// ([`matches`]: `!value` is any other).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Way {
    pub text: &'static str,
    pub when: &'static [(&'static str, &'static str)],
}

/// A way shown whatever the settings.
pub const fn always(text: &'static str) -> Way {
    Way { text, when: &[] }
}

/// A way shown while every condition holds.
pub const fn way(text: &'static str, when: &'static [(&'static str, &'static str)]) -> Way {
    Way { text, when }
}

/// One game, as its crate declares it.
#[derive(Debug, PartialEq)]
pub struct Manifest {
    /// Short id (`dl1`), used on the command line and in settings.
    pub id: &'static str,
    pub name: &'static str,
    pub status: Status,
    /// Executable file names this game runs as (the producer's `MONAKA_HOST`).
    pub executables: &'static [&'static str],
    pub steam_app: u32,
    /// Producer DLL stem (`monaka_dl1` for `monaka_dl1.dll`).
    pub producer: &'static str,
    /// The producer's log file name in the run folder.
    pub log: &'static str,
    /// The `key=value` options file beside the DLL copy.
    pub options_file: &'static str,
    /// Run folder prefix (`runs\<prefix>-<version>`).
    pub run_prefix: &'static str,
    pub launch: Launch,
    /// Launch options the game must not run with: (text in its command line, why), checked
    /// case-insensitively before attaching.
    pub refused_arguments: &'static [(&'static str, &'static str)],
    /// What to set in the game before playing.
    pub before: &'static str,
    pub options: &'static [Opt],
    /// Third-party notices (in the launcher's licence folder) copied into each run folder.
    pub licences: &'static [&'static str],
    /// With motion controls, the left hand aims its own tools or powers: the viewer draws a ring
    /// on the left controller.
    pub left_hand_aims: bool,
    /// With motion controls, the weapon in the aiming hand gets a ring where it points (not for
    /// a sword, which aims nowhere).
    pub weapon_ring: bool,
    /// How to do things in the game, for the Controls tab, in the order shown.
    pub controls: &'static [HowTo],
}

impl Manifest {
    /// A game's manifest before its own fields: started by hand (`launch` manual), no refused
    /// arguments, no licences to show, neither hand aims nor a weapon ring for the viewer. A game
    /// writes its own and takes the rest by struct update (`..Manifest::GAME`); [`Manifest::check`]
    /// refuses one that leaves its own empty.
    pub const GAME: Manifest = Manifest {
        id: "",
        name: "",
        status: Status::Experimental,
        executables: &[],
        steam_app: 0,
        producer: "",
        log: "",
        options_file: "",
        run_prefix: "",
        launch: Launch::Manual,
        refused_arguments: &[],
        before: "",
        options: &[],
        licences: &[],
        left_hand_aims: false,
        weapon_ring: false,
        controls: &[],
    };

    pub fn option(&self, id: &str) -> Option<&Opt> {
        self.options.iter().find(|o| o.id.eq_ignore_ascii_case(id))
    }

    /// The default of a number option, when the game offers it.
    pub fn number_default(&self, id: &str) -> Option<f64> {
        match self.option(id)?.kind {
            Kind::Number { default, .. } => Some(default),
            _ => None,
        }
    }

    /// The default of a switch option, when the game offers it.
    pub fn switch_default(&self, id: &str) -> Option<bool> {
        match self.option(id)?.kind {
            Kind::Switch { default } => Some(default),
            _ => None,
        }
    }

    /// The default of a choice option, when the game offers it.
    pub fn choice_default(&self, id: &str) -> Option<&'static str> {
        match self.option(id)?.kind {
            Kind::Choice { default, .. } => Some(default),
            _ => None,
        }
    }

    /// The option that turns switch `id` off now, and its rule: `raw` gives an option's value as
    /// set (`None`: its default), as the window's settings or the producer's options file hold it.
    pub fn turned_off(&self, id: &str, raw: impl Fn(&str) -> Option<String>) -> Option<(&Opt, TurnsOff)> {
        self.options.iter().find_map(|cause| {
            let rule = cause.turns_off.filter(|r| r.target == id)?;
            let mut values = Values::default();
            if let Some(value) = raw(cause.id) {
                values.set(cause.id, value);
            }
            matches(&values.text(self, cause.id), rule.value).then_some((cause, rule))
        })
    }

    /// Whether option `id` exists and `value` is one of its values as written.
    fn takes(&self, id: &str, value: &str) -> bool {
        let mut values = Values::default();
        values.set(id, value);
        self.option(id).is_some() && values.text(self, id) == value
    }

    /// Whether this pack serves any game on its engine ([`ANY_EXECUTABLE`]).
    pub fn any_game(&self) -> bool {
        self.executables == [ANY_EXECUTABLE]
    }

    /// The executables the game runs as with `values`: the manifest's own, or for a pack serving
    /// any game, the one its [`GAME_OPTION`] picks (none while nothing is picked or typed).
    pub fn runs_as(&self, values: &Values) -> Vec<String> {
        if !self.any_game() {
            return self.executables.iter().map(|e| (*e).to_owned()).collect();
        }
        let picked = values.text(self, GAME_OPTION);
        let picked = if picked == OTHER_GAME { values.text(self, OTHER_GAME_OPTION) } else { picked };
        let name = picked.trim().rsplit(['\\', '/']).next().unwrap_or("").trim_matches('"').to_owned();
        (!name.is_empty() && name != OTHER_GAME).then_some(name).into_iter().collect()
    }

    /// Checks the table is well formed: every default offered and in range, the launch options
    /// only where the launcher may start the game. The reason when it is not.
    pub fn check(&self) -> Result<(), String> {
        for (field, value) in [("id", self.id), ("name", self.name), ("log", self.log), ("options_file", self.options_file), ("run_prefix", self.run_prefix)] {
            if value.is_empty() {
                return Err(format!("{}: no {field}", self.id));
            }
        }
        if !self.producer.starts_with("monaka_") {
            return Err(format!("{}: producer {} is not monaka_*", self.id, self.producer));
        }
        if self.executables.is_empty() {
            return Err(format!("{}: no executables", self.id));
        }
        if self.any_game() && self.option(GAME_OPTION).is_none() {
            return Err(format!("{}: serves any game but has no {GAME_OPTION} option to pick it", self.id));
        }
        if self.id.eq_ignore_ascii_case(MONAKA.id) {
            return Err(format!("{}: that id belongs to Monaka's own settings", self.id));
        }
        for opt in self.options {
            // One of Monaka's own settings its producer reads, listed as Monaka has it; no other.
            let read = MONAKA.option(opt.id).is_some_and(|own| own.producer && own == opt);
            if (opt.shown == Shown::Monaka || MONAKA.option(opt.id).is_some()) && !read {
                return Err(format!("{}.{}: Monaka's own settings are not a game's", self.id, opt.id));
            }
        }
        for opt in self.options {
            match opt.kind {
                Kind::Choice { choices, default, window } => {
                    if !choices[..window.min(choices.len())].iter().any(|(id, _)| *id == default) {
                        return Err(format!("{}.{}: the window cannot show the default {default}", self.id, opt.id));
                    }
                }
                Kind::Number { min, max, default, .. } => {
                    if !(min..=max).contains(&default) {
                        return Err(format!("{}.{}: default {default} outside {min}..={max}", self.id, opt.id));
                    }
                }
                _ => {}
            }
            if let Some((id, _)) = opt.when
                && self.option(id).is_none()
            {
                return Err(format!("{}.{}: depends on unknown option {id}", self.id, opt.id));
            }
        }
        for opt in self.options {
            let Some(rule) = opt.turns_off else { continue };
            let Some(target) = self.option(rule.target) else {
                return Err(format!("{}.{}: turns off unknown option {}", self.id, opt.id, rule.target));
            };
            if !matches!(target.kind, Kind::Switch { .. }) {
                return Err(format!("{}.{}: turns off {}, which is not a switch", self.id, opt.id, rule.target));
            }
            // One level: a switch turned off turns nothing off itself, and nothing turns itself off.
            if target.turns_off.is_some() || rule.target == opt.id {
                return Err(format!("{}.{}: turns off {}, which turns off another option", self.id, opt.id, rule.target));
            }
            if !self.takes(opt.id, condition_value(rule.value)) {
                return Err(format!("{}.{}: turns {} off at {}, not one of its values", self.id, opt.id, rule.target, rule.value));
            }
        }
        let mut tuned: Vec<&str> = Vec::new();
        for how in self.controls {
            for way in how.ways {
                for (id, value) in way.when {
                    if !self.takes(id, condition_value(value)) {
                        return Err(format!("{}: \"{}\" waits on {id}={value}, not an option's value", self.id, how.action));
                    }
                }
            }
            for id in how.options {
                if self.option(id).is_none() {
                    return Err(format!("{}: \"{}\" shows unknown option {id}", self.id, how.action));
                }
                if tuned.contains(id) {
                    return Err(format!("{}: option {id} is under two controls", self.id));
                }
                tuned.push(id);
            }
        }
        for opt in self.options {
            let needs = opt.needs_eye_size || opt.needs_files.is_some();
            if needs && !matches!(opt.kind, Kind::Switch { .. }) {
                return Err(format!("{}.{}: only a switch can need something", self.id, opt.id));
            }
            if let Some((value, _)) = opt.viewer.filter(|(value, _)| *value != "*") {
                let mut values = Values::default();
                values.set(opt.id, value);
                if values.text(self, opt.id) != value {
                    return Err(format!("{}.{}: passes viewer arguments at {value}, not one of its values", self.id, opt.id));
                }
            }
        }
        let starts = self.option("launch").is_some() || self.option("settle_seconds").is_some();
        match self.launch {
            Launch::Auto { settle_seconds } => {
                if self.option("launch").is_none() {
                    return Err(format!("{}: started through Steam but offers no launch option", self.id));
                }
                if self.number_default("settle_seconds") != Some(settle_seconds as f64) {
                    return Err(format!("{}: settle_seconds default is not the launch's {settle_seconds}", self.id));
                }
            }
            Launch::Manual if starts => return Err(format!("{}: launch options on a game the player starts", self.id)),
            Launch::Manual => {}
        }
        Ok(())
    }
}

/// The chosen values of one game's options, by id. Anything missing or invalid reads as its
/// default.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Values(pub BTreeMap<String, String>);

impl Values {
    pub fn set(&mut self, id: &str, value: impl Into<String>) {
        self.0.insert(id.to_owned(), value.into());
    }

    /// Whether `opt` means anything with these values (its [`Opt::when`] holds).
    pub fn applies(&self, game: &Manifest, opt: &Opt) -> bool {
        opt.when.is_none_or(|(id, value)| matches(&self.text(game, id), value))
    }

    /// The choices of `opt` the window offers with these values: its window's, less the aims the
    /// device in hand cannot make ([`aim_fits`]).
    pub fn shown_choices(&self, game: &Manifest, opt: &Opt) -> Vec<(&'static str, &'static str)> {
        let Kind::Choice { choices, window, .. } = opt.kind else { return Vec::new() };
        let device = self.text(game, PLAY_WITH.id);
        choices[..window.min(choices.len())].iter().copied().filter(|(id, _)| opt.id != AIM_ID || aim_fits(&device, id)).collect()
    }

    /// The texts of `how`'s ways that hold with these values, in order.
    pub fn ways(&self, game: &Manifest, how: &HowTo) -> Vec<&'static str> {
        how.ways.iter().filter(|w| w.when.iter().all(|(id, value)| matches(&self.text(game, id), value))).map(|w| w.text).collect()
    }

    /// The value of `id` as text, its default when unset or invalid.
    pub fn text(&self, game: &Manifest, id: &str) -> String {
        let Some(opt) = game.option(id) else { return String::new() };
        let raw = self.0.get(id).map(|v| v.trim());
        match opt.kind {
            Kind::Switch { default } => {
                let on = raw.and_then(parse_switch).unwrap_or(default) && self.turned_off(game, id).is_none();
                (if on { "1" } else { "0" }).to_owned()
            }
            Kind::Choice { choices, default, .. } => {
                let chosen = raw.and_then(|r| choices.iter().find(|(id, _)| id.eq_ignore_ascii_case(r))).map_or(default, |(id, _)| id);
                // An aim the device in hand cannot make gives way to the default, else the first
                // that it can.
                if id == AIM_ID {
                    let device = self.text(game, PLAY_WITH.id);
                    if !aim_fits(&device, chosen) {
                        let fallback = if aim_fits(&device, default) { Some(default) } else { choices.iter().map(|(id, _)| *id).find(|id| aim_fits(&device, id)) };
                        return fallback.unwrap_or(chosen).to_owned();
                    }
                }
                chosen.to_owned()
            }
            Kind::Number { min, max, default, .. } => {
                let n = raw.and_then(|r| r.parse::<f64>().ok()).filter(|n| (min..=max).contains(n)).unwrap_or(default);
                number_text(n)
            }
            Kind::Text => raw.unwrap_or("").to_owned(),
            Kind::Size => raw.and_then(parse_size).map(|(w, h)| format!("{w}x{h}")).unwrap_or_default(),
        }
    }

    /// The option that turns switch `id` off with these values, and its rule.
    pub fn turned_off<'g>(&self, game: &'g Manifest, id: &str) -> Option<(&'g Opt, TurnsOff)> {
        game.turned_off(id, |cause| self.0.get(cause).cloned())
    }

    /// The value of a [`Kind::Size`] option, when one is set.
    pub fn size(&self, game: &Manifest, id: &str) -> Option<(u32, u32)> {
        parse_size(&self.text(game, id))
    }

    pub fn switch(&self, game: &Manifest, id: &str) -> bool {
        self.text(game, id) == "1"
    }

    pub fn number(&self, game: &Manifest, id: &str) -> f64 {
        self.text(game, id).parse().unwrap_or(0.0)
    }

    /// Checks `id=value` for the command line: a known option and a valid value.
    pub fn set_checked(&mut self, game: &Manifest, id: &str, value: &str) -> Result<(), String> {
        let opt = game.option(id).ok_or_else(|| format!("{} has no option {id}", game.name))?;
        let value = value.trim();
        let ok = match opt.kind {
            Kind::Switch { .. } => parse_switch(value).is_some(),
            Kind::Choice { choices, .. } => choices.iter().any(|(c, _)| c.eq_ignore_ascii_case(value)),
            Kind::Number { min, max, .. } => value.parse::<f64>().is_ok_and(|n| (min..=max).contains(&n)),
            Kind::Text => true,
            Kind::Size => value.is_empty() || parse_size(value).is_some(),
        };
        if !ok {
            return Err(format!("{id}={value} is not valid; {}", describe(opt)));
        }
        self.set(id, value);
        Ok(())
    }

    /// The producer's options file for these values: one `id=value` line per producer option
    /// that applies, then `extra`'s `key=value` pairs (which may override). Sizes the launcher
    /// settled (`eye_size`) and anything PC-dependent are the caller's to add.
    pub fn producer_lines(&self, game: &Manifest) -> Vec<String> {
        let mut lines: Vec<String> = game
            .options
            .iter()
            .filter(|opt| opt.producer && !matches!(opt.kind, Kind::Text | Kind::Size) && self.applies(game, opt))
            .map(|opt| format!("{}={}", opt.id, self.text(game, opt.id)))
            .collect();
        lines.extend(self.text(game, "extra").split_whitespace().filter(|p| p.contains('=')).map(str::to_owned));
        lines
    }
}

pub fn parse_switch(text: &str) -> Option<bool> {
    match text.to_ascii_lowercase().as_str() {
        "1" | "on" | "yes" | "true" => Some(true),
        "0" | "off" | "no" | "false" => Some(false),
        _ => None,
    }
}

/// A number without trailing zeros, with `.` as the decimal point.
pub fn number_text(n: f64) -> String {
    let text = format!("{n:.4}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// What values an option takes, for help texts and errors.
pub fn describe(opt: &Opt) -> String {
    match opt.kind {
        Kind::Switch { default } => format!("on or off (default {})", if default { "on" } else { "off" }),
        Kind::Choice { choices, default, .. } => {
            format!("one of {} (default {default})", choices.iter().map(|(id, _)| *id).collect::<Vec<_>>().join(", "))
        }
        Kind::Number { min, max, default, .. } => {
            format!("{} to {} (default {})", number_text(min), number_text(max), number_text(default))
        }
        Kind::Text => "any text".to_owned(),
        Kind::Size => "a size WxH from 256x256 to 8192x8192 (default none)".to_owned(),
    }
}

/// The options the launcher's defaults would make, and once more for each value another option
/// waits on (`Opt::when`: the FSR settings behind `fsr=1`, the aiming hand behind
/// `aim=controller`). With `read` having parsed each, the ids it never looked at: a producer
/// test's check that it reads every option its manifest offers.
pub fn unread_options(game: &Manifest, mut read: impl FnMut(&Options)) -> Vec<String> {
    let mut variants: Vec<Option<(&str, &str)>> = vec![None];
    for opt in game.options {
        if let Some(when) = opt.when
            && !variants.contains(&Some(when))
        {
            variants.push(Some(when));
        }
    }
    let mut seen: Vec<String> = Vec::new();
    for variant in variants {
        let text: Vec<String> = game
            .options
            .iter()
            .filter(|o| o.producer)
            .map(|o| match variant {
                Some((id, value)) if id == o.id => format!("{id}={value}"),
                _ => format!("{}={}", o.id, o.default_text()),
            })
            .collect();
        let options = Options::parse(&text.join("\n"));
        read(&options);
        seen.extend(options.keys_read());
    }
    game.options.iter().filter(|o| o.producer && !seen.iter().any(|k| k == o.id)).map(|o| o.id.to_owned()).collect()
}

// ---- The producer's host export ----

/// The executables of `manifests` joined with `;`, NUL-terminated: its length, for the export's
/// array size.
pub const fn host_len(manifests: &[Manifest]) -> usize {
    let mut n = 0;
    let mut i = 0;
    while i < manifests.len() {
        let mut j = 0;
        while j < manifests[i].executables.len() {
            n += manifests[i].executables[j].len() + 1;
            j += 1;
        }
        i += 1;
    }
    if n == 0 { 1 } else { n }
}

/// The executables of `manifests` joined with `;`, NUL-terminated (`N` is [`host_len`]).
pub const fn host_bytes<const N: usize>(manifests: &[Manifest]) -> [u8; N] {
    let mut out = [0u8; N];
    let mut at = 0;
    let mut i = 0;
    while i < manifests.len() {
        let mut j = 0;
        while j < manifests[i].executables.len() {
            let bytes = manifests[i].executables[j].as_bytes();
            if at > 0 {
                out[at - 1] = b';';
            }
            let mut k = 0;
            while k < bytes.len() {
                out[at + k] = bytes[k];
                k += 1;
            }
            at += bytes.len() + 1;
            j += 1;
        }
        i += 1;
    }
    out
}

/// The executables of `manifests` joined with `;` (what `MONAKA_HOST` says, for the host check).
pub fn host_text(manifests: &[Manifest]) -> String {
    manifests.iter().flat_map(|m| m.executables.iter().copied()).collect::<Vec<_>>().join(";")
}

// ---- Settings every game shares ----
//
// One list: a setting has the same id, name, help and meaning in every game. A game lists the ones
// its producer supports, adjusting a default where its own differs (`default_number`...), and
// `monaka_core::options::Shared` reads them. What is a fact about a game (its eye timing, how much
// head movement it takes) is fixed per game, not a setting; those stay on the command line.

/// What aims. Motion controls: the arms and what they hold follow the controllers, swings attack
/// and the left hand aims its own tools or powers (with a ring). The other aims get little testing
/// where motion controls exist.
pub const AIM_MOTION: (&str, &str) = ("hand_rig", "Motion controls");
pub const AIM_LOOK: (&str, &str) = ("head", "Look-based (less tested)");
pub const AIM_POINT: (&str, &str) = ("controller", "Pointing-based (less tested)");
pub const AIM_MOUSE: (&str, &str) = ("off", "Mouse / stick only");

/// The `aim` option's ids, as the producers read them.
pub mod aims {
    pub const HAND_RIG: &str = "hand_rig";
    pub const HEAD: &str = "head";
    pub const CONTROLLER: &str = "controller";
    pub const OFF: &str = "off";
}

/// The `aim` option's id.
pub const AIM_ID: &str = "aim";

/// The `aim` option over `choices` (from [`AIM_MOTION`] and friends), `default` one of them. The
/// window offers the ones the device in hand can make ([`aim_fits`]), and none when that leaves
/// one.
pub const fn aim(choices: &'static [(&'static str, &'static str)], default: &'static str) -> Opt {
    opt(AIM_ID, "Aim", "What points your weapon.", choice(choices, default)).on(Shown::Controls)
}

/// What the player holds ([`PLAY_WITH`]).
pub mod plays {
    pub const VR: &str = "vr";
    pub const KEYBOARD: &str = "keyboard";
    pub const GAMEPAD: &str = "gamepad";
}

/// What the player plays with: the VR controllers, or a mouse and keyboard or a gamepad of their
/// own. The first option of the Controls tab: what aims ([`aim_fits`]) and how each thing is done
/// ([`HowTo`]) follow it. Away from the VR controllers they are not the game's gamepad.
pub const PLAY_WITH: Opt = opt(
    "play_with",
    "Play with",
    "What you hold.",
    choice(&[(plays::VR, "VR controllers"), (plays::KEYBOARD, "Mouse and keyboard"), (plays::GAMEPAD, "Gamepad")], plays::VR),
)
.on(Shown::Controls)
.launcher()
.turns_off("!vr", "controller_pad", "");

/// Whether `device` ([`PLAY_WITH`]) can make aim `aim`: motion controls and a pointing controller
/// need the VR controllers; the look and the mouse or stick aim with anything.
pub fn aim_fits(device: &str, aim: &str) -> bool {
    device != plays::KEYBOARD && device != plays::GAMEPAD || !matches!(aim, aims::HAND_RIG | aims::CONTROLLER)
}

/// Which controller a pointing aim follows. Only for `aim=controller`: with motion controls the
/// weapon is in the games' right hand, so the right controller aims.
pub const AIM_HAND: Opt =
    opt("aim_hand", "Aiming hand", "The controller that aims.", choice(&[("right", "Right"), ("left", "Left")], "right"))
        .on(Shown::Controls)
        .only_when("aim", "controller")
        .owned_by(Feature::Look);

/// The free hands' fingers follow the controllers' finger sensing, when the headset has any.
pub const FINGER_TRACKING: Opt =
    opt("finger_tracking", "Finger tracking", "Your fingers move the in-game fingers.", Kind::Switch { default: true })
        .on(Shown::Skeleton)
        .only_when("aim", "hand_rig")
        .owned_by(Feature::FingerRig);

/// With finger tracking (controllers that sense the fingers): each finger open or a fist.
pub const BINARY_FINGERS: Opt =
    opt("finger_binary", "Binary fingers", "Fingers register as fully open or fully closed.", Kind::Switch { default: false })
        .on(Shown::Skeleton)
        .only_when("aim", "hand_rig")
        .owned_by(Feature::FingerRig);

/// The sizes `render_size` offers: the headset's own (SteamVR's recommended, its supersampling
/// included), common headsets' panels, or the game's own resolution.
pub const RENDER_SIZES: &[(&str, &str)] = &[
    ("headset", "Let the headset pick"),
    ("2160x2160", "Steam Frame (2160 x 2160)"),
    ("2064x2208", "Quest 3 (2064 x 2208)"),
    ("1832x1920", "Quest 2 (1832 x 1920)"),
    ("1440x1600", "Valve Index (1440 x 1600)"),
    ("game", "The game's own"),
];

/// The resolution each eye renders at while VR runs (the launcher settles it into `eye_size`).
pub const RENDER_SIZE: Opt = opt(
    "render_size",
    "Headset resolution",
    "The resolution each eye renders at while VR runs: the headset's is sharper, a smaller one runs faster.",
    choice(RENDER_SIZES, "headset"),
)
.on(Shown::Rendering)
.launcher();

/// A render size of the player's own, beside [`render_size`].
pub const OWN_EYE_SIZE: Opt = line("eye_size", "Render size (WxH)", "A render size of your own; replaces render_size.", Kind::Size).launcher();

/// The HUD's pieces on the hands instead of in front of the eyes.
pub const WORLD_HUD: Opt =
    opt("world_hud", "Dynamic HUD", "Puts HUD pieces on your hands.", Kind::Switch { default: true }).on(Shown::Hud).owned_by(Feature::HudPanels);

/// The HUD's size, 1 being its size on a monitor (`default_number` for the game's own).
pub const HUD_SIZE: Opt = opt("hud_scale", "HUD size", "1 is the size on a monitor.", Kind::Number { min: 0.1, max: 1.0, step: 0.01, default: 0.5 })
    .on(Shown::Hud)
    .owned_by(Feature::HudLayer);

/// Runs on a game build other than the one Monaka was built for (`monaka_producer::require_build`
/// lets a mismatch through, logged): each hook still checks the exact code it patches and is left
/// out where that moved, but plain offsets are not checked, so features may misbehave or the game
/// may crash.
pub const OTHER_BUILDS: Opt = opt(
    "other_builds",
    "Allow other game versions",
    "Runs even if your game version isn't the one Monaka was built for. Features may not work and the game may crash.",
    Kind::Switch { default: false },
)
.on(Shown::Game);

/// The d-pad directions (`left,right`, ...) that open a choice the game keeps open only while the
/// d-pad is held: on a controller without a d-pad, pushed through the viewer's d-pad layer they
/// stay held until a pick is made, so both thumbs are free for the face buttons.
pub const fn dpad_hold(directions: &'static [&'static str]) -> Opt {
    opt(
        "dpad_hold",
        "Hold d-pad menus",
        "D-pad menus stay open until you pick with A, B, X or Y.",
        Kind::Switch { default: true },
    )
    .on(Shown::Controls)
    .launcher()
    .viewer_args("1", directions)
}

/// The held d-pad menus ([`dpad_hold`]) by reaching over a shoulder too (the viewer's
/// `--menu-reach`): the left hand opens the d-pad-left menu, the right hand the d-pad-right one, a
/// stick flick picks. `help` says what the game's menus are.
pub const fn menu_reach(help: &'static str) -> Opt {
    opt(
        "menu_reach",
        "Menus by reach",
        help,
        choice(&[("same", "Pick with that hand's stick"), ("other", "Pick with the other hand's stick"), ("off", "Off")], "same"),
    )
    .on(Shown::Controls)
    .launcher()
    .only_when("dpad_hold", "1")
    .viewer_args("*", &["--menu-reach"])
}

/// How far behind the head a hand reaches to open a menu by reach ([`menu_reach`]), in cm (the
/// viewer's `--menu-reach-depth`).
pub const MENU_REACH_DEPTH: Opt = opt(
    "menu_reach_depth",
    "Menu reach depth (cm)",
    "How far behind your head a hand reaches before its menu opens.",
    Kind::Number { min: 5.0, max: 40.0, step: 1.0, default: 15.0 },
)
.on(Shown::Controls)
.launcher()
.only_when("dpad_hold", "1")
.viewer_args("*", &["--menu-reach-depth"]);

/// A layout test: the controllers at hand act like Quest's or Index's in the viewer (what those lack
/// hidden, the left grip standing in for their d-pad layer). Never for play.
pub const CONTROLLER_PROXY: Opt = opt(
    "controller_proxy",
    "Controller test",
    "Try another controller's layout. The left grip stands in for the thumbrest or trackpad touch.",
    choice(&[("off", "Off"), ("touch", "Quest controllers"), ("index", "Valve Index controllers")], "off"),
)
.on(Shown::Controls)
.launcher()
.viewer_args("*", &["--act-like"]);

pub const HUD_DISTANCE: Opt = opt("hud_distance", "HUD distance (m)", "How far away the HUD appears.", Kind::Number { min: 0.5, max: 20.0, step: 0.1, default: 2.0 })
    .on(Shown::Hud)
    .owned_by(Feature::HudLayer);

/// How much of the headset's position moves the view: `lean` (leaning, without the eyes' small
/// shift as the head turns), `full` or `off` (rotation only). Fixed at lean; for testing.
pub const HEAD_MOVEMENT: Opt =
    line("head_position", "Head movement", "lean, full or off (rotation only).", choice(&[("lean", "Lean"), ("full", "Full"), ("off", "Rotation only")], "lean"))
        .owned_by(Feature::Stereo);

/// Presents between choosing an eye and presenting it: each producer knows its game's
/// (`default_number`); for testing.
pub const LATENCY: Opt =
    line("latency", "Eye timing (frames)", "Overrides the game's own.", Kind::Number { min: 0.0, max: 8.0, step: 1.0, default: 2.0 }).owned_by(Feature::Stereo);

/// Used only when the headset reports no eye distance.
pub const EYE_SEPARATION: Opt =
    line("eye_separation", "Eye distance (m)", "Used when the headset reports none.", Kind::Number { min: 0.03, max: 0.09, step: 0.001, default: 0.064 })
        .owned_by(Feature::Stereo);

/// The character walks where the player steps (`monaka_body::roomscale`).
pub const ROOM_SCALE: Opt =
    opt("room_scale", "Room-scale movement", "Real steps and leans move your character.", Kind::Switch { default: true })
        .on(Shown::Controls)
        .only_when("aim", "hand_rig")
        .owned_by(Feature::RoomScale);

/// How the right stick turns the player where a producer turns it itself
/// (`monaka_core::pad::StickTurn::from_options`). A game that offers it may narrow when it shows.
pub const TURN: Opt =
    opt("turn", "Turning", "How the right stick turns you.", choice(&[("smooth", "Smooth"), ("snap", "Snap (30 degrees)")], "smooth")).on(Shown::Controls);

/// The VR controllers as the game's gamepad (`monaka_pad`).
pub const CONTROLLER_PAD: Opt =
    opt("controller_pad", "Controllers as gamepad", "Sticks and buttons act as the game's gamepad.", Kind::Switch { default: true })
        .on(Shown::Controls)
        .owned_by(Feature::Gamepad);

// ---- How to do things, for the Controls tab ----
//
// What Monaka itself does the same in every game, written once; a game lists these among its own
// ([`Manifest::controls`]) with the options they name.

/// The game's buttons on what the player holds: the VR controllers as an Xbox pad (the game's own
/// prompts name its buttons), and the d-pad of controllers without one. Names `controller_pad` and
/// `controller_proxy`.
pub const HOW_BUTTONS: HowTo = HowTo {
    action: "The game's buttons",
    ways: &[
        way("Xbox layout: grips are LB/RB, menu button is Start.", &[("play_with", plays::VR), ("controller_pad", "1")]),
        way(
            "D-pad on Quest: left thumb on the thumbrest + right stick (menu button is then View). On Index: touch the left trackpad + right stick; right trackpad click is View.",
            &[("play_with", plays::VR), ("controller_pad", "1")],
        ),
        way("As in the game.", &[("play_with", plays::KEYBOARD)]),
        way("As in the game.", &[("play_with", plays::GAMEPAD)]),
    ],
    options: &["controller_pad", "controller_proxy"],
};

/// Walking in the room, with motion controls. Names `room_scale`.
pub const HOW_WALK: HowTo = HowTo {
    action: "Walk",
    ways: &[],
    options: &["room_scale"],
};

/// Each eye's depth goes to the headset beside its image, for the runtime's own reprojection of
/// late frames; off for an A/B check against colour only.
pub const HEADSET_DEPTH: Opt = opt(
    "headset_depth",
    "Depth to headset",
    "Gives the headset each eye's depth, so when the game falls behind, the headset's own frame smoothing handles you moving your head, not only turning it.",
    Kind::Switch { default: true },
)
.on(Shown::Rendering)
.owned_by(Feature::DepthLayer);

/// Every shared option a part of a game's code honours (each with its owner): a game offers these
/// constants, never an option of its own under one of their ids.
pub const SHARED: &[Opt] = &[AIM_HAND, FINGER_TRACKING, BINARY_FINGERS, WORLD_HUD, HUD_SIZE, HUD_DISTANCE, HEAD_MOVEMENT, LATENCY, EYE_SEPARATION, ROOM_SCALE, CONTROLLER_PAD, HEADSET_DEPTH];

/// The owner of the shared option `id` ([`SHARED`]), for an option read back from a pack's text.
pub fn shared_owner(id: &str) -> Option<Feature> {
    SHARED.iter().find(|o| o.id == id).and_then(|o| o.owner)
}

// ---- Monaka's own settings ----
//
// Not a game's: saved once, offered on every game's Monaka tab and applied to every run. What they
// change happens in the viewer, so no producer reads them.

/// Steadies the controllers' poses before any game reads them (the viewer's
/// `--controller-smoothing`): 0 is off, higher steadies more; quick movements stay quick.
pub const CONTROLLER_SMOOTHING: Opt = opt(
    "controller_smoothing",
    "Controller smoothing",
    "Steadies small shakes in the controllers' tracking, in every game. Quick movements stay quick. 0 is off.",
    Kind::Number { min: 0.0, max: 10.0, step: 1.0, default: 3.0 },
)
.on(Shown::Monaka)
.launcher()
.viewer_args("*", &["--controller-smoothing"]);

/// Disabled view bobbing: Monaka's own setting (saved once, for every game), read by the producer of
/// each game that can (the game lists it among its options, unchanged: [`Manifest::check`]).
/// More detail in the producer's log, for a bug report. Not an options-file line: the launcher
/// leaves [`DIAGNOSTIC_MARKER`] in the run folder, and the window adds or removes it while VR runs,
/// so it can change mid-run.
pub const DIAGNOSTIC_LOGGING: Opt =
    opt("diagnostic_logging", "Diagnostic logging", "More detail in the logs, for bug reports. Stops by itself at 64 MB per run.", Kind::Switch { default: false })
        .on(Shown::Monaka)
        .launcher();

/// The run-folder file whose presence turns [`DIAGNOSTIC_LOGGING`] on in the producer.
pub const DIAGNOSTIC_MARKER: &str = "diagnostic-logging.on";

pub const DISABLE_VIEW_BOBBING: Opt = opt(
    "disable_view_bobbing",
    "Disable view bobbing",
    "The view stays level while you walk and run, instead of rising and falling with the character's steps, in every game that can.",
    Kind::Switch { default: true },
)
.on(Shown::Monaka);

/// Monaka's own settings as a manifest, so the window and the launcher treat them as they do a
/// game's options. It is never a game: no executables, never on offer. Those a game's producer
/// reads (written to its options file) are listed by that game too.
pub const MONAKA: Manifest = Manifest {
    id: "monaka",
    name: "Monaka",
    status: Status::Playable,
    executables: &[],
    steam_app: 0,
    producer: "",
    log: "",
    options_file: "",
    run_prefix: "",
    launch: Launch::Manual,
    refused_arguments: &[],
    before: "",
    options: &[CONTROLLER_SMOOTHING, DISABLE_VIEW_BOBBING, DIAGNOSTIC_LOGGING],
    licences: &[],
    left_hand_aims: false,
    weapon_ring: false,
    controls: &[],
};

/// Extra `key=value` producer options, appended to the options file (the game's config lists them).
pub const EXTRA: Opt = line("extra", "Extra producer options", "key=value pairs separated by spaces (see the game's config.rs).", Kind::Text).launcher();

/// For games the launcher may start through Steam (the window always may).
pub const LAUNCH: Opt = line("launch", "Start the game", "Start the game through Steam when it is not running; off refuses instead.", Kind::Switch { default: true }).launcher();

/// `settle_seconds` for a game started through Steam; its default is the game's [`Launch::Auto`].
pub const fn settle(max: f64, default: f64) -> Opt {
    line("settle_seconds", "Menu wait (s)", "How long a game started through Steam gets to reach its menu.", Kind::Number { min: 0.0, max, step: 1.0, default }).launcher()
}

#[cfg(test)]
mod tests {
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
    fn unread_options_are_found() {
        let unread = unread_options(&TEST, |o| {
            o.text("aim");
            o.number("hud_scale", 0.0, 1.0, 0.5);
        });
        assert_eq!(unread, ["aim_hand", "finger_tracking", "latency"]);
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
}
