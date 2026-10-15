//! The contract between Monaka and a game pack (a producer DLL and what comes with it), so a game's
//! VR support can be built, shipped and updated apart from Monaka.
//!
//! A producer DLL carries two exports the launcher reads from the file without loading it, as it
//! reads `MONAKA_HOST`:
//!
//! - `MONAKA_CONTRACT`: the [`CONTRACT`] it was built against ([`Contract::packed`]). The
//!   launcher runs a pack of its own major version only ([`Contract::runs`]).
//! - `MONAKA_MANIFEST`: its manifests ([`Manifest`]) as text, written at compile time from the
//!   crate's own `manifest.rs` ([`text_bytes`]), so the game list, options and tabs can come from
//!   the pack instead of being compiled into the launcher. [`parse`] reads it back.
//!
//! The text is one field per line, `key value`, the value escaped (`\\`, `\n`, `\t`, `\r`); pairs
//! inside a value are tab-separated; numbers that are options' bounds are an f64's bits in hex, so
//! they come back exactly. Before the games, `author` and `source` say who made the pack and where
//! it comes from (its crate's `authors` and `repository`), `version` and `monaka` its crate's
//! version and the Monaka it was built with, and `requires` and `uses` the host features it names
//! ([`crate::host`]). A `game` line starts each manifest and an `option` line each option; after
//! the options, a `control` line starts each how-to, its `way` lines (the text, then `id`, `value`
//! pairs) and one `tunes` line (option ids).
//!
//! # Changing it
//!
//! Within a major version the text only grows, and a reader skips what it does not know: a field
//! or line it has no word for is passed over, an unknown tab puts the option on the Game tab, an
//! unknown kind shows it as text (its value still reaches the producer), an unknown status reads as
//! Experimental and an unknown launch as manual. So an older Monaka reads a newer pack and simply
//! does without what it cannot show. A pack that cannot work without something new in the host
//! says so in `requires` ([`crate::host`]); that, not a version number, is when a pack needs a
//! newer Monaka.
//!
//! Anything an older reader would get wrong rather than skip (a field read another way, a value's
//! parts in another order, a record's layout changed in place) is a new major version, and every
//! pack is rebuilt for it. So is a field that hides or holds something back, such as an option's
//! `dev`: an older reader would skip it and show the player what it should hide.

use crate::host;
use crate::manifest::{HowTo, Kind, Launch, Manifest, Opt, Shown, Status, SteamFiles, TurnsOff, Way};

/// A version of the contract. Only `major` exists: it changes when something a pack relies on
/// changes shape (a pack built before would read or write the wrong thing). Additions need no new
/// version (see the module's "Changing it").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Contract {
    pub major: u16,
}

/// The contract this Monaka speaks. It covers the loader's entry points (`MonakaStart`,
/// `MonakaStop`, `MonakaStatus`) and the producer's exports, the options file (`id=value` lines and
/// the shared vocabulary in [`crate::manifest`]), the run folder's files ([`crate::protocol`]'s
/// `live-channel.txt`, [`host::FEATURES_FILE`]), the channel's header and blocks
/// ([`crate::protocol`]) and this text.
///
/// - 1 to 3: minor versions for each addition, so every added field needed a new Monaka.
/// - 4: additions are skipped by an older reader and named as host features ([`crate::host`]);
///   every shared-memory record starts with the same 16-byte stamp and only grows within its
///   major ([`crate::protocol::Stamp`]); each eye's depth is its own block; options reach the
///   viewer as `id=value` lines.
pub const CONTRACT: Contract = Contract { major: 4 };

/// This Monaka's version, written into every pack built with it ([`About::monaka`]).
pub const MONAKA_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Who made a pack and where it comes from, as the pack itself says (nothing checks it): its
/// crate's `authors` and `repository`. Empty where the crate names none.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct About {
    pub author: String,
    pub source: String,
    /// Its crate's version.
    pub version: String,
    /// The Monaka it was built with ([`MONAKA_VERSION`]).
    pub monaka: String,
}

