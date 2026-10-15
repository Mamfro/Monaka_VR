//! A game page: rows a producer writes for the viewer's debug menu (a tab of its own) and the 2D
//! window, beside the run's channel. A row is a heading, a line of text (a label and its value),
//! or a button; the player's presses on buttons go back to the producer, which acts on them and
//! writes the page again. A tool that finds things in a game (the Explorer) lists them this way,
//! with no code of its own in the viewer. The first row is the page's summary: a page longer than
//! its panel scrolls under it, and it stays in view.

use crate::protocol::{Stamp, Versioned};

#[cfg(test)]
mod tests;

pub const PAGE_MAGIC: u32 = 0x4547_4150;
pub const PAGE_MAJOR: u16 = 2;
/// Rows a page holds; more are dropped.
pub const PAGE_ROWS: usize = 32;
/// Bytes of UTF-8 in a row's label, its value and the page's title (cut at a character).
pub const LABEL_BYTES: usize = 64;
pub const VALUE_BYTES: usize = 128;
pub const TITLE_BYTES: usize = 32;
/// Presses kept for the producer to read (slot = sequence % presses).
pub const PAGE_PRESSES: usize = 16;
/// A page not written for this long (ms) is gone: its producer stopped.
pub const PAGE_FRESH_MS: u64 = 3000;

/// What a row is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u16)]
pub enum Kind {
    #[default]
    Heading = 0,
    Text = 1,
    Button = 2,
}

/// How a row is shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u16)]
pub enum Tone {
    #[default]
    Plain = 0,
    /// Not there yet, or not usable now.
    Dim = 1,
    /// Found, confirmed, working.
    Good = 2,
    /// Needs the player's attention.
    Warn = 3,
}

/// A press on a button: A, or the stick left or right.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Press {
    Switch = 0,
    Left = 1,
    Right = 2,
}

impl Press {
    pub fn from_code(code: u32) -> Option<Press> {
        [Press::Switch, Press::Left, Press::Right].into_iter().find(|p| *p as u32 == code)
    }
}

/// One row as stored.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Row {
    /// The producer's name for a button, sent back with its presses (0 for other rows).
    pub id: u32,
    pub kind: u16,
    pub tone: u16,
    pub label: [u8; LABEL_BYTES],
    pub value: [u8; VALUE_BYTES],
}

impl Default for Row {
    fn default() -> Self {
        Self { id: 0, kind: 0, tone: 0, label: [0; LABEL_BYTES], value: [0; VALUE_BYTES] }
    }
}

impl Row {
    pub fn heading(text: &str) -> Self {
        Self { kind: Kind::Heading as u16, label: fixed(text), ..Self::default() }
    }

    pub fn text(label: &str, value: &str) -> Self {
        Self { kind: Kind::Text as u16, label: fixed(label), value: fixed(value), ..Self::default() }
    }

    /// A button named `id` for its presses.
    pub fn button(id: u32, label: &str, value: &str) -> Self {
        Self { id, kind: Kind::Button as u16, label: fixed(label), value: fixed(value), ..Self::default() }
    }

    pub fn tone(self, tone: Tone) -> Self {
        Self { tone: tone as u16, ..self }
    }

    pub fn kind(&self) -> Kind {
        [Kind::Heading, Kind::Text, Kind::Button].into_iter().find(|k| *k as u16 == self.kind).unwrap_or(Kind::Text)
    }

    pub fn shade(&self) -> Tone {
        [Tone::Plain, Tone::Dim, Tone::Good, Tone::Warn].into_iter().find(|t| *t as u16 == self.tone).unwrap_or(Tone::Plain)
    }

    pub fn label(&self) -> &str {
        text_of(&self.label)
    }

    pub fn value(&self) -> &str {
        text_of(&self.value)
    }
}

/// `text` in `N` bytes, cut at a character, the rest zeros.
pub fn fixed<const N: usize>(text: &str) -> [u8; N] {
    let mut out = [0; N];
    let mut end = text.len().min(N);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    out[..end].copy_from_slice(&text.as_bytes()[..end]);
    out
}

/// The text in `bytes`, up to the first zero (what is not UTF-8 is left out).
pub fn text_of(bytes: &[u8]) -> &str {
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    match std::str::from_utf8(&bytes[..end]) {
        Ok(text) => text,
        Err(e) => std::str::from_utf8(&bytes[..e.valid_up_to()]).unwrap_or(""),
    }
}

/// A page as the viewer shows it: its title and rows, all inline (it is copied into the menu's
/// state, which is compared to know when to draw again).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Page {
    pub title: [u8; TITLE_BYTES],
    pub rows: [Row; PAGE_ROWS],
    pub count: u32,
}

impl Default for Page {
    fn default() -> Self {
        Self { title: [0; TITLE_BYTES], rows: [Row::default(); PAGE_ROWS], count: 0 }
    }
}

impl Page {
    pub fn new(title: &str, rows: &[Row]) -> Self {
        let mut page = Self { title: fixed(title), ..Self::default() };
        let count = rows.len().min(PAGE_ROWS);
        page.rows[..count].copy_from_slice(&rows[..count]);
        page.count = count as u32;
        page
    }

    pub fn title(&self) -> &str {
        text_of(&self.title)
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows[..(self.count as usize).min(PAGE_ROWS)]
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
}

/// A press as the viewer leaves it for the producer.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PagePress {
    pub sequence: u64,
    pub id: u32,
    pub press: u32,
}

/// The shared record beside a channel ([`crate::protocol::ChannelName::page`]).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageBlock {
    pub stamp: Stamp,
    /// Counts the producer's writes; when it last wrote.
    pub generation: u64,
    pub written_tick: u64,
    pub page: Page,
    /// The viewer's presses, newest at `sequence % PAGE_PRESSES`.
    pub presses: [PagePress; PAGE_PRESSES],
    pub press_sequence: u64,
}

impl PageBlock {
    pub fn write(&mut self, page: &Page, tick: u64) {
        self.page = *page;
        self.generation += 1;
        self.written_tick = tick;
    }

    /// The page, while its producer keeps writing it.
    pub fn fresh(&self, now: u64) -> Option<&Page> {
        (self.generation > 0 && now.saturating_sub(self.written_tick) <= PAGE_FRESH_MS).then_some(&self.page)
    }

    pub fn press(&mut self, id: u32, press: Press) {
        self.press_sequence += 1;
        self.presses[(self.press_sequence % PAGE_PRESSES as u64) as usize] = PagePress { sequence: self.press_sequence, id, press: press as u32 };
    }

    /// The presses after `seen` (oldest first; those overwritten are lost), and the newest's
    /// sequence.
    pub fn presses_since(&self, seen: u64) -> (Vec<(u32, Press)>, u64) {
        let from = seen.max(self.press_sequence.saturating_sub(PAGE_PRESSES as u64)) + 1;
        let presses = (from..=self.press_sequence)
            .filter_map(|sequence| {
                let p = self.presses[(sequence % PAGE_PRESSES as u64) as usize];
                (p.sequence == sequence).then(|| Press::from_code(p.press).map(|press| (p.id, press))).flatten()
            })
            .collect();
        (presses, self.press_sequence.max(seen))
    }
}

impl Versioned for PageBlock {
    const MAGIC: u32 = PAGE_MAGIC;
    const MAJOR: u16 = PAGE_MAJOR;
    const MINOR: u16 = 0;
    const WHAT: &'static str = "page block";
    const CAPACITY: usize = 16 * 1024;

    fn stamp(&self) -> Stamp {
        self.stamp
    }

    fn start(&mut self) {
        self.stamp = Stamp::of::<Self>();
    }
}
