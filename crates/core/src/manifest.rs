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

use crate::options::{Options, parse_size};
use std::collections::BTreeMap;

/// How finished a game's VR is, as the README's table says.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Playable,
    Beta,
    Alpha,
    InProgress,
    Experimental,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Playable => "Playable",
            Status::Beta => "Beta",
            Status::Alpha => "Alpha",
            Status::InProgress => "In Progress",
            Status::Experimental => "Experimental",
        }
    }

    pub fn meaning(self) -> &'static str {
        match self {
            Status::Playable => "You can play the whole game this way with no big problems.",
            Status::Beta => "You can probably play the whole game this way. Some rough edges remain.",
            Status::Alpha => "You can technically play it, but it isn't comfortable yet, and pieces may be missing.",
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
    /// What aims, and the resolution.
    Main,
    Hud,
    /// The arms and hands: finger tracking.
    Skeleton,
    /// What the game's engine offers (how it renders).
    Engine,
    /// What only this game has.
    Game,
    /// On the command line only (`monaka_play`, the `Play-*.ps1` scripts).
    Line,
}

/// The window's tabs, in order, and their names; a game shows the ones it has options in.
pub const TABS: [(Shown, &str); 5] = [(Shown::Main, "Main"), (Shown::Hud, "HUD"), (Shown::Skeleton, "Skeleton"), (Shown::Engine, "Engine"), (Shown::Game, "Game")];

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
    /// Offered only while another option (`id`) has this value ([`Values::applies`]).
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

/// While the option it is declared on has `value`, the switch `target` is off whatever it is set
/// to, for `reason` (one sentence: the window shows it under both options).
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
    /// the target out and says why under both; the launcher writes it off; the producer reads it
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
    Opt { id, label, help, kind, shown: Shown::Main, when: None, producer: true, turns_off: None, needs_eye_size: false, needs_files: None, viewer: None, eye_size_note: None }
}

pub const fn line(id: &'static str, label: &'static str, help: &'static str, kind: Kind) -> Opt {
    Opt { id, label, help, kind, shown: Shown::Line, when: None, producer: true, turns_off: None, needs_eye_size: false, needs_files: None, viewer: None, eye_size_note: None }
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
}

impl Manifest {
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

    /// Checks the table is well formed: every default offered and in range, the launch options
    /// only where the launcher may start the game. The reason when it is not.
    /// The option that turns switch `id` off now, and its rule: `raw` gives an option's value as
    /// set (`None`: its default), as the window's settings or the producer's options file hold it.
    pub fn turned_off(&self, id: &str, raw: impl Fn(&str) -> Option<String>) -> Option<(&Opt, TurnsOff)> {
        self.options.iter().find_map(|cause| {
            let rule = cause.turns_off.filter(|r| r.target == id)?;
            let mut values = Values::default();
            if let Some(value) = raw(cause.id) {
                values.set(cause.id, value);
            }
            (values.text(self, cause.id) == rule.value).then_some((cause, rule))
        })
    }