/// The host features a pack names ([`crate::host`]): those it cannot work without, and those it
/// does more with. A pack's `uses` also include what its manifests need (the Controls tab, a game
/// joined as it starts), added when the text is written.
#[derive(Clone, Copy, Debug)]
pub struct Needs<'a> {
    pub requires: &'a [&'a str],
    /// Each feature with whether this pack uses it (so a pack's set can be worked out at compile
    /// time from what its code provides).
    pub uses: &'a [(&'a str, bool)],
}

impl Needs<'_> {
    /// No host features beyond what the contract's major version brings.
    pub const NONE: Needs<'static> = Needs { requires: &[], uses: &[] };
}

/// A pack's text read back ([`parse`]).
#[derive(Debug)]
pub struct Parsed {
    /// The contract the text names.
    pub contract: Contract,
    pub about: About,
    /// The host features the pack cannot work without.
    pub requires: Vec<String>,
    /// The host features the pack does more with.
    pub uses: Vec<String>,
    pub manifests: Vec<Manifest>,
}

impl Parsed {
    /// The features the pack requires that this host lacks: it cannot run here.
    pub fn missing_required(&self) -> Vec<&str> {
        host::missing(&self.requires.iter().map(String::as_str).collect::<Vec<_>>())
    }

    /// The features the pack uses that this host lacks: it runs, without them.
    pub fn missing_used(&self) -> Vec<&str> {
        host::missing(&self.uses.iter().map(String::as_str).collect::<Vec<_>>())
    }
}

impl Contract {
    /// As `MONAKA_CONTRACT` holds it: `major` in the high 16 bits (the low 16 held a minor version
    /// before 4, and are zero since).
    pub const fn packed(self) -> u32 {
        (self.major as u32) << 16
    }

    pub const fn from_packed(packed: u32) -> Self {
        Self { major: (packed >> 16) as u16 }
    }

