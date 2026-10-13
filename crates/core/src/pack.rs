//! The contract between Monaka and a game pack (a producer DLL and what comes with it), so a game's
//! VR support can be built, shipped and updated apart from Monaka.
//!
//! A producer DLL carries two exports the launcher reads from the file without loading it, as it
//! reads `MONAKA_HOST`:
//!
//! - `MONAKA_CONTRACT`: the [`CONTRACT`] it was built against ([`Contract::packed`]). The
//!   launcher runs a pack of its own major version and its own or an older minor one
//!   ([`Contract::runs`]).
//! - `MONAKA_MANIFEST`: its manifests ([`Manifest`]) as text, written at compile time from the
//!   crate's own `manifest.rs` ([`text_bytes`]), so the game list, options and tabs can come from
//!   the pack instead of being compiled into the launcher. [`parse`] reads it back.
//!
//! The text is one field per line, `key value`, the value escaped (`\\`, `\n`, `\t`, `\r`); pairs
//! inside a value are tab-separated; numbers that are options' bounds are an f64's bits in hex, so
//! they come back exactly. Before the games, `author` and `source` say who made the pack and where
//! it comes from (its crate's `authors` and `repository`; contract 1.1). A `game` line starts each
//! manifest and an `option` line each option; after the options, a `control` line starts each
//! how-to, its `way` lines (the text, then `id`, `value` pairs) and one `tunes` line (option ids).

use crate::manifest::{HowTo, Kind, Launch, Manifest, Opt, Shown, Status, SteamFiles, TurnsOff, Way};

/// A version of the contract: `major` changes when something a pack relies on changes shape (a
/// pack built before would read or write the wrong thing), `minor` when something is only added
/// (an older pack simply does not use it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Contract {
    pub major: u16,
    pub minor: u16,
}

/// The contract this Monaka speaks. It covers the loader's entry points (`MonakaStart`,
/// `MonakaStop`, `MonakaStatus`) and the producer's exports, the options file (`id=value` lines and
/// the shared vocabulary in [`crate::manifest`]), the run folder's files (`live-channel.txt`), the
/// channel layouts ([`crate::protocol`]) and this text. Raise `major` with any change a pack built
/// before would not survive, `minor` with anything only added.
///
/// - 1.1: the text's `author` and `source` ([`About`]).
/// - 1.2: packs for any game on an engine (`exe` [`crate::manifest::ANY_EXECUTABLE`], the game
///   picked in [`crate::manifest::GAME_OPTION`]), 32-bit twins of a pack's DLL for 32-bit games, and
///   Monaka's own settings a game's producer reads, listed among its options (on the `monaka` tab;
///   [`crate::manifest::DISABLE_VIEW_BOBBING`]).
/// - 1.3: the Controls tab: the `controls` tab word, a game's how-tos (`control`, `way`, `tunes`
///   lines; [`crate::manifest::HowTo`]), and conditions on any value but one (`!value`).
/// - 2.0: the `engine` tab word is gone: a 1.x pack that puts an option there cannot be read.
pub const CONTRACT: Contract = Contract { major: 2, minor: 0 };

/// Who made a pack and where it comes from, as the pack itself says (nothing checks it): its
/// crate's `authors` and `repository`. Empty where the crate names none, or the pack predates 1.1.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct About {
    pub author: String,
    pub source: String,
}

/// A pack's text read back ([`parse`]).
#[derive(Debug)]
pub struct Parsed {
    /// The contract the text names.
    pub contract: Contract,
    pub about: About,
    pub manifests: Vec<Manifest>,
}

impl Contract {
    /// As `MONAKA_CONTRACT` holds it: `major` in the high 16 bits.
    pub const fn packed(self) -> u32 {
        (self.major as u32) << 16 | self.minor as u32
    }

    pub const fn from_packed(packed: u32) -> Self {
        Self { major: (packed >> 16) as u16, minor: packed as u16 }
    }

    /// Whether a pack of `pack`'s contract needs a newer Monaka than a host of this one.
    pub fn needs_newer_host(self, pack: Contract) -> bool {
        pack.major > self.major || (pack.major == self.major && pack.minor > self.minor)
    }

