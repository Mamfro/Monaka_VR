//! The option area: which options a game shows on which tab, in what order, with which help line,
//! and their controls. What is shown is worked out again every frame from the saved values, so an
//! option that comes or goes with another one's value needs no remaking.

use crate::look;
use eframe::egui::{self, Align, Checkbox, ComboBox, Label, Layout, RichText, Slider, TextEdit};
use monaka_core::manifest::{DEBUG_MENU, DIAGNOSTIC_LOGGING};
use monaka_launch::games::{Game, HowTo, Kind, MONAKA, Manifest, Opt, Shown, TABS, Values, number_text};
use monaka_launch::settings::Settings;

/// The label column; long labels wrap.
const LABEL_WIDTH: f32 = 180.0;
/// Wide windows leave the controls at a readable width.
const CONTROL_WIDTH: f32 = 460.0;

/// One option the window offers: whose it is (the game's, or Monaka's own, saved for every game)
/// and which.
#[derive(Clone, Copy)]
pub struct Row {
    pub owner: &'static Manifest,
    pub opt: &'static Opt,
}

/// What the option area shows, in order.
#[derive(Clone, Copy)]
enum Item {
    Row(Row),
    How(&'static HowTo),
}

/// The game's options with a place in the window: the Rendering tab's in Monaka's order, every
/// other tab's in the game's, then Monaka's own settings. Command-line-only options have none, and
/// the debug menu's switch none unless `debug_menu` (it is found from the About page).
pub fn rows(game: &Game, debug_menu: bool) -> Vec<Row> {
    let owned = |owner: &'static Manifest| owner.options.iter().map(move |opt| Row { owner, opt });
    let mut rows: Vec<Row> = owned(game.manifest).filter(|r| r.opt.shown != Shown::Monaka).collect();
    rows.sort_by_key(|r| monaka_core::rendering::place(r.opt));
    let shown = |r: &Row| r.opt.shown != Shown::Line && (debug_menu || r.opt.id != DEBUG_MENU.id);
    rows.into_iter().chain(owned(&MONAKA)).filter(shown).collect()
}

/// Whether a how-to names `opt`: it is shown under that how-to, on the Controls tab.
fn tuned(game: &Game, opt: &Opt) -> bool {
    game.manifest.controls.iter().any(|how| how.options.contains(&opt.id))
}

/// The tabs this game has options (or how-tos) in, in [`TABS`] order; an option a how-to names
/// counts for the Controls tab.
pub fn tabs(game: &Game, rows: &[Row]) -> Vec<Shown> {
    let tab_of = |row: &Row| if tuned(game, row.opt) { Shown::Controls } else { row.opt.shown };
    let has_hows = !game.manifest.controls.is_empty();
    TABS.iter().map(|(t, _)| *t).filter(|t| rows.iter().any(|r| tab_of(r) == *t) || (*t == Shown::Controls && has_hows)).collect()
}

/// An option that means nothing with the others' values (the aiming hand without a pointing
/// controller), or a choice with one choice left to offer, is left out.
fn offered(values: &Values, row: &Row) -> bool {
    values.applies(row.owner, row.opt) && (!matches!(row.opt.kind, Kind::Choice { .. }) || values.shown_choices(row.owner, row.opt).len() > 1)
}

/// On the Controls tab the options no how-to names, then each how-to with its options under it;
/// on the others, the tab's options.
fn items(game: &Game, rows: &[Row], values: &Values, tab: Shown) -> Vec<Item> {
    let mut order: Vec<Item> = rows.iter().filter(|r| r.opt.shown == tab && !tuned(game, r.opt) && offered(values, r)).map(|r| Item::Row(*r)).collect();
    if tab == Shown::Controls {
        for how in game.manifest.controls {
            let under: Vec<Item> =
                how.options.iter().filter_map(|id| rows.iter().find(|r| r.opt.id == *id)).filter(|r| offered(values, r)).map(|r| Item::Row(*r)).collect();
            // A how-to with nothing to say for these settings (motion controls' ways, played with a
            // gamepad) and nothing to set is left out.
            if !under.is_empty() || !values.ways(game.manifest, how).is_empty() {
                order.push(Item::How(how));
                order.extend(under);
            }
        }
    }
    order
}

/// Draws tab `tab`'s options for `game` and saves what the player changes. While a session runs
/// only diagnostic logging may change (the producer follows it mid-run). Returns the id of the
/// option changed, if any.
pub fn show(ui: &mut egui::Ui, settings: &mut Settings, game: &Game, rows: &[Row], tab: Shown, running: bool) -> Option<&'static str> {
    let values = settings.values(game.id);
    let mut changed = None;
    for item in items(game, rows, &values, tab) {
        match item {
            Item::How(how) => {
                ui.add_space(8.0);
                ui.label(RichText::new(how.action).family(look::semibold()).size(look::HEADING));
                for way in values.ways(game.manifest, how) {
                    ui.add(Label::new(way).wrap());
                }
                ui.add_space(10.0);
            }
            Item::Row(row) => {
                if let Some(value) = option_row(ui, &values, row, running) {
                    let saved = if row.owner.id == MONAKA.id { &mut settings.monaka } else { settings.values.entry(game.id.to_owned()).or_default() };
                    saved.set(row.opt.id, value);
                    let _ = settings.save();
                    changed = Some(row.opt.id);
                }
            }
        }
    }
    changed
}

