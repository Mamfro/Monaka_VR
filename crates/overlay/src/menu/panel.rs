//! The menu drawn with egui into a texture of its own ([`Canvas`]), again only when what it shows
//! changed.

use super::{Click, Item, Menu, PAGE_TAB, Press, Settings, Status, TABS};
use monaka_contract::page::{Kind, Tone};
use crate::Result;
use crate::canvas::{Canvas, Cursor};
use egui::{Align2, Color32, CornerRadius, FontId, Frame, Margin, RichText, Sense, Stroke, StrokeKind, Vec2};
use windows::Win32::Graphics::Direct3D11::*;
#[cfg(test)]
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;

#[cfg(test)]
mod tests;

/// The panel's image size (pixels): a ring panel's, as the home's.
pub const PIXELS: [u32; 2] = crate::home::place::PIXELS;

// The headset panels' colours (also [`crate::settings`]'s).
pub(crate) const BACKGROUND: Color32 = Color32::from_rgba_premultiplied(18, 20, 26, 235);
pub(crate) const ROW: Color32 = Color32::from_rgb(52, 70, 110);
pub(crate) const TEXT: Color32 = Color32::from_rgb(230, 232, 238);
pub(crate) const DIM: Color32 = Color32::from_rgb(120, 124, 135);
pub(crate) const ON: Color32 = Color32::from_rgb(120, 220, 140);
pub(crate) const ACCENT: Color32 = Color32::from_rgb(255, 170, 60);
const WARN: Color32 = Color32::from_rgb(240, 110, 90);

pub struct Panel {
    canvas: Canvas,
    cursor: Cursor,
    /// What the last image showed: the menu, its settings and status, and whether it was in front.
    shown: Option<(Menu, Settings, Status, bool)>,
}

/// The outline round the panel the pad drives, in the headset (points); the others keep the same
/// room, clear, so nothing moves when it goes elsewhere.
pub const OUTLINE: f32 = 4.0;

/// The outline's stroke: the accent while `on`, clear otherwise.
pub(crate) fn outline(on: bool) -> Stroke {
    Stroke::new(OUTLINE, if on { ACCENT } else { Color32::TRANSPARENT })
}

impl Panel {
    /// A panel drawn in `format` (the headset's image format).
    pub fn new(format: DXGI_FORMAT) -> Self {
        Self { canvas: Canvas::new(format, PIXELS), cursor: Cursor::default(), shown: None }
    }

    /// The menu's latest image, once drawn.
    pub fn image(&self) -> Option<&ID3D11Texture2D> {
        self.canvas.image()
    }

    /// Forgets what was drawn: the next [`Panel::draw`] draws (its image went somewhere new).
    pub fn forget(&mut self) {
        self.shown = None;
    }

    /// Draws the menu as a page on the debug ring ([`crate::ring_page`]) when it shows something
    /// new, was moved to or from the front (`front`), or the pointer did anything on it
    /// (`events`, the cursor drawn where it is); the image when drawn, and the click made, if one.
    pub fn draw(&mut self, device: &ID3D11Device, context: &ID3D11DeviceContext, menu: &Menu, settings: &Settings, status: &Status, front: bool, events: Vec<egui::Event>) -> Result<(Option<&ID3D11Texture2D>, Option<Click>)> {
        let now = (*menu, *settings, *status, front);
        if self.shown == Some(now) && self.canvas.image().is_some() && events.is_empty() {
            return Ok((None, None));
        }
        self.cursor.follow(&events);
        let cursor = self.cursor;
        let mut click = None;
        let image = self.canvas.draw_with(device, context, events, |ui| {
            crate::ring_page::style(ui, &crate::ring_page::DEBUG);
            click = crate::ring_page::framed(ui, &crate::ring_page::DEBUG, "Monaka debug", front, crate::panels::KEYS, |ui| {
                egui::ScrollArea::vertical().id_salt("menu").auto_shrink([false, false]).show(ui, |ui| {
                    let (clicked, shown) = body(ui, menu, settings, status, Sense::click());
                    notes(ui, status, shown, true);
                    clicked
                })
                .inner
            });
            cursor.paint(ui.ctx(), crate::ring_page::DEBUG.accent);
        })?;
        if image.is_some() {
            self.shown = Some(now);
        }
        Ok((image, click))
    }
}