    /// Whether a host of this contract runs a pack of `pack`'s, or what the player should do.
    pub fn runs(self, pack: Contract) -> Result<(), String> {
        if pack.major < self.major {
            Err(format!("it was made for an older Monaka (pack contract {pack}, this Monaka speaks {self}): get a version of the game pack made for this Monaka"))
        } else if self.needs_newer_host(pack) {
            Err(format!("it was made for a newer Monaka (pack contract {pack}, this Monaka speaks {self}): update Monaka"))
        } else {
            Ok(())
        }
    }
}

impl std::fmt::Display for Contract {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// The text's first word, before the contract version.
const MAGIC: &[u8] = b"monaka-manifest";

/// A writer that only counts while it has no room (the first pass sizes the second).
struct Out<'a> {
    buf: &'a mut [u8],
    at: usize,
}

impl Out<'_> {
    const fn byte(&mut self, b: u8) {
        if self.at < self.buf.len() {
            self.buf[self.at] = b;
        }
        self.at += 1;
    }

    const fn raw(&mut self, bytes: &[u8]) {
        let mut i = 0;
        while i < bytes.len() {
            self.byte(bytes[i]);
            i += 1;
        }
    }

    /// `text` escaped.
    const fn text(&mut self, text: &str) {
        let bytes = text.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'\\' => self.raw(b"\\\\"),
                b'\n' => self.raw(b"\\n"),
                b'\t' => self.raw(b"\\t"),
                b'\r' => self.raw(b"\\r"),
                b => self.byte(b),
            }
            i += 1;
        }
    }

    const fn decimal(&mut self, mut n: u64) {
        let mut digits = [0u8; 20];
        let mut count = 0;
        loop {
            digits[count] = b'0' + (n % 10) as u8;
            count += 1;
            n /= 10;
            if n == 0 {
                break;
            }
        }
        while count > 0 {
            count -= 1;
            self.byte(digits[count]);
        }
    }

    /// An f64, exactly: its bits as 16 hex digits.
    const fn float(&mut self, value: f64) {
        let bits = value.to_bits();
        let mut shift = 64;
        while shift > 0 {
            shift -= 4;
            let nibble = ((bits >> shift) & 0xf) as u8;
            self.byte(if nibble < 10 { b'0' + nibble } else { b'a' + nibble - 10 });
        }
    }

    const fn key(&mut self, key: &[u8]) {
        self.raw(key);
        self.byte(b' ');
    }

    const fn field(&mut self, key: &[u8], value: &str) {
        self.key(key);
        self.text(value);
        self.byte(b'\n');
    }

    const fn flag(&mut self, key: &[u8], on: bool) {
        self.key(key);
        self.byte(if on { b'1' } else { b'0' });
        self.byte(b'\n');
    }
}

const fn status_word(status: Status) -> &'static str {
    match status {
        Status::Playable => "playable",
        Status::Beta => "beta",
        Status::Alpha => "alpha",
        Status::PreAlpha => "pre_alpha",
        Status::InProgress => "in_progress",
        Status::Experimental => "experimental",
    }
}

const fn shown_word(shown: Shown) -> &'static str {
    match shown {
        Shown::Main => "main",
        Shown::Controls => "controls",
        Shown::Hud => "hud",
        Shown::Skeleton => "skeleton",
        Shown::Rendering => "rendering",
        Shown::Game => "game",
        Shown::Monaka => "monaka",
        Shown::Line => "line",
    }
}