    pub fn check(&self) -> Result<(), String> {
        if !self.producer.starts_with("monaka_") {
            return Err(format!("{}: producer {} is not monaka_*", self.id, self.producer));
        }
        if self.executables.is_empty() {
            return Err(format!("{}: no executables", self.id));
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
            let mut values = Values::default();
            values.set(opt.id, rule.value);
            if values.text(self, opt.id) != rule.value {
                return Err(format!("{}.{}: turns {} off at {}, not one of its values", self.id, opt.id, rule.target, rule.value));
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
        opt.when.is_none_or(|(id, value)| self.text(game, id) == value)
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
                raw.and_then(|r| choices.iter().find(|(id, _)| id.eq_ignore_ascii_case(r))).map_or(default, |(id, _)| id).to_owned()
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

/// The `aim` option over `choices` (from [`AIM_MOTION`] and friends), `default` one of them.
pub const fn aim(choices: &'static [(&'static str, &'static str)], default: &'static str) -> Opt {
    opt("aim", "Aim", "What points your weapon.", choice(choices, default))
}

/// Which controller a pointing aim follows. Only for `aim=controller`: with motion controls the
/// weapon is in the games' right hand, so the right controller aims.
pub const AIM_HAND: Opt =
    opt("aim_hand", "Aiming hand", "The controller that aims.", choice(&[("right", "Right"), ("left", "Left")], "right")).only_when("aim", "controller");

/// The free hands' fingers follow the controllers' finger sensing, when the headset has any.
pub const FINGER_TRACKING: Opt =
    opt("finger_tracking", "Finger tracking", "Your fingers move the in-game fingers.", Kind::Switch { default: true }).on(Shown::Skeleton).only_when("aim", "hand_rig");

/// With finger tracking (controllers that sense the fingers): each finger open or a fist.
pub const BINARY_FINGERS: Opt =
    opt("finger_binary", "Binary fingers", "Fingers register as fully open or fully closed.", Kind::Switch { default: false }).on(Shown::Skeleton).only_when("aim", "hand_rig");

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

/// The resolution each eye renders at while VR runs (the launcher settles it into `eye_size`);
/// `help` says what matters in this game.
pub const fn render_size(help: &'static str) -> Opt {
    opt("render_size", "Headset resolution", help, choice(RENDER_SIZES, "headset")).launcher()
}

/// The HUD's pieces on the hands instead of in front of the eyes.
pub const WORLD_HUD: Opt = opt("world_hud", "Dynamic HUD", "Puts HUD pieces on your hands.", Kind::Switch { default: true }).on(Shown::Hud);

/// The HUD's size, 1 being its size on a monitor (`default_number` for the game's own).
pub const HUD_SIZE: Opt = opt("hud_scale", "HUD size", "1 is the size on a monitor.", Kind::Number { min: 0.1, max: 1.0, step: 0.01, default: 0.5 }).on(Shown::Hud);

/// Runs on a game build other than the one Monaka was measured on (`monaka_producer::require_build`
/// lets a mismatch through, logged): each hook still checks the exact code it patches and is left
/// out where that moved, but plain offsets are not checked, so features may misbehave or the game
/// may crash.
pub const OTHER_BUILDS: Opt = opt(
    "other_builds",
    "Allow other game versions",
    "Runs even if your game version isn't the one Monaka was built for. Features may not work and the game may crash.",
    Kind::Switch { default: false },
)
.on(Shown::Engine);

/// The d-pad directions (`left,right`, ...) that open a choice the game keeps open only while the
/// d-pad is held: on a controller without a d-pad, pushed through the viewer's d-pad layer they
/// stay held until a pick is made, so both thumbs are free for the face buttons.
pub const fn dpad_hold(directions: &'static [&'static str]) -> Opt {
    opt(
        "dpad_hold",
        "Hold d-pad menus",
        "For controllers without a d-pad (Quest): a d-pad menu stays open until you pick with A, B, X or Y.",
        Kind::Switch { default: true },
    )
    .on(Shown::Game)
    .launcher()
    .viewer_args("1", directions)
}

/// A layout test: the controllers at hand act like Quest's or Index's in the viewer (what those lack
/// hidden, the left grip standing in for their d-pad layer). Never for play.
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
    .on(Shown::Game)
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
.on(Shown::Game)
.launcher()
.only_when("dpad_hold", "1")
.viewer_args("*", &["--menu-reach-depth"]);

pub const CONTROLLER_PROXY: Opt = opt(
    "controller_proxy",
    "Controller test",
    "Your controllers act like another kind, to try its layout: Quest's have no d-pad or View button, Index's no d-pad. Your left grip stands in for the Quest thumbrest or the Index trackpad touch.",
    choice(&[("off", "Off"), ("touch", "Quest controllers"), ("index", "Valve Index controllers")], "off"),
)
.on(Shown::Engine)
.launcher()
.viewer_args("*", &["--act-like"]);

pub const HUD_DISTANCE: Opt = opt("hud_distance", "HUD distance (m)", "How far away the HUD appears.", Kind::Number { min: 0.5, max: 20.0, step: 0.1, default: 2.0 }).on(Shown::Hud);

/// How much of the headset's position moves the view: `lean` (leaning, without the eyes' small
/// shift as the head turns), `full` or `off` (rotation only). Fixed at lean; for testing.
pub const HEAD_MOVEMENT: Opt =
    line("head_position", "Head movement", "lean, full or off (rotation only).", choice(&[("lean", "Lean"), ("full", "Full"), ("off", "Rotation only")], "lean"));

/// Presents between choosing an eye and presenting it: each producer knows its game's
/// (`default_number`); for testing.
pub const LATENCY: Opt = line("latency", "Eye timing (frames)", "Overrides the game's own.", Kind::Number { min: 0.0, max: 8.0, step: 1.0, default: 2.0 });

/// Used only when the headset reports no eye distance.
pub const EYE_SEPARATION: Opt = line("eye_separation", "Eye distance (m)", "Used when the headset reports none.", Kind::Number { min: 0.03, max: 0.09, step: 0.001, default: 0.064 });

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
        options: &[aim(&[AIM_MOTION, AIM_LOOK, AIM_POINT, AIM_MOUSE], "hand_rig"), AIM_HAND, render_size(""), HUD_SIZE.default_number(0.36), FINGER_TRACKING, LATENCY, EXTRA, LAUNCH, settle(60.0, 5.0)],
        licences: &[],
        left_hand_aims: true,
        weapon_ring: true,
    };

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
    }
}
