//! A page on a ring of panels in the headset ([`crate::home::place`]), the same for Monaka's home
//! and the debug menu: the look (type and targets large enough at a metre or so) and the frame (the
//! title, a body of fixed height, the keys at the foot, outlined while in front).

use crate::menu::panel::{ACCENT, BACKGROUND, DIM, ROW, TEXT};
use egui::{Align, Color32, CornerRadius, FontId, Frame, Layout, Margin, RichText, Stroke, TextStyle, Vec2};

/// Type and targets large enough at a metre or so.
pub fn style(ui: &mut egui::Ui) {
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

/// The panel's frame: the `title`, `body` in the room left above the footer (clipped to it), and
/// the `keys` at the foot; outlined while it is in `front`. All of it inside the image, whatever
/// `body` holds.
pub fn framed<R>(ui: &mut egui::Ui, title: &str, front: bool, keys: &str, body: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let inside = ui.available_size() - Vec2::splat(2.0 * (EDGE as f32 + OUTLINE));
    let outline = Stroke::new(OUTLINE, if front { ACCENT } else { Color32::TRANSPARENT });
    Frame::new()
        .fill(BACKGROUND)
        .stroke(outline)
        .corner_radius(CornerRadius::same(14))
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
            ui.label(RichText::new(keys).size(11.0).color(DIM));
            inner
        })
        .inner
}