const fn write_option(out: &mut Out, opt: &Opt) {
    out.field(b"option", opt.id);
    out.field(b"label", opt.label);
    out.field(b"help", opt.help);
    out.field(b"shown", shown_word(opt.shown));
    out.flag(b"to_producer", opt.producer);
    if let Some((id, value)) = opt.when {
        out.key(b"when");
        out.text(id);
        out.byte(b'\t');
        out.text(value);
        out.byte(b'\n');
    }
    if let Some(rule) = opt.turns_off {
        out.key(b"turns_off");
        out.text(rule.value);
        out.byte(b'\t');
        out.text(rule.target);
        out.byte(b'\t');
        out.text(rule.reason);
        out.byte(b'\n');
    }
    if opt.needs_eye_size {
        out.raw(b"needs_eye_size 1\n");
    }
    if let Some(files) = opt.needs_files {
        out.key(b"needs_files");
        out.text(files.game_folder);
        out.byte(b'\t');
        out.text(files.path);
        out.byte(b'\t');
        out.text(files.file);
        out.byte(b'\t');
        out.text(files.line);
        out.byte(b'\t');
        out.text(files.missing);
        out.byte(b'\n');
    }
    if let Some((value, args)) = opt.viewer {
        out.key(b"viewer");
        out.text(value);
        let mut i = 0;
        while i < args.len() {
            out.byte(b'\t');
            out.text(args[i]);
            i += 1;
        }
        out.byte(b'\n');
    }
    if let Some(note) = opt.eye_size_note {
        out.field(b"eye_size_note", note);
    }
    match opt.kind {
        Kind::Switch { default } => {
            out.key(b"kind");
            out.raw(b"switch ");
            out.byte(if default { b'1' } else { b'0' });
            out.byte(b'\n');
        }
        Kind::Choice { choices, default, window } => {
            out.key(b"kind");
            out.raw(b"choice ");
            out.text(default);
            out.byte(b'\t');
            out.decimal(window as u64);
            out.byte(b'\n');
            let mut i = 0;
            while i < choices.len() {
                out.key(b"choice");
                out.text(choices[i].0);
                out.byte(b'\t');
                out.text(choices[i].1);
                out.byte(b'\n');
                i += 1;
            }
        }
        Kind::Number { min, max, step, default } => {
            out.key(b"kind");
            out.raw(b"number ");
            out.float(min);
            out.byte(b'\t');
            out.float(max);
            out.byte(b'\t');
            out.float(step);
            out.byte(b'\t');
            out.float(default);
            out.byte(b'\n');
        }
        Kind::Text => out.raw(b"kind text\n"),
        Kind::Size => out.raw(b"kind size\n"),
    }
}

const fn write_game(out: &mut Out, m: &Manifest) {
    out.field(b"game", m.id);
    out.field(b"name", m.name);
    out.field(b"status", status_word(m.status));
    let mut i = 0;
    while i < m.executables.len() {
        out.field(b"exe", m.executables[i]);
        i += 1;
    }
    out.key(b"steam_app");
    out.decimal(m.steam_app as u64);
    out.byte(b'\n');
    out.field(b"producer", m.producer);
    out.field(b"log", m.log);
    out.field(b"options_file", m.options_file);
    out.field(b"run_prefix", m.run_prefix);
    match m.launch {
        Launch::Auto { settle_seconds } => {
            out.raw(b"launch auto ");
            out.decimal(settle_seconds);
            out.byte(b'\n');
        }
        Launch::Manual => out.raw(b"launch manual\n"),
    }
    i = 0;
    while i < m.refused_arguments.len() {
        out.key(b"refuse");
        out.text(m.refused_arguments[i].0);
        out.byte(b'\t');
        out.text(m.refused_arguments[i].1);
        out.byte(b'\n');
        i += 1;
    }
    out.field(b"before", m.before);
    i = 0;
    while i < m.licences.len() {
        out.field(b"licence", m.licences[i]);
        i += 1;
    }
    out.flag(b"left_hand_aims", m.left_hand_aims);
    out.flag(b"weapon_ring", m.weapon_ring);
    i = 0;
    while i < m.options.len() {
        write_option(out, &m.options[i]);
        i += 1;
    }
    i = 0;
    while i < m.controls.len() {
        write_how(out, &m.controls[i]);
        i += 1;
    }
    out.raw(b"end\n");
}

const fn write_how(out: &mut Out, how: &HowTo) {
    out.field(b"control", how.action);
    let mut i = 0;
    while i < how.ways.len() {
        let way = &how.ways[i];
        out.key(b"way");
        out.text(way.text);
        let mut j = 0;
        while j < way.when.len() {
            out.byte(b'\t');
            out.text(way.when[j].0);
            out.byte(b'\t');
            out.text(way.when[j].1);
            j += 1;
        }
        out.byte(b'\n');
        i += 1;
    }
    out.key(b"tunes");
    i = 0;
    while i < how.options.len() {
        if i > 0 {
            out.byte(b'\t');
        }
        out.text(how.options[i]);
        i += 1;
    }
    out.byte(b'\n');
}

const fn write(out: &mut Out, manifests: &[Manifest], author: &str, source: &str) {
    out.raw(MAGIC);
    out.byte(b' ');
    out.decimal(CONTRACT.major as u64);
    out.byte(b'.');
    out.decimal(CONTRACT.minor as u64);
    out.byte(b'\n');
    out.field(b"author", author);
    out.field(b"source", source);
    let mut i = 0;
    while i < manifests.len() {
        write_game(out, &manifests[i]);
        i += 1;
    }
}

