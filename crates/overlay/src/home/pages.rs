//! The home's panels drawn with egui, each into a texture of its own ([`Canvas`]), and only when
//! something on it changed. Made for a pointer at arm's length: large type and targets, no
//! drop-down lists (a choice shows all its values as buttons), a cursor where the controller
//! points. Each panel keeps to its image: a title, a body of fixed height that scrolls when it
//! holds more, and the footer.

use super::place::PIXELS;
use super::{Home, LogSource, PAGES, Page, Setting};
use crate::Result;
use crate::canvas::{Canvas, Cursor};
use crate::menu::panel::{ACCENT, BACKGROUND, DIM, ON, ROW, TEXT};
use egui::{Align, Color32, CornerRadius, FontId, Frame, Layout, Margin, RichText, ScrollArea, Stroke, TextStyle, Vec2};
use monaka_contract::manifest::{Kind, TABS, number_text};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;

/// The three panels' images and where the pointer is on each (points).
pub struct Pages {
    canvases: [Canvas; 3],
    cursors: [Cursor; 3],
}

impl Pages {
    /// Panels drawn in `format` (the headset's image format).
    pub fn new(format: DXGI_FORMAT) -> Self {
        Self { canvases: std::array::from_fn(|_| Canvas::new(format, PIXELS)), cursors: [Cursor::default(); 3] }
    }

    /// A panel's size in points, where the pointer's events are given.
    pub fn points(&self) -> Vec2 {
        self.canvases[0].points()
    }

    /// Draws panel `page` with what the pointer did on it (`events`); its image.
    pub fn draw(&mut self, page: usize, device: &ID3D11Device, context: &ID3D11DeviceContext, home: &mut Home, events: Vec<egui::Event>) -> Result<Option<&ID3D11Texture2D>> {
        self.cursors[page].follow(&events);
        let cursor = self.cursors[page];
        let front = home.front == page;
        // A list stuck to its bottom (the log) learns its height in one pass and scrolls there in
        // the next: its panel is laid out once more first, so the image shows the newest lines.
        if PAGES[page] == Page::Log {
            let _ = self.canvases[page].draw_with(device, context, Vec::new(), |ui| {
                style(ui);
                framed(ui, PAGES[page], front, |ui| log(ui, home));
            })?;
        }
        self.canvases[page].draw_with(device, context, events, |ui| {
            style(ui);
            framed(ui, PAGES[page], front, |ui| match PAGES[page] {
                Page::Games => games(ui, home),
                Page::Settings => settings(ui, home),
                Page::Log => log(ui, home),
            });
            cursor.paint(ui.ctx());
        })
    }
}

/// Type and targets large enough at a metre or so.
fn style(ui: &mut egui::Ui) {
    let style = ui.style_mut();
    style.text_styles.insert(TextStyle::Body, FontId::proportional(15.0));
    style.text_styles.insert(TextStyle::Button, FontId::proportional(15.0));
    style.text_styles.insert(TextStyle::Small, FontId::proportional(12.0));
    style.text_styles.insert(TextStyle::Monospace, FontId::monospace(11.0));
    style.spacing.button_padding = Vec2::new(12.0, 6.0);
    style.spacing.item_spacing = Vec2::new(8.0, 6.0);
    style.spacing.interact_size.y = 30.0;
    style.spacing.scroll.bar_width = 12.0;
    style.visuals = egui::Visuals::dark();
    style.visuals.override_text_color = Some(TEXT);
    style.visuals.selection.bg_fill = ROW;
    style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.5, ACCENT);
}

/// Space inside a panel's edge, the outline round the one in front, and the footer's height
/// (points).
const EDGE: i8 = 16;
const OUTLINE: f32 = 3.0;
const FOOTER: f32 = 22.0;

/// The panel's frame: the title, `body` in the room left above the footer (clipped to it), and
/// the footer; outlined while it is in front. All of it inside the image, whatever `body` holds.
fn framed(ui: &mut egui::Ui, page: Page, front: bool, body: impl FnOnce(&mut egui::Ui)) {
    let inside = ui.available_size() - Vec2::splat(2.0 * (EDGE as f32 + OUTLINE));
    let outline = Stroke::new(OUTLINE, if front { ACCENT } else { Color32::TRANSPARENT });
    Frame::new().fill(BACKGROUND).stroke(outline).corner_radius(CornerRadius::same(14)).inner_margin(Margin::same(EDGE)).show(ui, |ui| {
        ui.set_min_size(inside);
        ui.set_max_size(inside);
        ui.label(RichText::new(page.title()).size(22.0).strong());
        ui.add_space(4.0);
        let room = Vec2::new(inside.x, (ui.available_height() - FOOTER).max(0.0));
        ui.allocate_ui_with_layout(room, Layout::top_down(Align::Min), |ui| {
            ui.set_min_size(room);
            ui.set_max_size(room);
            ui.set_clip_rect(ui.max_rect().intersect(ui.clip_rect()));
            body(ui);
        });
        ui.add_space(4.0);
        ui.label(RichText::new("Point and pull the trigger.  Right stick: scroll.  LB/RB: panels.  B or hold Start: close.").size(11.0).color(DIM));
    });
}

