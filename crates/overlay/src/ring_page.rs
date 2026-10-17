//! A page on a ring of panels in the headset ([`crate::home::place`]), the same for Monaka's home
//! and the debug menu: the look (type and targets large enough at a metre or so, and the colours,
//! [`Look`]) and the frame (the title, a body of fixed height, the keys at the foot, outlined while
//! in front).

use crate::menu::panel::{ACCENT, BACKGROUND, DIM, ON, ROW, TEXT};
use egui::{Align, Color32, CornerRadius, FontId, Frame, Layout, Margin, RichText, Stroke, TextStyle, Vec2};

/// A ring's colours.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    /// The panel behind everything (premultiplied: below opaque, the headset shows through).
    pub background: Color32,
    /// A setting's box.
    pub card: Color32,
    pub button: Color32,
    /// What is chosen: a tab, the game in the list, the bar of settings waiting.
    pub row: Color32,
    pub text: Color32,
    pub dim: Color32,
    /// Separators and the scroll bar's track.
    pub line: Color32,
    /// The outline of the panel in front, the cursor, a hovered target, a problem.
    pub accent: Color32,
    /// A game playing, a setting that applies now.
    pub on: Color32,
    pub stop: Color32,
    pub apply: Color32,
}

/// The debug menu's (egui's dark buttons and lines, the old blue and orange).
pub const DEBUG: Look = Look {
    background: BACKGROUND,
    card: Color32::from_rgba_premultiplied(6, 6, 6, 6),
    button: Color32::from_gray(60),
    row: ROW,
    text: TEXT,
    dim: DIM,
    line: Color32::from_gray(60),
    accent: ACCENT,
    on: ON,
    stop: Color32::from_rgb(120, 40, 36),
    apply: Color32::from_rgb(70, 110, 60),
};

/// Monaka's home: oxblood.
pub const HOME: Look = Look {
    background: Color32::from_rgba_premultiplied(23, 14, 16, 235),
    card: Color32::from_rgba_premultiplied(10, 10, 10, 10),
    button: Color32::from_rgb(59, 38, 41),
    row: Color32::from_rgb(110, 31, 43),
    text: Color32::from_rgb(244, 231, 232),
    dim: Color32::from_rgb(179, 150, 153),
    line: Color32::from_rgb(61, 42, 45),
    accent: Color32::from_rgb(255, 106, 92),
    on: Color32::from_rgb(143, 212, 160),
    stop: Color32::from_rgb(184, 48, 42),
    apply: Color32::from_rgb(74, 125, 79),
};

/// How much of the panel's colour a frosted panel keeps over the blurred game behind it.
const GLASS: u8 = 190;

impl Look {
    /// The look as frosted glass ([`crate::frost`] blurs what is behind): the panel and its buttons
    /// let the game through, and the dim text is lighter to stay readable over it.
    pub fn frosted(self) -> Self {
        let see_through = |c: Color32, alpha: u8| Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), alpha);
        let [r, g, b, a] = self.background.to_array();
        let solid = Color32::from_rgb(unmultiply(r, a), unmultiply(g, a), unmultiply(b, a));
        Self {
            background: see_through(solid, GLASS),
            button: see_through(self.button, 230),
            line: Color32::from_white_alpha(30),
            dim: Color32::from_rgb(196, 168, 171),
            ..self
        }
    }
}

fn unmultiply(channel: u8, alpha: u8) -> u8 {
    if alpha == 0 { 0 } else { (channel as u32 * 255 / alpha as u32).min(255) as u8 }
}

/// Type and targets large enough at a metre or so, in `look`'s colours.
pub fn style(ui: &mut egui::Ui, look: &Look) {
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
    let v = &mut style.visuals;
    v.override_text_color = Some(look.text);
    v.selection.bg_fill = look.row;
    v.widgets.hovered.bg_stroke = Stroke::new(1.5, look.accent);
    v.widgets.noninteractive.bg_stroke.color = look.line;
    v.widgets.inactive.weak_bg_fill = look.button;
    v.widgets.inactive.bg_fill = look.button;
    v.widgets.hovered.weak_bg_fill = shade(look.button, 10);
    v.widgets.hovered.bg_fill = shade(look.button, 10);
    v.widgets.active.weak_bg_fill = shade(look.button, -5);
    v.widgets.active.bg_fill = shade(look.button, -5);
}

/// `colour` lighter (`by` > 0) or darker, its alpha kept.
fn shade(colour: Color32, by: i16) -> Color32 {
    let [r, g, b, a] = colour.to_array();
    let step = |c: u8| (c as i16 + by * a as i16 / 255).clamp(0, a as i16) as u8;
    Color32::from_rgba_premultiplied(step(r), step(g), step(b), a)
}

/// Space inside a panel's edge, the outline round the one in front, and the footer's height
/// (points).
const EDGE: i8 = 16;
const OUTLINE: f32 = 3.0;
const FOOTER: f32 = 22.0;
/// The panel's corners (points); [`crate::frost`] rounds the blur behind it the same.
pub const CORNER: u8 = 14;

/// The panel's frame in `look`: the `title`, `body` in the room left above the footer (clipped to
/// it), and the `keys` at the foot; outlined while it is in `front`. All of it inside the image,
/// whatever `body` holds.
pub fn framed<R>(ui: &mut egui::Ui, look: &Look, title: &str, front: bool, keys: &str, body: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let inside = ui.available_size() - Vec2::splat(2.0 * (EDGE as f32 + OUTLINE));
    let outline = Stroke::new(OUTLINE, if front { look.accent } else { Color32::TRANSPARENT });
    Frame::new()
        .fill(look.background)
        .stroke(outline)
        .corner_radius(CornerRadius::same(CORNER))
        .inner_margin(Margin::same(EDGE))
        .show(ui, |ui| {
            ui.set_min_size(inside);
            ui.set_max_size(inside);
            ui.label(RichText::new(title).size(22.0).strong());
            ui.add_space(4.0);
            let room = Vec2::new(inside.x, (ui.available_height() - FOOTER).max(0.0));
            let inner = ui
                .allocate_ui_with_layout(room, Layout::top_down(Align::Min), |ui| {
                    ui.set_min_size(room);
                    ui.set_max_size(room);
                    ui.set_clip_rect(ui.max_rect().intersect(ui.clip_rect()));
                    body(ui)
                })
                .inner;
            ui.add_space(4.0);
            ui.label(RichText::new(keys).size(11.0).color(look.dim));
            inner
        })
        .inner
}

#[cfg(test)]
mod tests;