    /// Whether a pack of `pack`'s contract needs a newer Monaka than a host of this one.
    pub fn needs_newer_host(self, pack: Contract) -> bool {
        pack.major > self.major
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
        write!(f, "{}", self.major)
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
    if opt.to_viewer {
        out.raw(b"to_viewer 1\n");
    }
    if opt.dev {
        out.raw(b"dev 1\n");
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
        Launch::AtStart => out.raw(b"launch at_start\n"),
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

/// Whether any of `manifests` puts something on the Controls tab.
const fn uses_controls(manifests: &[Manifest]) -> bool {
    let mut i = 0;
    while i < manifests.len() {
        if !manifests[i].controls.is_empty() {
            return true;
        }
        let mut o = 0;
        while o < manifests[i].options.len() {
            if matches!(manifests[i].options[o].shown, Shown::Controls) {
                return true;
            }
            o += 1;
        }
        i += 1;
    }
    false
}

/// Whether any of `manifests` is joined as its game starts.
const fn joins_at_start(manifests: &[Manifest]) -> bool {
    let mut i = 0;
    while i < manifests.len() {
        if matches!(manifests[i].launch, Launch::AtStart) {
            return true;
        }
        i += 1;
    }
    false
}

const fn named(name: &str, names: &[&str]) -> bool {
    let mut i = 0;
    while i < names.len() {
        let (a, b) = (name.as_bytes(), names[i].as_bytes());
        if a.len() == b.len() {
            let mut j = 0;
            while j < a.len() && a[j] == b[j] {
                j += 1;
            }
            if j == a.len() {
                return true;
            }
        }
        i += 1;
    }
    false
}

/// The `uses` names of `list` that are on and not required, after a tab unless `first`; whether
/// the line is still empty.
const fn write_used(out: &mut Out, mut first: bool, list: &[(&str, bool)], requires: &[&str]) -> bool {
    let mut i = 0;
    while i < list.len() {
        let (name, on) = list[i];
        if on && !named(name, requires) {
            if first {
                out.key(b"uses");
                first = false;
            } else {
                out.byte(b'\t');
            }
            out.text(name);
        }
        i += 1;
    }
    first
}

const fn write_needs(out: &mut Out, manifests: &[Manifest], needs: &Needs) {
    if !needs.requires.is_empty() {
        out.key(b"requires");
        let mut i = 0;
        while i < needs.requires.len() {
            if i > 0 {
                out.byte(b'\t');
            }
            out.text(needs.requires[i]);
            i += 1;
        }
        out.byte(b'\n');
    }
    let derived = [(host::CONTROLS_TAB, uses_controls(manifests)), (host::LAUNCH_AT_START, joins_at_start(manifests))];
    let first = write_used(out, true, needs.uses, needs.requires);
    if !write_used(out, first, &derived, needs.requires) {
        out.byte(b'\n');
    }
}

const fn write(out: &mut Out, manifests: &[Manifest], author: &str, source: &str, version: &str, needs: &Needs) {
    out.raw(MAGIC);
    out.byte(b' ');
    out.decimal(CONTRACT.major as u64);
    out.byte(b'\n');
    out.field(b"author", author);
    out.field(b"source", source);
    out.field(b"version", version);
    out.field(b"monaka", MONAKA_VERSION);
    write_needs(out, manifests, needs);
    let mut i = 0;
    while i < manifests.len() {
        write_game(out, &manifests[i]);
        i += 1;
    }
}

/// The length of `manifests`' text with its NUL (the size of `MONAKA_MANIFEST`), for a pack by
/// `author` from `source` at `version` naming the host features `needs`.
pub const fn text_len(manifests: &[Manifest], author: &str, source: &str, version: &str, needs: &Needs) -> usize {
    let mut nothing = [0u8; 0];
    let mut out = Out { buf: &mut nothing, at: 0 };
    write(&mut out, manifests, author, source, version, needs);
    out.at + 1
}

/// `manifests`' text, NUL-terminated (`N` is [`text_len`]).
pub const fn text_bytes<const N: usize>(manifests: &[Manifest], author: &str, source: &str, version: &str, needs: &Needs) -> [u8; N] {
    let mut bytes = [0u8; N];
    let mut out = Out { buf: &mut bytes, at: 0 };
    write(&mut out, manifests, author, source, version, needs);
    bytes
}

/// `manifests`' text.
pub fn text(manifests: &[Manifest], author: &str, source: &str, version: &str, needs: &Needs) -> String {
    let mut bytes = vec![0u8; text_len(manifests, author, source, version, needs)];
    let mut out = Out { buf: &mut bytes, at: 0 };
    write(&mut out, manifests, author, source, version, needs);
    bytes.pop();
    String::from_utf8(bytes).expect("written from UTF-8 strings")
}

// ---- Reading it back

/// The words of a game's own fields and lines: one of these closes an open option; any other
/// word while an option is open is the option's (one this reader may not know).
const GAME_FIELDS: &[&str] = &[
    "option", "name", "status", "exe", "steam_app", "producer", "log", "options_file", "run_prefix", "launch", "refuse", "before", "licence",
    "left_hand_aims", "weapon_ring", "control", "way", "tunes", "end",
];

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
    to_viewer: bool,
    dev: bool,
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
            to_viewer: self.to_viewer,
            dev: self.dev,
            needs_files: self.needs_files.map(|[game_folder, path, file, line, missing]| SteamFiles {
                game_folder: leak(game_folder),
                path: leak(path),
                file: leak(file),
                line: leak(line),
                missing: leak(missing),
            }),
            viewer: self.viewer.map(|(value, args)| (leak(value), leak_slice(args.into_iter().map(leak).collect()))),
            eye_size_note: self.eye_size_note.map(leak),
            shared: crate::manifest::is_shared(&id),
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

/// The manifests in a pack's text ([`text`]), with the contract it names, who made it and the
/// host features it names. What it does not know it skips (the module's "Changing it"); what it
/// knows but cannot read is an error. The strings live for the rest of the process (a launcher
/// reads each pack once).
pub fn parse(text: &str) -> Result<Parsed, String> {
    let mut lines = text.lines();
    let header = lines.next().ok_or("empty manifest text")?;
    // `4`; before 4, `major.minor`.
    let contract = header
        .strip_prefix("monaka-manifest ")
        .and_then(|v| v.trim().split('.').next()?.parse().ok())
        .map(|major| Contract { major })
        .ok_or_else(|| format!("not a Monaka manifest (it starts {header:?})"))?;
    let (mut requires, mut uses) = (Vec::new(), Vec::new());
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
                "version" => about.version = unescape(value),
                "monaka" => about.monaka = unescape(value),
                "requires" => requires = parts(value),
                "uses" => uses = parts(value),
                // Something a newer pack says about itself.
                _ => {}
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
                "shown" => o.shown = Some(shown_of(value)),
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
                "to_viewer" => o.to_viewer = flag(value).map_err(at)?,
                "dev" => o.dev = flag(value).map_err(at)?,
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
                        // A kind a newer pack has: the value still reaches the producer.
                        _ => o.kind = Some(Kind::Text),
                    }
                }
                "choice" => {
                    let p = parts(value);
                    let [id, label] = <[String; 2]>::try_from(p).map_err(|_| at("a choice takes an id and a label".into()))?;
                    o.choices.push((leak(id), leak(label)));
                }
                _ => taken = !GAME_FIELDS.contains(&key),
            }
            if taken {
                continue;
            }
            g.options.push(opt.take().expect("open").finish().map_err(at)?);
        }
        match key {
            "option" => opt = Some(OptDraft { id: unescape(value), ..OptDraft::default() }),
            "name" => g.name = Some(unescape(value)),
            "status" => g.status = Some(status_of(value)),
            "exe" => g.executables.push(leak(unescape(value))),
            "steam_app" => g.steam_app = Some(value.parse().map_err(|_| at(format!("not an app id: {value}")))?),
            "producer" => g.producer = Some(unescape(value)),
            "log" => g.log = Some(unescape(value)),
            "options_file" => g.options_file = Some(unescape(value)),
            "run_prefix" => g.run_prefix = Some(unescape(value)),
            "launch" => {
                g.launch = Some(match value.split_once(' ') {
                    None if value == "manual" => Launch::Manual,
                    None if value == "at_start" => Launch::AtStart,
                    Some(("auto", seconds)) => Launch::Auto { settle_seconds: seconds.parse().map_err(|_| at(format!("not seconds: {seconds}")))? },
                    // A way a newer pack has: the player starts the game.
                    _ => Launch::Manual,
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
            // A field a newer pack has.
            _ => {}
        }
    }
    if game.is_some() {
        return Err("the last game has no end".into());
    }
    let mut seen = Vec::new();
    uses.retain(|name| !requires.contains(name) && !seen.contains(name) && {
        seen.push(name.clone());
        true
    });
    Ok(Parsed { contract, about, requires, uses, manifests: games })
}

/// A newer pack's status reads as experimental.
fn status_of(word: &str) -> Status {
    match word {
        "playable" => Status::Playable,
        "beta" => Status::Beta,
        "alpha" => Status::Alpha,
        "pre_alpha" => Status::PreAlpha,
        "in_progress" => Status::InProgress,
        _ => Status::Experimental,
    }
}

/// A newer pack's tab puts the option on the Game tab.
fn shown_of(word: &str) -> Shown {
    match word {
        "main" => Shown::Main,
        "controls" => Shown::Controls,
        "hud" => Shown::Hud,
        "skeleton" => Shown::Skeleton,
        "rendering" => Shown::Rendering,
        "monaka" => Shown::Monaka,
        "line" => Shown::Line,
        _ => Shown::Game,
    }
}

#[cfg(test)]
mod tests;