/// Space inside the panel's edge, and inside a row's (points).
const EDGE: i8 = 18;
const ROW_EDGE: i8 = 10;
/// A game page row's height (the gap under it is the style's item spacing), and the room kept for
/// the footer (points).
const PAGE_ROW: f32 = 22.0;
const FOOTER: f32 = 48.0;

/// The menu's page; with a `pointer` (the 2D window) its tabs and lines take clicks, and the one
/// made is returned. `outlined`: the pad drives it (the headset, beside other panels).
pub fn layout(ui: &mut egui::Ui, menu: &Menu, settings: &Settings, status: &Status, pointer: bool, outlined: bool) -> Option<Click> {
    let sense = if pointer { Sense::click() } else { Sense::hover() };
    let inside = ui.available_size() - Vec2::splat(2.0 * (EDGE as f32 + OUTLINE));
    Frame::new()
        .fill(BACKGROUND)
        .stroke(outline(outlined))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(Margin::same(EDGE))
        .show(ui, |ui| {
            ui.set_min_size(inside);
            ui.set_max_size(inside);
            ui.label(RichText::new("Monaka debug").size(20.0).color(TEXT).strong());
            ui.add_space(6.0);
            let (clicked, shown) = body(ui, menu, settings, status, sense);
            ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                let keys = if pointer { "Click: switch   Right-click: step back" } else { "Stick / D-pad: move   A: switch   LB / RB: panels   LT / RT: tabs   B: close" };
                ui.label(RichText::new(keys).size(12.0).color(DIM));
                notes(ui, status, shown, pointer);
            });
            clicked
        })
        .inner
}

/// What the menu shows under its title: its tabs, and the tab's lines (or the game's page); the
/// press made on them (`sense` taking clicks), and which of the page's rows show when it is the
/// page.
fn body(ui: &mut egui::Ui, menu: &Menu, settings: &Settings, status: &Status, sense: Sense) -> (Option<Click>, Option<(usize, usize, usize)>) {
    let mut clicked = None;
    let row_width = ui.available_width() - 2.0 * ROW_EDGE as f32 - 2.0;
    ui.horizontal(|ui| {
        for (index, tab) in TABS.iter().enumerate() {
            if index == PAGE_TAB && status.page.is_empty() {
                continue;
            }
            let chosen = index == menu.tab;
            let name = if index == PAGE_TAB && !status.page.title().is_empty() { status.page.title() } else { tab.name };
            let text = RichText::new(name).size(15.0).color(if chosen { Color32::BLACK } else { DIM });
            let tab = Frame::new().fill(if chosen { ACCENT } else { Color32::TRANSPARENT }).corner_radius(CornerRadius::same(8)).inner_margin(Margin::symmetric(10, 4)).show(ui, |ui| {
                ui.label(text);
            });
            if pressed(&tab.response.interact(sense)).is_some() {
                clicked = Some(Click::Tab(index));
            }
        }
    });
    ui.add_space(8.0);
    let shown = if menu.tab == PAGE_TAB { Some(page(ui, menu, status, row_width, sense, &mut clicked)) } else { None };
    for (line, item) in TABS[menu.tab].items.iter().enumerate() {
        let selected = line == menu.selected[menu.tab];
        let available = Menu::available(*item, settings, status);
        let (name, value) = describe(*item, settings, status);
        let row = Frame::new()
            .fill(if selected { ROW } else { Color32::TRANSPARENT })
            .stroke(if selected { Stroke::new(1.0, ACCENT) } else { Stroke::NONE })
            .corner_radius(CornerRadius::same(6))
            .inner_margin(Margin::symmetric(ROW_EDGE, 4))
            .show(ui, |ui| {
                ui.set_width(row_width);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(name).size(15.0).color(if available { TEXT } else { DIM }));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let colour = match (available, value.as_str()) {
                            (false, _) => DIM,
                            (true, "On") => ON,
                            _ => TEXT,
                        };
                        ui.label(RichText::new(value).size(15.0).color(colour));
                    });
                });
            });
        if let Some(press) = pressed(&row.response.interact(sense)) {
            clicked = Some(Click::Line(line, press));
        }
    }
    (clicked, shown)
}

