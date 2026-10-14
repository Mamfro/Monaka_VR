//! The Monaka menu drawn with egui into a texture of its own ([`Canvas`]), again only when what it
//! shows changed; in the debug menu's colours.

use super::{SettingsMenu, shown};
use crate::Result;
use crate::canvas::Canvas;
use crate::menu::panel::{ACCENT, BACKGROUND, DIM, ON, ROW, TEXT};
use egui::{Color32, CornerRadius, Frame, Margin, RichText, Stroke, Vec2};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;

/// The panel's image size (pixels).
pub const PIXELS: [u32; 2] = [1024, 640];

pub struct SettingsPanel {
    canvas: Canvas,
    shown: Option<SettingsMenu>,
}

impl SettingsPanel {
    /// A panel drawn in `format` (the headset's image format).
    pub fn new(format: DXGI_FORMAT) -> Self {
        Self { canvas: Canvas::new(format, PIXELS), shown: None }
    }

    /// The latest image, once drawn.
    pub fn image(&self) -> Option<&ID3D11Texture2D> {
        self.canvas.image()
    }

    /// Forgets what was drawn: the next [`SettingsPanel::draw`] draws (its image went somewhere new).
    pub fn forget(&mut self) {
        self.shown = None;
    }

    /// Draws the menu when it shows something new; the image when it was drawn.
    pub fn draw(&mut self, device: &ID3D11Device, context: &ID3D11DeviceContext, menu: &SettingsMenu) -> Result<Option<&ID3D11Texture2D>> {
        if self.shown.as_ref() == Some(menu) && self.canvas.image().is_some() {
            return Ok(None);
        }
        let image = self.canvas.draw(device, context, |ui| layout(ui, menu))?;
        if image.is_some() {
            self.shown = Some(menu.clone());
        }
        Ok(image)
    }
}

/// Space inside the panel's edge, and inside a row's (points).
const EDGE: i8 = 18;
const ROW_EDGE: i8 = 10;

fn layout(ui: &mut egui::Ui, menu: &SettingsMenu) {
    let inside = ui.available_size() - Vec2::splat(2.0 * EDGE as f32);
    Frame::new().fill(BACKGROUND).corner_radius(CornerRadius::same(14)).inner_margin(Margin::same(EDGE)).show(ui, |ui| {
        ui.set_min_size(inside);
        ui.set_max_size(inside);
        let row_width = inside.x - 2.0 * ROW_EDGE as f32 - 2.0;
        ui.label(RichText::new("Monaka").size(20.0).color(TEXT).strong());
        ui.add_space(8.0);
        for (index, line) in menu.lines.iter().enumerate() {
            let selected = index == menu.selected;
            let value = shown(line.opt, &line.value);
            Frame::new()
                .fill(if selected { ROW } else { Color32::TRANSPARENT })
                .stroke(if selected { Stroke::new(1.0, ACCENT) } else { Stroke::NONE })
                .corner_radius(CornerRadius::same(6))
                .inner_margin(Margin::symmetric(ROW_EDGE, 4))
                .show(ui, |ui| {
                    ui.set_width(row_width);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(line.opt.label).size(15.0).color(TEXT));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(RichText::new(value.as_str()).size(15.0).color(if value == "On" { ON } else { TEXT }));
                        });
                    });
                });
        }
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            ui.label(RichText::new("Stick: move, change   B: close").size(12.0).color(DIM));
            if let Some(line) = menu.lines.get(menu.selected).filter(|l| !l.opt.help.is_empty()) {
                ui.label(RichText::new(line.opt.help).size(13.0).color(DIM));
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use monaka_channel::d3d::{create_device, read_image};
    use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM;

    #[test]
    fn the_menu_draws_and_redraws_only_on_change() {
        let (device, context, _) = create_device(None).unwrap();
        let mut panel = SettingsPanel::new(DXGI_FORMAT_R8G8B8A8_UNORM);
        let mut menu = SettingsMenu::parse("controller_smoothing=3,menu_reach=same,menu_reach_depth=15,controller_proxy=off");
        menu.open = true;
        let image = panel.draw(&device, &context, &menu).unwrap().expect("drawn").clone();
        assert!(panel.draw(&device, &context, &menu).unwrap().is_none(), "nothing changed");
        menu.selected = 1;
        assert!(panel.draw(&device, &context, &menu).unwrap().is_some(), "another line picked");
        let (width, height, pixels) = read_image(&device, &context, &image).unwrap();
        assert_eq!([width, height], PIXELS);
        let at = |x: u32, y: u32| pixels[((y * width + x) * 4 + 3) as usize];
        assert!(at(width / 2, height / 2) > 200, "the panel's background is nearly opaque");
        if let Some(path) = std::env::var_os("MONAKA_SETTINGS_PNG") {
            std::fs::write(path, monaka_core::png::rgba(width, height, &pixels)).unwrap();
        }
    }
}