/// The Games panel: the game shown, what can be done with it, and the games on offer. In the
/// Monaka window's own headset view a game is picked from the list and played or stopped here.
fn games(ui: &mut egui::Ui, home: &mut Home) {
    ScrollArea::vertical().id_salt("games").auto_shrink([false, false]).show(ui, |ui| {
        match home.game {
            Some(game) => {
                ui.label(RichText::new(game.name).size(19.0).strong());
                ui.label(RichText::new(format!("{}: {}", game.status.label(), game.status.meaning())).color(DIM));
                let about = &home.about;
                let mut made = String::new();
                if !about.version.is_empty() {
                    made.push_str(&format!("Game pack {}", about.version));
                }
                if !about.author.is_empty() {
                    made.push_str(&format!("{}by {}", if made.is_empty() { "" } else { " " }, about.author));
                }
                if !made.is_empty() {
                    ui.label(RichText::new(made).color(DIM));
                }
            }
            None if home.window => {
                ui.label(RichText::new("No game picked yet: pick one below.").color(DIM));
            }
            None => {
                ui.label(RichText::new("Started without the launcher: no game to show.").color(DIM));
            }
        }
        ui.add_space(10.0);
        let stop = |ui: &mut egui::Ui, label: &str| ui.add(egui::Button::new(RichText::new(label).strong()).fill(Color32::from_rgb(120, 40, 36))).clicked();
        ui.horizontal(|ui| {
            if !home.window {
                if stop(ui, "Stop VR") {
                    home.ask_stop();
                }
            } else if home.playing().is_some() {
                if stop(ui, "Stop game") {
                    home.ask_stop();
                }
            } else if home.game.is_some() && ui.add(egui::Button::new(RichText::new("Play").strong()).fill(ROW)).clicked() {
                home.ask_play();
            }
            if ui.button("Recenter panels").clicked() {
                home.ask_recenter();
            }
            if home.window && ui.button("Leave VR").clicked() {
                home.ask_leave();
            }
        });
        if let Some(playing) = home.playing().filter(|p| home.window && !p.current) {
            ui.label(RichText::new(format!("Playing now: {}", playing.name)).color(ON));
        }
        ui.add_space(12.0);
        if home.games.is_empty() {
            return;
        }
        ui.label(RichText::new("Your games").size(17.0).strong());
        let mut picked = None;
        for listed in &home.games {
            let state = if listed.playing { "Playing" } else { listed.status.as_str() };
            let row = Frame::new().fill(if listed.current { ROW } else { Color32::TRANSPARENT }).corner_radius(CornerRadius::same(6)).inner_margin(Margin::symmetric(10, 4)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(&listed.name);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new(state).color(if listed.playing { ON } else { DIM }));
                    });
                });
            });
            // In the window's own view a row is picked by pointing at it.
            if home.window && !listed.current && row.response.interact(egui::Sense::click()).clicked() {
                picked = Some(listed.id.clone());
            }
        }
        if let Some(id) = picked {
            home.ask_select(&id);
        }
        if !home.window {
            ui.label(RichText::new("Starting another game from the headset is not here yet: use the window.").size(12.0).color(DIM));
        }
    });
}

fn settings(ui: &mut egui::Ui, home: &mut Home) {
    let tabs = home.tabs();
    if tabs.is_empty() {
        ui.label(RichText::new("Nothing to set: the viewer was started without the launcher.").color(DIM));
        return;
    }
    if !tabs.contains(&home.tab) {
        home.show_tab(tabs[0]);
    }
    ui.horizontal_wrapped(|ui| {
        for tab in &tabs {
            let label = TABS.iter().find(|(t, _)| t == tab).map_or("", |(_, label)| *label);
            if ui.add(egui::Button::selectable(home.tab == *tab, label)).clicked() {
                home.show_tab(*tab);
            }
        }
    });
    ui.separator();
    apply_bar(ui, home);
    let shown: Vec<Setting> = home.settings().into_iter().filter(|s| s.opt.shown == home.tab).collect();
    ScrollArea::vertical().id_salt(("settings", home.tab as u8)).auto_shrink([false, false]).show(ui, |ui| {
        for setting in shown {
            setting_row(ui, home, &setting);
            ui.add_space(4.0);
        }
    });
}