/// The length of `manifests`' text with its NUL (the size of `MONAKA_MANIFEST`), for a pack by
/// `author` from `source`.
pub const fn text_len(manifests: &[Manifest], author: &str, source: &str) -> usize {
    let mut nothing = [0u8; 0];
    let mut out = Out { buf: &mut nothing, at: 0 };
    write(&mut out, manifests, author, source);
    out.at + 1
}

/// `manifests`' text, NUL-terminated (`N` is [`text_len`]).
pub const fn text_bytes<const N: usize>(manifests: &[Manifest], author: &str, source: &str) -> [u8; N] {
    let mut bytes = [0u8; N];
    let mut out = Out { buf: &mut bytes, at: 0 };
    write(&mut out, manifests, author, source);
    bytes
}

/// `manifests`' text.
pub fn text(manifests: &[Manifest], author: &str, source: &str) -> String {
    let mut bytes = vec![0u8; text_len(manifests, author, source)];
    let mut out = Out { buf: &mut bytes, at: 0 };
    write(&mut out, manifests, author, source);
    bytes.pop();
    String::from_utf8(bytes).expect("written from UTF-8 strings")
}

// ---- Reading it back

fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

fn leak(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}

fn leak_slice<T>(items: Vec<T>) -> &'static [T] {
    Box::leak(items.into_boxed_slice())
}

/// A value's tab-separated parts, each unescaped (a tab in a part is escaped, so a raw tab
/// always separates).
fn parts(value: &str) -> Vec<String> {
    value.split('\t').map(unescape).collect()
}

fn flag(value: &str) -> Result<bool, String> {
    match value {
        "1" => Ok(true),
        "0" => Ok(false),
        _ => Err(format!("not 0 or 1: {value}")),
    }
}

fn float(value: &str) -> Result<f64, String> {
    u64::from_str_radix(value, 16).map(f64::from_bits).map_err(|_| format!("not a number's bits: {value}"))
}

#[derive(Default)]
struct OptDraft {
    id: String,
    label: String,
    help: String,
    shown: Option<Shown>,
    producer: Option<bool>,
    when: Option<(String, String)>,
    turns_off: Option<(String, String, String)>,
    needs_eye_size: bool,
    needs_files: Option<[String; 5]>,
    viewer: Option<(String, Vec<String>)>,
    eye_size_note: Option<String>,
    kind: Option<Kind>,
    choice: Option<(String, usize)>,
    choices: Vec<(&'static str, &'static str)>,
}

impl OptDraft {
    fn finish(self) -> Result<Opt, String> {
        let id = self.id;
        let missing = |what: &str| format!("option {id}: no {what}");
        let kind = match (self.kind, self.choice) {
            (Some(kind), None) => kind,
            (None, Some((default, window))) => {
                if window > self.choices.len() {
                    return Err(format!("option {id}: offers {window} of {} choices", self.choices.len()));
                }
                Kind::Choice { choices: leak_slice(self.choices), default: leak(default), window }
            }
            _ => return Err(missing("kind")),
        };
        Ok(Opt {
            id: leak(id.clone()),
            label: leak(self.label),
            help: leak(self.help),
            kind,
            shown: self.shown.ok_or_else(|| missing("tab"))?,
            when: self.when.map(|(a, b)| (leak(a), leak(b))),
            producer: self.producer.ok_or_else(|| missing("to_producer"))?,
            turns_off: self.turns_off.map(|(value, target, reason)| TurnsOff { value: leak(value), target: leak(target), reason: leak(reason) }),
            needs_eye_size: self.needs_eye_size,
            needs_files: self.needs_files.map(|[game_folder, path, file, line, missing]| SteamFiles {
                game_folder: leak(game_folder),
                path: leak(path),
                file: leak(file),
                line: leak(line),
                missing: leak(missing),
            }),
            viewer: self.viewer.map(|(value, args)| (leak(value), leak_slice(args.into_iter().map(leak).collect()))),
            eye_size_note: self.eye_size_note.map(leak),
            owner: crate::manifest::shared_owner(&id),
        })
    }
}

