//! Abstract home backgrounds, made rather than captured: a room ([`room`]), a dome round the player
//! with a floor below, whose colours a small pass paints each frame into a cube map strip
//! ([`PatternPass`], `pattern.hlsl`), one pixel per texel of the room. The scene pass then draws it
//! as it does a captured room: tiles at their distances, hard pixels, the Bayer dither, the
//! player's turns. Painting texels, not the eyes' pixels, keeps it cheap: a few hundred thousand
//! small sums a frame, however big the headset's images.

use crate::Result;
use crate::gpu::{self, Constants, failed};
use crate::scene::files::{self, Loaded, Place};
use monaka_channel::d3d::{create_texture, simple_texture_desc};
use windows::Win32::Graphics::Direct3D::D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::s;

const SOURCE: &str = include_str!("pattern.hlsl");

/// Pixels a face: the captured rooms' grain.
pub const FACE: u32 = 340;
/// The dome's radius and the floor's height below the head (metres; `pattern.hlsl`).
const DOME: f32 = 14.0;
const FLOOR: f32 = -1.6;

/// The patterns on offer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pattern {
    /// Soft colours drifting round the dome, the floor a dim mirror of them.
    Drift,
}

impl Pattern {
    pub fn name(self) -> &'static str {
        match self {
            Pattern::Drift => "Drift",
        }
    }
}

/// How far from the middle the room's surface is in direction `dir` (of unit length).
fn distance(dir: [f32; 3]) -> f32 {
    if dir[1] < 0.0 {
        let t = FLOOR / dir[1];
        let (x, z) = (dir[0] * t, dir[2] * t);
        if x * x + z * z < DOME * DOME - FLOOR * FLOOR {
            return t;
        }
    }
    DOME
}

/// The patterns' room, `n` pixels a face, as a captured room would be read: one capture point in
/// the middle, its depth the dome and the floor; its colours come from [`PatternPass`].
pub fn room(pattern: Pattern, n: u32) -> Loaded {
    let size = n as usize;
    let width = size * 6;
    let depth: Vec<f32> = (0..width * size)
        .map(|pixel| {
            let (x, y) = (pixel % width, pixel / width);
            let dir = files::direction(n, x / size, (x % size) as f32 + 0.5, y as f32 + 0.5);
            let length = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
            distance(dir.map(|c| c / length))
        })
        .collect();
    let place = Place { name: "c", offset: [0.0; 3], corners: files::corners(&depth, n), tiles: (0..depth.len() as u32).collect(), skirts: files::skirts(&depth, n) };
    Loaded {
        name: pattern.name().to_owned(),
        face: n,
        shades: files::SHADES,
        dither: files::DITHER,
        times: vec![0],
        sides: [DOME; 2],
        places: vec![place],
        colours: Vec::new(),
    }
}

/// The `Pattern` constant buffer of `pattern.hlsl`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct PatternConstants {
    grid: [u32; 4],
    time: [f32; 4],
}

/// Paints a pattern's colours into a strip the scene pass reads as its hour.
pub struct PatternPass {
    vertex: ID3D11VertexShader,
    pixel: ID3D11PixelShader,
    constants: Constants<PatternConstants>,
    rasterizer: ID3D11RasterizerState,
    target: ID3D11RenderTargetView,
    view: ID3D11ShaderResourceView,
    n: u32,
}

impl PatternPass {
    /// A pass painting strips `n` pixels a face, 16 bits a channel (the dither has fine gradients
    /// to work from).
    pub fn new(device: &ID3D11Device, n: u32) -> Result<Self> {
        let mut desc = simple_texture_desc(n * 6, n, DXGI_FORMAT_R16G16B16A16_UNORM);
        desc.BindFlags = (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32;
        let texture = create_texture(device, &desc).map_err(|e| format!("pattern image: {e}"))?;
        let (mut target, mut view) = (None, None);
        // SAFETY: views of our own live texture, in its own format.
        unsafe {
            device.CreateRenderTargetView(&texture, None, Some(&mut target)).map_err(failed("pattern target"))?;
            device.CreateShaderResourceView(&texture, None, Some(&mut view)).map_err(failed("pattern view"))?;
        }
        Ok(Self {
            vertex: gpu::vertex_shader(device, SOURCE, "pattern", s!("vs_main"))?,
            pixel: gpu::pixel_shader(device, SOURCE, "pattern", s!("ps_main"))?,
            constants: Constants::new(device)?,
            rasterizer: gpu::rasterizer(device, false)?,
            target: target.ok_or("no pattern target")?,
            view: view.ok_or("no pattern view")?,
            n,
        })
    }

    /// The strip, for the scene pass ([`crate::scene::ScenePass::add_time_views`]).
    pub fn view(&self) -> ID3D11ShaderResourceView {
        self.view.clone()
    }

    /// Paints `pattern` as it is `seconds` in.
    pub fn draw(&self, context: &ID3D11DeviceContext, pattern: Pattern, seconds: f64) {
        // A day's seconds at most: the drift's sums stay precise in 32 bits.
        let time = (seconds % 86_400.0) as f32;
        let mode = match pattern {
            Pattern::Drift => 0,
        };
        self.constants.write(context, &PatternConstants { grid: [self.n, mode, 0, 0], time: [time, 0.0, 0.0, 0.0] });
        gpu::bind_target(context, &self.target, self.n * 6, self.n);
        // SAFETY: our own live shaders, view and states, bound for one draw on the viewer's
        // immediate context; the target is unbound after, so the scene pass can read it.
        unsafe {
            context.IASetInputLayout(None);
            context.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            context.VSSetShader(&self.vertex, None);
            context.PSSetShader(&self.pixel, None);
            context.PSSetConstantBuffers(0, Some(&[Some(self.constants.buffer.clone())]));
            context.RSSetState(&self.rasterizer);
            context.OMSetBlendState(None, None, u32::MAX);
            context.OMSetDepthStencilState(None, 0);
            context.Draw(3, 0);
        }
        gpu::unbind_target(context);
    }
}