/// What waits for the game to start again, and the button that does it now; or that it is.
fn apply_bar(ui: &mut egui::Ui, home: &mut Home) {
    if home.pending.is_empty() && !home.applying && home.problem.is_none() {
        return;
    }
    Frame::new().fill(ROW).corner_radius(CornerRadius::same(8)).inner_margin(Margin::symmetric(10, 6)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let said = if home.applying {
                "Applying: the game comes back in a few seconds.".to_owned()
            } else if home.pending.len() == 1 {
                "1 change waits for the game's VR to restart.".to_owned()
            } else {
                format!("{} changes wait for the game's VR to restart.", home.pending.len())
            };
            ui.label(RichText::new(said).size(14.0));
            if !home.applying && home.can_apply() && !home.pending.is_empty() {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.add(egui::Button::new(RichText::new("Apply now").strong()).fill(Color32::from_rgb(70, 110, 60))).clicked() {
                        home.ask_apply();
                    }
                });
            }
        });
        if let Some(problem) = &home.problem {
            ui.label(RichText::new(problem).size(12.0).color(ACCENT));
        }
    });
    ui.add_space(4.0);
}

/// One setting: its label and when a change applies, its control, its help line.
fn setting_row(ui: &mut egui::Ui, home: &mut Home, setting: &Setting) {
    let value = home.value(setting);
    Frame::new().fill(Color32::from_white_alpha(6)).corner_radius(CornerRadius::same(8)).inner_margin(Margin::symmetric(10, 6)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(RichText::new(setting.opt.label).size(16.0));
            // When a change applies: at once, or (while its game plays) once VR restarts in it;
            // with no game playing, simply when it starts.
            let when = if setting.live() { Some(("applies now", ON)) } else { home.can_apply().then_some(("applies on restart", DIM)) };
            if let Some((when, colour)) = when {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(when).size(11.0).color(colour));
                });
            }
        });
        match setting.opt.kind {
            Kind::Switch { .. } => {
                let on = value == "1";
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::selectable(on, "On")).clicked() {
                        home.set(setting, "1".to_owned());
                    }
                    if ui.add(egui::Button::selectable(!on, "Off")).clicked() {
                        home.set(setting, "0".to_owned());
                    }
                });
            }
            Kind::Choice { .. } => {
                let choices = home.values.shown_choices(setting.owner, setting.opt);
                ui.horizontal_wrapped(|ui| {
                    for (id, label) in choices {
                        if ui.add(egui::Button::selectable(value == id, label)).clicked() {
                            home.set(setting, id.to_owned());
                        }
                    }
                });
            }
            Kind::Number { min, max, step, .. } => {
                let now = value.parse::<f64>().unwrap_or(min);
                ui.horizontal(|ui| {
                    if ui.add_enabled(now > min, egui::Button::new(RichText::new("  -  ").strong())).clicked() {
                        home.set(setting, number_text((now - step).clamp(min, max)));
                    }
                    ui.label(RichText::new(number_text(now)).size(17.0).strong());
                    if ui.add_enabled(now < max, egui::Button::new(RichText::new("  +  ").strong())).clicked() {
                        home.set(setting, number_text((now + step).clamp(min, max)));
                    }
                    ui.label(RichText::new(format!("{} to {}", number_text(min), number_text(max))).size(11.0).color(DIM));
                });
            }
            Kind::Text | Kind::Size => {
                let shown = if value.is_empty() { "(none)" } else { value.as_str() };
                ui.label(RichText::new(format!("{shown}   (typed in the window)")).color(DIM));
            }
        }
        if !setting.opt.help.is_empty() {
            ui.label(RichText::new(setting.opt.help).size(12.0).color(DIM));
        }
    });
}

fn log(ui: &mut egui::Ui, home: &mut Home) {
    let game_log = home.game.map_or("Game", |g| g.name);
    ui.horizontal(|ui| {
        for (source, label) in [(LogSource::Monaka, "Monaka"), (LogSource::Game, game_log)] {
            if ui.add(egui::Button::selectable(home.log.source == source, label)).clicked() {
                home.show_log(source);
            }
        }
        // The debug menu's tools, for a tester (its own menu needs Back held, which some
        // controllers have no button for).
        if home.debug {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Debug menu").clicked() {
                    home.ask_debug_menu();
                }
                if ui.button("Save capture").clicked() {
                    home.ask_capture();
                }
            });
        }
    });
    ui.separator();
    ScrollArea::vertical().id_salt("log").stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
        for line in &home.log.lines {
            ui.label(RichText::new(line).monospace().size(11.0));
        }
    });
}

#[cfg(test)]
mod tests;