/// One option: its label, its control with its help line under it. Returns the new value when the
/// player changed it.
fn option_row(ui: &mut egui::Ui, values: &Values, row: Row, running: bool) -> Option<String> {
    let Row { owner, opt } = row;
    let control_width = (ui.available_width() - LABEL_WIDTH - 22.0).clamp(160.0, CONTROL_WIDTH);
    let editable = !running || opt.id == DIAGNOSTIC_LOGGING.id;
    let current = values.text(owner, opt.id);
    let mut new = None;
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(egui::vec2(LABEL_WIDTH, 0.0), Layout::top_down(Align::Min), |ui| {
            ui.set_width(LABEL_WIDTH - 12.0);
            ui.add_space(3.0);
            ui.add(Label::new(opt.label).wrap());
        });
        ui.vertical(|ui| {
            ui.set_width(control_width);
            ui.add_enabled_ui(editable, |ui| match opt.kind {
                Kind::Switch { .. } => {
                    // A switch another option turns off is greyed out, shown off.
                    let mut on = values.switch(owner, opt.id);
                    let free = values.turned_off(owner, opt.id).is_none();
                    if ui.add_enabled(free, Checkbox::new(&mut on, "On")).changed() {
                        new = Some((if on { "1" } else { "0" }).to_owned());
                    }
                }
                Kind::Choice { .. } => {
                    let shown = values.shown_choices(owner, opt);
                    let name = shown.iter().find(|(id, _)| *id == current).or(shown.first()).map_or("", |(_, name)| name);
                    let mut picked = current.clone();
                    ComboBox::from_id_salt(opt.id).width(control_width).selected_text(name).show_ui(ui, |ui| {
                        for (id, choice) in &shown {
                            ui.selectable_value(&mut picked, (*id).to_owned(), *choice);
                        }
                    });
                    if picked != current {
                        new = Some(picked);
                    }
                }
                Kind::Number { min, max, step, .. } => {
                    let mut number = current.parse::<f64>().unwrap_or(min);
                    ui.spacing_mut().slider_width = control_width - 64.0;
                    let slider = Slider::new(&mut number, min..=max).step_by(step).custom_formatter(|n, _| number_text(n));
                    if ui.add(slider).changed() {
                        new = Some(number_text(number));
                    }
                }
                Kind::Text | Kind::Size => {
                    let mut text = current.clone();
                    if ui.add(TextEdit::singleline(&mut text).desired_width(control_width)).changed() {
                        new = Some(text);
                    }
                }
            });
            // The help line says what this option turns off, or why it is off now.
            let help = help_text(owner, values, opt);
            if !help.is_empty() {
                ui.add(Label::new(RichText::new(help).size(look::SMALL).weak()).wrap());
            }
        });
    });
    ui.add_space(10.0);
    new
}

/// An option's help line with these values: why it is off now (another option turns it off), or
/// its own help and what it turns off.
fn help_text(game: &Manifest, values: &Values, opt: &Opt) -> String {
    if let Some((cause, rule)) = values.turned_off(game, opt.id) {
        return format!("Off while {} is {}. {}", cause.label, value_name(cause, rule.value), rule.reason).trim_end().to_owned();
    }
    match opt.turns_off.and_then(|rule| game.option(rule.target).map(|target| (rule, target))) {
        Some((rule, target)) => format!("{} {} turns off {}.", opt.help, set_to(opt, rule.value), target.label),
        None => opt.help.to_owned(),
    }
}

/// How the window names `value` of `opt`: on or off, or a choice's label; a rule's `!value`, "not"
/// it.
fn value_name(opt: &Opt, value: &str) -> String {
    if let Some(not) = value.strip_prefix('!') {
        return format!("not {}", value_name(opt, not));
    }
    match opt.kind {
        Kind::Switch { .. } => (if value == "1" { "on" } else { "off" }).to_owned(),
        Kind::Choice { choices, .. } => choices.iter().find(|(id, _)| *id == value).map_or(value, |(_, name)| name).to_owned(),
        _ => value.to_owned(),
    }
}

/// "Turning it on", "Turning it off", "Set to <choice>," or "Set to anything but <choice>,".
fn set_to(opt: &Opt, value: &str) -> String {
    match (opt.kind, value.strip_prefix('!')) {
        (Kind::Switch { .. }, _) => format!("Turning it {}", value_name(opt, value)),
        (_, Some(not)) => format!("Set to anything but {},", value_name(opt, not)),
        (_, None) => format!("Set to {},", value_name(opt, value)),
    }
}

#[cfg(test)]
mod tests;