/// What the menu says under its lines: which of the page's rows show, or how the drawings are
/// drawn.
fn notes(ui: &mut egui::Ui, status: &Status, shown: Option<(usize, usize, usize)>, pointer: bool) {
    if let Some((first, last, count)) = shown {
        if last - first < count {
            let scrolls = if pointer { "the selection scrolls" } else { "the stick scrolls" };
            ui.label(RichText::new(format!("Rows {} to {last} of {count}: {scrolls}", first + 1)).size(12.0).color(DIM));
        }
        return;
    }
    let drawings = if status.synced { "on each eye's own frame" } else { "on the newest frame (this game's pairs name no frames)" };
    let depth = if status.depth { "depth: yes" } else { "depth: no" };
    ui.label(RichText::new(format!("Drawings {drawings}; {depth}")).size(12.0).color(DIM));
}

/// What a pointer's click on `response` presses: the primary button switches, the secondary steps
/// back.
fn pressed(response: &egui::Response) -> Option<Press> {
    if response.clicked() {
        Some(Press::Switch)
    } else if response.secondary_clicked() {
        Some(Press::Left)
    } else {
        None
    }
}

/// An item's name and its value as shown.
fn describe(item: Item, settings: &Settings, status: &Status) -> (String, String) {
    let on_off = |on: bool| if on { "On".to_owned() } else { "Off".to_owned() };
    match item {
        Item::Category(category) => {
            let value = if status.offered.has(category) { on_off(settings.wanted.has(category)) } else { "Not in this game".to_owned() };
            (category.label().to_owned(), value)
        }
        Item::View => {
            let missing = if settings.view.needs_depth() && !status.depth { "  (no depth in this game)" } else { "" };
            ("Eyes show".to_owned(), format!("<  {}{missing}  >", settings.view.label()))
        }
        Item::DepthPass => {
            let value = match (settings.depth_pass, status.depth_passes) {
                (_, 0) => "Not in this game".to_owned(),
                (0, _) => "<  At the present  >".to_owned(),
                (pass, passes) => format!("<  Before clear {pass} of {passes}  >"),
            };
            ("Depth taken".to_owned(), value)
        }
        Item::DepthNear => ("Near is".to_owned(), format!("<  {}  >", settings.depth_near.label())),
        Item::DepthRange => ("Black at".to_owned(), if settings.depth_log { "1 km (Log)".to_owned() } else { format!("<  {} m  >", settings.depth_range) }),
        Item::DepthCurve => ("Curve".to_owned(), format!("<  {}  >", if settings.depth_log { "Log" } else { "Linear" })),
        Item::Target => {
            let count = status.target_count;
            let describe = |n: u32| {
                status.targets.get(n.wrapping_sub(1) as usize).filter(|_| n >= 1 && n <= count).map(|c| format!("{n} of {count}: {}x{} {}, {} draws", c.width, c.height, c.colour_format_name(), c.draws))
            };
            let value = match (settings.target, count) {
                (_, 0) => "Not in this game".to_owned(),
                (0, _) => format!("<  Auto ({})  >", describe(status.target_shared).unwrap_or_else(|| "none yet".to_owned())),
                (n, _) => format!("<  {}  >", describe(n).unwrap_or_else(|| format!("{n} (gone)"))),
            };
            ("Buffer".to_owned(), value)
        }
        Item::DepthBuffer => {
            let count = status.depth_buffer_count;
            let describe = |n: u32| {
                status.depth_buffers.get(n.wrapping_sub(1) as usize).filter(|_| n >= 1 && n <= count).map(|c| format!("{n} of {count}: {}x{} {}, {} draws", c.width, c.height, c.format_name(), c.draws))
            };
            let value = match (settings.depth_buffer, count) {
                (_, 0) => "Not in this game".to_owned(),
                (0, _) => format!("<  Auto ({})  >", describe(status.depth_buffer_shared).unwrap_or_else(|| "none yet".to_owned())),
                (n, _) => format!("<  {}  >", describe(n).unwrap_or_else(|| format!("{n} (gone)"))),
            };
            ("Depth buffer".to_owned(), value)
        }
        Item::Freeze => ("Freeze frame".to_owned(), on_off(settings.frozen)),
        Item::CaptureOverlays => ("Captures include drawings".to_owned(), on_off(settings.capture_overlays)),
        Item::Record => ("Record".to_owned(), on_off(settings.recording)),
        Item::SaveCapture => ("Save capture".to_owned(), "A".to_owned()),
        Item::Panel(panel) => (panel.label().to_owned(), on_off(settings.panels.has(panel))),
        Item::SaveRecording => ("Save the last 30 s".to_owned(), if settings.recording { format!("{:.0} s kept", status.recorded) } else { "Record is off".to_owned() }),
        Item::Page(line) => status.page.rows().get(usize::from(line)).map_or_else(Default::default, |r| (r.label().to_owned(), r.value().to_owned())),
    }
}