#[derive(Default)]
struct GameDraft {
    id: String,
    name: Option<String>,
    status: Option<Status>,
    executables: Vec<&'static str>,
    steam_app: Option<u32>,
    producer: Option<String>,
    log: Option<String>,
    options_file: Option<String>,
    run_prefix: Option<String>,
    launch: Option<Launch>,
    refused_arguments: Vec<(&'static str, &'static str)>,
    before: Option<String>,
    options: Vec<Opt>,
    licences: Vec<&'static str>,
    left_hand_aims: Option<bool>,
    weapon_ring: Option<bool>,
    /// The how-tos: (action, ways, option ids).
    controls: Vec<(&'static str, Vec<Way>, &'static [&'static str])>,
}

impl GameDraft {
    fn finish(self) -> Result<Manifest, String> {
        let id = self.id;
        let missing = |what: &str| format!("game {id}: no {what}");
        Ok(Manifest {
            name: leak(self.name.ok_or_else(|| missing("name"))?),
            status: self.status.ok_or_else(|| missing("status"))?,
            executables: leak_slice(self.executables),
            steam_app: self.steam_app.ok_or_else(|| missing("steam_app"))?,
            producer: leak(self.producer.ok_or_else(|| missing("producer"))?),
            log: leak(self.log.ok_or_else(|| missing("log"))?),
            options_file: leak(self.options_file.ok_or_else(|| missing("options_file"))?),
            run_prefix: leak(self.run_prefix.ok_or_else(|| missing("run_prefix"))?),
            launch: self.launch.ok_or_else(|| missing("launch"))?,
            refused_arguments: leak_slice(self.refused_arguments),
            before: leak(self.before.ok_or_else(|| missing("before"))?),
            options: leak_slice(self.options),
            licences: leak_slice(self.licences),
            left_hand_aims: self.left_hand_aims.ok_or_else(|| missing("left_hand_aims"))?,
            weapon_ring: self.weapon_ring.ok_or_else(|| missing("weapon_ring"))?,
            controls: leak_slice(self.controls.into_iter().map(|(action, ways, options)| HowTo { action, ways: leak_slice(ways), options }).collect()),
            id: leak(id),
        })
    }
}

/// The manifests in a pack's text ([`text`]), with the contract it names and who made it. The
/// strings live for the rest of the process (a launcher reads each pack once).
pub fn parse(text: &str) -> Result<Parsed, String> {
    let mut lines = text.lines();
    let header = lines.next().ok_or("empty manifest text")?;
    let contract = header
        .strip_prefix("monaka-manifest ")
        .and_then(|v| v.trim().split_once('.'))
        .and_then(|(major, minor)| Some(Contract { major: major.parse().ok()?, minor: minor.parse().ok()? }))
        .ok_or_else(|| format!("not a Monaka manifest (it starts {header:?})"))?;
    let mut about = About::default();
    let mut games = Vec::new();
    let mut game: Option<GameDraft> = None;
    let mut opt: Option<OptDraft> = None;
    for (number, line) in lines.enumerate() {
        let at = |e: String| format!("line {}: {e}", number + 2);
        if line.is_empty() {
            continue;
        }
        let (key, value) = line.split_once(' ').unwrap_or((line, ""));
        if key == "game" {
            if game.is_some() {
                return Err(at("a game inside a game (no end)".into()));
            }
            game = Some(GameDraft { id: unescape(value), ..GameDraft::default() });
            continue;
        }
        if game.is_none() && games.is_empty() {
            match key {
                "author" => about.author = unescape(value),
                "source" => about.source = unescape(value),
                _ => return Err(at(format!("{key} outside a game"))),
            }
            continue;
        }
        let g = game.as_mut().ok_or_else(|| at(format!("{key} outside a game")))?;
        // An option's own fields while one is open; anything else closes it.
        if let Some(o) = opt.as_mut() {
            let mut taken = true;
            match key {
                "label" => o.label = unescape(value),
                "help" => o.help = unescape(value),
                "shown" => o.shown = Some(shown_of(value).map_err(at)?),
                "to_producer" => o.producer = Some(flag(value).map_err(at)?),
                "when" => {
                    let p = parts(value);
                    let [a, b] = <[String; 2]>::try_from(p).map_err(|_| at("when takes an id and a value".into()))?;
                    o.when = Some((a, b));
                }
                "turns_off" => {
                    let p = parts(value);
                    let [a, b, c] = <[String; 3]>::try_from(p).map_err(|_| at("turns_off takes a value, a target and a reason".into()))?;
                    o.turns_off = Some((a, b, c));
                }
                "needs_eye_size" => o.needs_eye_size = flag(value).map_err(at)?,
                "needs_files" => {
                    let p = parts(value);
                    o.needs_files = Some(<[String; 5]>::try_from(p).map_err(|_| at("needs_files takes a game folder, a path, a file, a line and a reason".into()))?);
                }
                "viewer" => {
                    let mut p = parts(value).into_iter();
                    let first = p.next().ok_or_else(|| at("viewer takes a value and arguments".into()))?;
                    o.viewer = Some((first, p.collect()));
                }
                "eye_size_note" => o.eye_size_note = Some(unescape(value)),
                "kind" => {
                    let (word, rest) = value.split_once(' ').unwrap_or((value, ""));
                    match word {
                        "switch" => o.kind = Some(Kind::Switch { default: flag(rest).map_err(at)? }),
                        "text" => o.kind = Some(Kind::Text),
                        "size" => o.kind = Some(Kind::Size),
                        "number" => {
                            let p: Vec<&str> = rest.split('\t').collect();
                            let [min, max, step, default] = <[&str; 4]>::try_from(p).map_err(|_| at("a number takes min, max, step, default".into()))?;
                            o.kind = Some(Kind::Number { min: float(min).map_err(at)?, max: float(max).map_err(at)?, step: float(step).map_err(at)?, default: float(default).map_err(at)? });
                        }
                        "choice" => {
                            let p = parts(rest);
                            let [default, window] = <[String; 2]>::try_from(p).map_err(|_| at("a choice takes a default and a window".into()))?;
                            o.choice = Some((default, window.parse().map_err(|_| at(format!("not a count: {window}")))?));
                        }
                        other => return Err(at(format!("unknown kind {other}"))),
                    }
                }
                "choice" => {
                    let p = parts(value);
                    let [id, label] = <[String; 2]>::try_from(p).map_err(|_| at("a choice takes an id and a label".into()))?;
                    o.choices.push((leak(id), leak(label)));
                }
                _ => taken = false,
            }
            if taken {
                continue;
            }
            g.options.push(opt.take().expect("open").finish().map_err(at)?);
        }
        match key {
            "option" => opt = Some(OptDraft { id: unescape(value), ..OptDraft::default() }),
            "name" => g.name = Some(unescape(value)),
            "status" => g.status = Some(status_of(value).map_err(at)?),
            // Older packs list known bugs; the launcher does not show them.
            "bug" => {}
            "exe" => g.executables.push(leak(unescape(value))),
            "steam_app" => g.steam_app = Some(value.parse().map_err(|_| at(format!("not an app id: {value}")))?),
            "producer" => g.producer = Some(unescape(value)),
            "log" => g.log = Some(unescape(value)),
            "options_file" => g.options_file = Some(unescape(value)),
            "run_prefix" => g.run_prefix = Some(unescape(value)),
            "launch" => {
                g.launch = Some(match value.split_once(' ') {
                    None if value == "manual" => Launch::Manual,
                    Some(("auto", seconds)) => Launch::Auto { settle_seconds: seconds.parse().map_err(|_| at(format!("not seconds: {seconds}")))? },
                    _ => return Err(at(format!("unknown launch {value}"))),
                })
            }
            "refuse" => {
                let p = parts(value);
                let [text, why] = <[String; 2]>::try_from(p).map_err(|_| at("refuse takes an argument and why".into()))?;
                g.refused_arguments.push((leak(text), leak(why)));
            }
            "before" => g.before = Some(unescape(value)),
            "licence" => g.licences.push(leak(unescape(value))),
            "left_hand_aims" => g.left_hand_aims = Some(flag(value).map_err(at)?),
            "weapon_ring" => g.weapon_ring = Some(flag(value).map_err(at)?),
            "control" => g.controls.push((leak(unescape(value)), Vec::new(), &[])),
            "way" => {
                let how = g.controls.last_mut().ok_or_else(|| at("a way outside a control".into()))?;
                let mut p = parts(value).into_iter();
                let text = leak(p.next().unwrap_or_default());
                let rest: Vec<String> = p.collect();
                if !rest.len().is_multiple_of(2) {
                    return Err(at("a way's conditions are id and value pairs".into()));
                }
                let when = leak_slice(rest.chunks(2).map(|pair| (leak(pair[0].clone()), leak(pair[1].clone()))).collect());
                how.1.push(Way { text, when });
            }
            "tunes" => {
                let how = g.controls.last_mut().ok_or_else(|| at("tunes outside a control".into()))?;
                how.2 = leak_slice(if value.is_empty() { Vec::new() } else { parts(value).into_iter().map(leak).collect() });
            }
            "end" => games.push(game.take().expect("open").finish().map_err(at)?),
            other => return Err(at(format!("unknown field {other}"))),
        }
    }
    if game.is_some() {
        return Err("the last game has no end".into());
    }
    Ok(Parsed { contract, about, manifests: games })
}

fn status_of(word: &str) -> Result<Status, String> {
    Ok(match word {
        "playable" => Status::Playable,
        "beta" => Status::Beta,
        "alpha" => Status::Alpha,
        "pre_alpha" => Status::PreAlpha,
        "in_progress" => Status::InProgress,
        "experimental" => Status::Experimental,
        _ => return Err(format!("unknown status {word}")),
    })
}

fn shown_of(word: &str) -> Result<Shown, String> {
    Ok(match word {
        "main" => Shown::Main,
        "controls" => Shown::Controls,
        "hud" => Shown::Hud,
        "skeleton" => Shown::Skeleton,
        "rendering" => Shown::Rendering,
        "game" => Shown::Game,
        "monaka" => Shown::Monaka,
        "line" => Shown::Line,
        _ => return Err(format!("unknown tab {word}")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::*;

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

    #[test]
    fn the_text_reads_back_exactly() {
        let text = text(&[GAME, MANUAL], "Someone <a@b.c>", "https://example.org/x\ty");
        let parsed = parse(&text).unwrap();
        assert_eq!(parsed.contract, CONTRACT);
        assert_eq!(parsed.about, About { author: "Someone <a@b.c>".into(), source: "https://example.org/x\ty".into() });
        let games = parsed.manifests;
        assert_eq!(games.len(), 2);
        assert!(games[0] == GAME && games[1] == MANUAL, "{text}");
        // The compile-time bytes are the same text, NUL-terminated.
        const BYTES: [u8; text_len(&[GAME, MANUAL], "Someone <a@b.c>", "https://example.org/x\ty")] = text_bytes(&[GAME, MANUAL], "Someone <a@b.c>", "https://example.org/x\ty");
        assert_eq!(&BYTES[..BYTES.len() - 1], text.as_bytes());
        assert_eq!(BYTES[BYTES.len() - 1], 0);
    }

    #[test]
    fn a_monaka_setting_a_game_reads_survives_the_text() {
        // A game listing one of Monaka's own settings its producer reads passes the check after the
        // round trip: what it lists still is Monaka's, unchanged.
        const READS: &[Opt] = &[crate::manifest::DISABLE_VIEW_BOBBING];
        const READER: Manifest = Manifest { options: READS, launch: Launch::Manual, controls: &[], ..GAME };
        let parsed = parse(&text(&[READER], "", "")).unwrap();
        assert!(parsed.manifests[0] == READER);
        assert!(parsed.manifests[0].check().is_ok(), "{:?}", parsed.manifests[0].check());
    }

    #[test]
    fn known_bugs_from_an_older_pack_are_read_past() {
        let old = text(&[MANUAL], "", "").replace("status in_progress\n", "status in_progress\nbug One.\nbug Two.\n");
        assert!(parse(&old).unwrap().manifests[0] == MANUAL);
    }

    #[test]
    fn a_pack_from_before_1_1_names_nobody() {
        // 1.0 wrote no author or source.
        let old = text(&[MANUAL], "", "").replace("author \nsource \n", "").replace(&format!("monaka-manifest {CONTRACT}"), "monaka-manifest 1.0");
        let parsed = parse(&old).unwrap();
        assert_eq!(parsed.contract, Contract { major: 1, minor: 0 });
        assert_eq!(parsed.about, About::default());
        assert!(parsed.manifests[0] == MANUAL);
        assert!(parse(&text(&[MANUAL], "", "").replace("source \n", "source \nbogus 1\n")).unwrap_err().contains("bogus outside a game"));
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
        let sideways = text(&[GAME], "", "").replace("status in_progress", "status sideways");
        assert!(parse(&sideways).unwrap_err().contains("unknown status"));
        let unended = text(&[MANUAL], "", "").replace("end\n", "");
        assert!(parse(&unended).unwrap_err().contains("no end"));
    }
}
