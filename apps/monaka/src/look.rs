//! Fonts and colours: Segoe UI from Windows' fonts folder (egui's own font when it is missing),
//! and the Win32 window's colours. Light or dark follows Windows' app mode live (eframe reads it),
//! unless `--theme` picks one.

use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily, FontId, TextStyle, Theme};
use std::sync::Arc;

/// Text sizes (points): help lines, the body, a how-to's name, Start VR, the game's name.
pub const SMALL: f32 = 11.5;
pub const BODY: f32 = 13.0;
pub const HEADING: f32 = 14.0;
pub const BIG: f32 = 16.0;
pub const TITLE: f32 = 22.0;

const SEMIBOLD: &str = "semibold";

/// The semibold face (the body face where Segoe UI Semibold is missing).
pub fn semibold() -> FontFamily {
    FontFamily::Name(SEMIBOLD.into())
}

/// Sets the fonts and both themes' colours on `ctx`; `forced` overrides Windows' app mode.
pub fn start(ctx: &egui::Context, forced: Option<bool>) {
    ctx.set_fonts(fonts());
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Small, FontId::proportional(SMALL)),
            (TextStyle::Body, FontId::proportional(BODY)),
            (TextStyle::Button, FontId::proportional(BODY)),
            (TextStyle::Heading, FontId::new(TITLE, semibold())),
            (TextStyle::Monospace, FontId::monospace(BODY)),
        ]
        .into();
        style.spacing.item_spacing = egui::vec2(8.0, 5.0);
        style.spacing.button_padding = egui::vec2(10.0, 4.0);
        style.spacing.interact_size.y = 24.0;
        // Bars that always show, so a tab with more options than fit says so.
        style.spacing.scroll = egui::style::ScrollStyle::solid();
    });
    ctx.style_mut_of(Theme::Dark, |style| {
        let v = &mut style.visuals;
        let (text, grey, background, surface) = (Color32::from_gray(240), Color32::from_gray(160), Color32::from_gray(32), Color32::from_gray(43));
        v.panel_fill = background;
        v.window_fill = surface;
        v.extreme_bg_color = surface;
        v.faint_bg_color = surface;
        v.weak_text_color = Some(grey);
        v.widgets.noninteractive.fg_stroke.color = text;
        v.widgets.inactive.fg_stroke.color = text;
    });
    ctx.style_mut_of(Theme::Light, |style| {
        let v = &mut style.visuals;
        v.panel_fill = Color32::WHITE;
        v.weak_text_color = Some(Color32::from_gray(109));
        v.widgets.noninteractive.fg_stroke.color = Color32::BLACK;
        v.widgets.inactive.fg_stroke.color = Color32::BLACK;
    });
    if let Some(dark) = forced {
        ctx.set_theme(if dark { Theme::Dark } else { Theme::Light });
    }
}

/// The games column's and the log's background: a step darker than the page in either theme.
pub fn side(visuals: &egui::Visuals) -> Color32 {
    if visuals.dark_mode { Color32::from_gray(26) } else { Color32::from_gray(243) }
}

/// egui's fonts with Segoe UI (and its semibold) first, as Windows' own windows draw.
fn fonts() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    let folder = std::path::PathBuf::from(std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into())).join("Fonts");
    let mut add = |name: &str, file: &str| -> bool {
        let Ok(bytes) = std::fs::read(folder.join(file)) else { return false };
        fonts.font_data.insert(name.to_owned(), Arc::new(FontData::from_owned(bytes)));
        true
    };
    let regular = add("Segoe UI", "segoeui.ttf");
    let semibold_face = add("Segoe UI Semibold", "seguisb.ttf");
    let proportional = fonts.families.entry(FontFamily::Proportional).or_default();
    if regular {
        proportional.insert(0, "Segoe UI".to_owned());
    }
    let mut bold = proportional.clone();
    if semibold_face {
        bold.insert(0, "Segoe UI Semibold".to_owned());
    }
    fonts.families.insert(semibold(), bold);
    fonts
}