/// The game's page: headings, its lines of text, and its buttons (the selected one marked), as
/// many rows as fit above the footer. The first row is the page's summary and stays; the window
/// over the others follows the selected one. The rows shown (first, end) and how many there are.
fn page(ui: &mut egui::Ui, menu: &Menu, status: &Status, row_width: f32, sense: Sense, clicked: &mut Option<Click>) -> (usize, usize, usize) {
    let rows = status.page.rows();
    let step = PAGE_ROW + ui.spacing().item_spacing.y;
    // In a scrolling panel the room is what shows of it.
    let room = ui.available_height().min(ui.clip_rect().height());
    let fit = (((room - FOOTER) / step).floor() as usize).max(2);
    let selected = menu.selected[PAGE_TAB].min(rows.len().saturating_sub(1));
    // The window after the summary: `fit - 1` rows, the selected one in its middle.
    let first = selected.saturating_sub((fit - 1) / 2).max(1).min(rows.len().saturating_sub(fit - 1).max(1));
    let end = (first + fit - 1).min(rows.len());
    let font = FontId::proportional(13.0);
    let shown = rows.iter().enumerate().take(1).chain(rows.iter().enumerate().take(end).skip(first));
    for (line, row) in shown {
        let (rect, response) = ui.allocate_exact_size(Vec2::new(row_width, PAGE_ROW), sense);
        if row.kind() == Kind::Button
            && let Some(press) = pressed(&response)
        {
            *clicked = Some(Click::Line(line, press));
        }
        let painter = ui.painter();
        let colour = match row.shade() {
            Tone::Plain => TEXT,
            Tone::Dim => DIM,
            Tone::Good => ON,
            Tone::Warn => WARN,
        };
        if row.kind() == Kind::Heading {
            painter.text(rect.left_bottom(), Align2::LEFT_BOTTOM, row.label(), FontId::proportional(15.0), ACCENT);
            continue;
        }
        let button = row.kind() == Kind::Button;
        if button && line == menu.selected[PAGE_TAB] {
            painter.rect(rect, 6.0, ROW, Stroke::new(1.0, ACCENT), StrokeKind::Inside);
        } else if button {
            painter.rect_stroke(rect, 6.0, Stroke::new(1.0, DIM), StrokeKind::Inside);
        }
        let inset = Vec2::new(f32::from(ROW_EDGE), 0.0);
        let label = painter.text(rect.left_center() + inset, Align2::LEFT_CENTER, row.label(), font.clone(), if button { TEXT } else { DIM });
        // The value keeps clear of the label: it is cut, never drawn over it.
        let room = rect.right() - inset.x - label.right() - 12.0;
        painter.text(rect.right_center() - inset, Align2::RIGHT_CENTER, fitted(ui, row.value(), &font, room), font.clone(), colour);
    }
    (first, end, rows.len())
}

/// `text` cut to `room` points wide in `font`, ending with an ellipsis when cut.
fn fitted(ui: &egui::Ui, text: &str, font: &FontId, room: f32) -> String {
    let width = |t: &str| ui.painter().layout_no_wrap(t.to_owned(), font.clone(), TEXT).size().x;
    if width(text) <= room {
        return text.to_owned();
    }
    let mut cut: String = text.to_owned();
    while !cut.is_empty() && width(&format!("{cut}...")) > room {
        cut.pop();
    }
    format!("{}...", cut.trim_end())
}
