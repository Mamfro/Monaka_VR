//! The console panel's page (`monaka_core::console`): what the producer's probe console has in (its
//! probes, the next step of a search for an action's code) and the log of what was asked of it and
//! answered, whoever asked, the newest at the bottom; a box to type commands in (Enter sends,
//! the arrows bring back earlier ones), and buttons for what needs no typing.
//!
//! The headset has no keyboard: there the stick (or the D-pad) picks one of the buttons and A
//! runs it, and the right stick scrolls the log ([`ConsoleNav`], [`headset_layout`]); commands are
//! typed in the 2D window.

use crate::menu::Press;
use crate::menu::panel::{ACCENT, DIM, ROW, TEXT};
use egui::{CornerRadius, RichText, ScrollArea, Stroke, TextEdit, TextStyle};

/// What the console panel shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConsoleView {
    /// Whether the run's console answers (a game pack built before it: nothing to show).
    pub open: bool,
    pub status: String,
    pub log: String,
}

/// The probe buttons: their labels and the commands they ask.
pub const PROBE_BUTTONS: [(&str, &str); 3] = [("List", "list"), ("Timeline", "timeline 1"), ("Remove probes", "unhook all")];

/// A search for the code an action runs (what runs when you Blink): Start watches every function,
/// then rounds in any order narrow the candidates: after doing the action, those that ran are kept;
/// after playing without it, those that did not.
pub const FIND_BUTTONS: [(&str, &str, &str); 6] = [
    ("Start", "cover start", "watch every function of the game"),
    ("Action", "cover action", "you did the action: keep only the candidates that ran"),
    ("Inaction", "cover inaction", "you played without the action: keep only the candidates that did not run"),
    ("Undo", "cover undo", "back to the candidates before the last Action or Inaction (then do that round again)"),
    ("Candidates", "cover candidates", "list what is left"),
    ("Stop", "cover stop", "stop watching"),
];

/// The headset's buttons, row by row: the probe buttons and the console's help, then the search's.
pub const HEADSET_PROBES: [(&str, &str, &str); 4] =
    [("List", "list", "what is hooked"), ("Timeline", "timeline 1", "what ran, in order"), ("Remove probes", "unhook all", "take every probe out"), ("Help", "help", "every command the console takes")];
pub const HEADSET_ROWS: [(&str, &[(&str, &str, &str)]); 2] = [("Probes", &HEADSET_PROBES), ("Find the code for an action", &FIND_BUTTONS)];

/// Where the headset's console is: the button picked (its row and place in it) and how many lines
/// up from the newest the log shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ConsoleNav {
    pub row: usize,
    pub column: usize,
    pub scroll: usize,
}

impl ConsoleNav {
    /// A press of the stick or A: up and down change the row, left and right the button in it
    /// (both round), A gives the picked button's command.
    pub fn press(&mut self, press: Press) -> Option<String> {
        let rows = HEADSET_ROWS.len();
        let count = |row: usize| HEADSET_ROWS[row].1.len();
        match press {
            Press::Up => self.row = (self.row + rows - 1) % rows,
            Press::Down => self.row = (self.row + 1) % rows,
            Press::Left => self.column = (self.column + count(self.row) - 1) % count(self.row),
            Press::Right => self.column = (self.column + 1) % count(self.row),
            Press::Switch => return Some(self.picked().1.to_owned()),
            _ => {}
        }
        self.column = self.column.min(count(self.row) - 1);
        None
    }

    /// The button picked: its label, command and what it does.
    pub fn picked(&self) -> (&'static str, &'static str, &'static str) {
        let row = HEADSET_ROWS[self.row.min(HEADSET_ROWS.len() - 1)].1;
        row[self.column.min(row.len() - 1)]
    }

    /// The log scrolled `lines` up (negative: down, toward the newest).
    pub fn scroll(&mut self, lines: i32) {
        self.scroll = (self.scroll as i64 + i64::from(lines)).clamp(0, SHOWN_LINES as i64) as usize;
    }
}

/// The `room` lines of `log` (as many as fit) the headset shows `scroll` lines up from the newest,
/// as far up as it goes: the first one's index, the last's (exclusive) and how many there are.
pub fn headset_window(log: &str, scroll: usize, room: usize) -> (usize, usize, usize) {
    let count = log.lines().count().min(SHOWN_LINES);
    let end = count - scroll.min(count.saturating_sub(room));
    (end.saturating_sub(room), end, count)
}

