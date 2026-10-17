//! An egui page drawn into a texture of its own, for a panel the headset shows ([`crate::menu`],
//! [`crate::settings`]).

use crate::Result;
use crate::gpu::failed;
use crate::menu::painter::Painter;
use egui::{Pos2, Vec2};
use monaka_channel::d3d::{create_texture, simple_texture_desc};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;

/// Pixels per egui point on every page.
pub const PIXELS_PER_POINT: f32 = 2.0;

pub struct Canvas {
    context: egui::Context,
    painter: Option<Painter>,
    format: DXGI_FORMAT,
    pixels: [u32; 2],
    target: Option<(ID3D11Texture2D, ID3D11RenderTargetView)>,
}

impl Canvas {
    /// A canvas of `pixels` drawn in `format` (the headset's image format).
    pub fn new(format: DXGI_FORMAT, pixels: [u32; 2]) -> Self {
        let context = egui::Context::default();
        context.all_styles_mut(|style| style.animation_time = 0.0);
        Self { context, painter: None, format, pixels, target: None }
    }

    /// The latest image, once drawn.
    pub fn image(&self) -> Option<&ID3D11Texture2D> {
        self.target.as_ref().map(|(texture, _)| texture)
    }

    /// The page's size in egui points (its pixels at [`PIXELS_PER_POINT`]).
    pub fn points(&self) -> Vec2 {
        Vec2::new(self.pixels[0] as f32 / PIXELS_PER_POINT, self.pixels[1] as f32 / PIXELS_PER_POINT)
    }

    /// Draws `layout` into the image; the image, when it was drawn.
    pub fn draw(&mut self, device: &ID3D11Device, context: &ID3D11DeviceContext, layout: impl FnMut(&mut egui::Ui)) -> Result<Option<&ID3D11Texture2D>> {
        self.draw_with(device, context, Vec::new(), layout)
    }

    /// [`Canvas::draw`] with what a pointer did on the page since the last draw (`events`, in
    /// points): egui keeps what it means (hover, a click, a scrolled list) between draws.
    pub fn draw_with(&mut self, device: &ID3D11Device, context: &ID3D11DeviceContext, events: Vec<egui::Event>, layout: impl FnMut(&mut egui::Ui)) -> Result<Option<&ID3D11Texture2D>> {
        if self.painter.is_none() {
            self.painter = Some(Painter::new(device)?);
        }
        if self.target.is_none() {
            let mut desc = simple_texture_desc(self.pixels[0], self.pixels[1], self.format);
            desc.BindFlags = (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32;
            let texture = create_texture(device, &desc).map_err(|e| format!("panel image: {e}"))?;
            let mut view = None;
            // SAFETY: a view of our own texture in its own format.
            unsafe { device.CreateRenderTargetView(&texture, None, Some(&mut view)) }.map_err(failed("panel image view"))?;
            self.target = Some((texture, view.ok_or("no panel image view")?));
        }
        let points = self.points();
        let mut input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, points)), max_texture_side: Some(4096), events, ..Default::default() };
        input.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(PIXELS_PER_POINT);
        let mut output = self.context.run_ui(input, layout);
        let primitives = self.context.tessellate(std::mem::take(&mut output.shapes), output.pixels_per_point);
        let (Some(painter), Some((texture, view))) = (self.painter.as_mut(), self.target.as_ref()) else {
            output.textures_delta.clear();
            return Ok(None);
        };
        // SAFETY: clears our own render target.
        unsafe { context.ClearRenderTargetView(view, &[0.0; 4]) };
        painter.paint(device, context, view, self.pixels, output.pixels_per_point, is_srgb(self.format), &primitives, &mut output.textures_delta)?;
        Ok(Some(texture))
    }
}

/// Where the pointer is on a page, kept from its events ([`Cursor::follow`]) and painted over the
/// page ([`Cursor::paint`]).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Cursor(Option<Pos2>);

impl Cursor {
    /// Follows `events`: moved, or gone.
    pub fn follow(&mut self, events: &[egui::Event]) {
        for event in events {
            match event {
                egui::Event::PointerMoved(at) => self.0 = Some(*at),
                egui::Event::PointerGone => self.0 = None,
                _ => {}
            }
        }
    }

    /// A dot where the pointer is, over everything else on the page.
    pub fn paint(&self, ctx: &egui::Context, colour: egui::Color32) {
        if let Some(at) = self.0 {
            let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("pointer cursor")));
            painter.circle_filled(at, 5.0, colour);
            painter.circle_stroke(at, 9.0, egui::Stroke::new(2.0, egui::Color32::from_black_alpha(160)));
        }
    }
}

fn is_srgb(format: DXGI_FORMAT) -> bool {
    matches!(format, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB)
}