/// The page as the headset shows it (`active`: the pad drives it): the buttons with the one
/// picked marked, what it does, the status, and the log at its scroll.
pub fn headset_layout(ui: &mut egui::Ui, view: &ConsoleView, nav: &ConsoleNav, active: bool) {
    if !view.open {
        ui.label(RichText::new("No console answers in this run: its game pack may be older than the console.").size(13.0).color(DIM));
        return;
    }
    for (r, (heading, row)) in HEADSET_ROWS.iter().enumerate() {
        ui.label(RichText::new(*heading).size(13.0).color(DIM));
        ui.horizontal_wrapped(|ui| {
            for (c, (label, _, _)) in row.iter().enumerate() {
                let picked = nav.row == r && nav.column == c;
                // A button of egui's: one that does not fit goes to the next line whole.
                let button = egui::Button::new(RichText::new(*label).size(13.0).color(TEXT))
                    .fill(if picked { ROW } else { egui::Color32::TRANSPARENT })
                    .stroke(Stroke::new(1.0, if picked && active { ACCENT } else { DIM }))
                    .corner_radius(CornerRadius::same(6))
                    .sense(egui::Sense::hover());
                ui.add(button);
            }
        });
    }
    let (_, command, does) = nav.picked();
    ui.label(RichText::new(format!("A runs \"{command}\": {does}. Typed commands: the 2D window.")).size(12.0).color(DIM));
    ui.add_space(4.0);
    let status = if view.status.trim().is_empty() { "nothing in" } else { view.status.trim_end() };
    ui.label(RichText::new(status).size(13.0).color(ACCENT).text_style(TextStyle::Monospace));
    ui.separator();
    let lines: Vec<&str> = view.log.lines().collect();
    let lines = &lines[lines.len().saturating_sub(SHOWN_LINES)..];
    // As many lines as fit under the line saying which they are, each cut at the panel's edge.
    let line = ui.fonts_mut(|f| f.row_height(&egui::FontId::monospace(12.0)));
    let room = ((ui.available_height() - 2.0 * line) / line).floor().max(1.0) as usize;
    let (first, last, count) = headset_window(&view.log, nav.scroll, room);
    if count > room {
        ui.label(RichText::new(format!("Lines {} to {last} of {count} (right stick scrolls)", first + 1)).size(12.0).color(DIM));
    }
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for text in &lines[first..last] {
            ui.add(egui::Label::new(RichText::new(*text).size(12.0).color(TEXT).text_style(TextStyle::Monospace)).wrap_mode(egui::TextWrapMode::Truncate));
        }
    });
}

/// The log's lines shown at most (the newest).
const SHOWN_LINES: usize = 400;
/// Commands typed kept for the arrows.
const HISTORY: usize = 50;

/// What the box holds and the commands typed before, kept in egui's memory between frames.
#[derive(Clone, Default)]
struct Typing {
    text: String,
    history: Vec<String>,
    /// Where the arrows are in the history (its length: past the newest).
    at: usize,
}

/// The page; the command typed or of a button pressed.
pub fn layout(ui: &mut egui::Ui, view: &ConsoleView) -> Option<String> {
    if !view.open {
        ui.label(RichText::new("No console answers in this run: its game pack may be older than the console.").size(13.0).color(DIM));
        return None;
    }
    let mut asked = None;
    let mut buttons = |ui: &mut egui::Ui, heading: &str, list: &mut dyn Iterator<Item = (&'static str, &'static str, &'static str)>| {
        ui.label(RichText::new(heading).size(13.0).color(DIM));
        ui.horizontal_wrapped(|ui| {
            for (label, command, help) in list {
                let button = ui.button(RichText::new(label).size(13.0));
                let button = if help.is_empty() { button } else { button.on_hover_text(help) };
                if button.clicked() {
                    asked = Some(command.to_owned());
                }
            }
        });
    };
    buttons(ui, "Probes", &mut PROBE_BUTTONS.into_iter().map(|(l, c)| (l, c, "")));
    buttons(ui, "Find the code for an action", &mut FIND_BUTTONS.into_iter());
    ui.add_space(4.0);
    let status = if view.status.trim().is_empty() { "nothing in" } else { view.status.trim_end() };
    ui.label(RichText::new(status).size(13.0).color(ACCENT).text_style(TextStyle::Monospace));
    ui.separator();
    if let Some(typed) = typing(ui) {
        asked = Some(typed);
    }
    let lines: Vec<&str> = view.log.lines().collect();
    let shown = lines[lines.len().saturating_sub(SHOWN_LINES)..].join("\n");
    ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).max_height(420.0).show(ui, |ui| {
        // Selectable, so an address can be copied out of an answer.
        ui.add(egui::Label::new(RichText::new(shown).size(12.0).color(TEXT).text_style(TextStyle::Monospace)).selectable(true));
    });
    asked
}

/// The box commands are typed in: the command sent with Enter.
fn typing(ui: &mut egui::Ui) -> Option<String> {
    let id = ui.id().with("console typing");
    let mut typing: Typing = ui.data_mut(|d| d.get_temp(id)).unwrap_or_default();
    let mut sent = None;
    let response = ui.add(
        TextEdit::singleline(&mut typing.text)
            .hint_text("type a command and press Enter (help lists them)")
            .font(TextStyle::Monospace)
            .desired_width(f32::INFINITY),
    );
    if response.has_focus() {
        let (up, down) = ui.input(|i| (i.key_pressed(egui::Key::ArrowUp), i.key_pressed(egui::Key::ArrowDown)));
        if up && typing.at > 0 {
            typing.at -= 1;
            typing.text = typing.history[typing.at].clone();
        } else if down && typing.at < typing.history.len() {
            typing.at += 1;
            typing.text = typing.history.get(typing.at).cloned().unwrap_or_default();
        }
    }
    // Enter makes the box let go: the command is sent and the box takes focus again for the next.
    if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        let command = typing.text.trim().to_owned();
        if !command.is_empty() {
            if typing.history.last() != Some(&command) {
                typing.history.push(command.clone());
                if typing.history.len() > HISTORY {
                    typing.history.remove(0);
                }
            }
            typing.at = typing.history.len();
            typing.text.clear();
            sent = Some(command);
        }
        response.request_focus();
    }
    ui.data_mut(|d| d.insert_temp(id, typing));
    sent
}

#[cfg(test)]
mod tests;
