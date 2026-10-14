//! An eye shown another way ([`crate::menu::View`]): drawn over its swapchain image from the eye's
//! own image, the other eye's and its depth, instead of the plain copy. The eye's alpha is kept.

use crate::Result;
use crate::gpu::{self, Constants, Targets, failed};
use crate::menu::View;
use monaka_channel::d3d::texture_desc;
use std::collections::HashMap;
use windows::Win32::Graphics::Direct3D::{D3D_SRV_DIMENSION_TEXTURE2D, D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::{Interface, s};

#[cfg(test)]
mod tests;

const SOURCE: &str = include_str!("views.hlsl");

/// An eye's depth: its texture (typeless R32 or R16), where the eye lies in it (left, top, width,
/// height in texels) and the distances at depth 0 and 1 (metres; either may be infinite).
#[derive(Clone, Copy)]
pub struct Depth<'a> {
    pub texture: &'a ID3D11Texture2D,
    pub rect: [u32; 4],
    pub distances: (f32, f32),
}

/// What an eye's view reads.
#[derive(Clone, Copy)]
pub struct Eye<'a> {
    pub image: &'a ID3D11Texture2D,
    pub other: &'a ID3D11Texture2D,
    pub depth: Option<Depth<'a>>,
    /// Presents this eye's frame is older than the other's.
    pub age: u32,
    /// The distance (metres) the depth view goes black at.
    pub range: f32,
    /// The depth view's ramp in even steps per doubling of distance.
    pub logarithmic: bool,
    /// One of the game's render targets, as stored, for the Game buffer view.
    pub buffer: Option<&'a ID3D11Texture2D>,
}

/// The `View` constant buffer of `views.hlsl`.
#[repr(C)]
#[derive(Clone, Copy)]
struct ViewConstants {
    mode: u32,
    age: u32,
    encoding: u32,
    range: f32,
    depth_rect: [f32; 4],
    inverse_at_0: f32,
    inverse_at_1: f32,
    depth_step: [f32; 2],
    logarithmic: u32,
    unused: [u32; 3],
}

pub struct ViewPass {
    vertex: ID3D11VertexShader,
    pixel: ID3D11PixelShader,
    constants: Constants<ViewConstants>,
    sampler: ID3D11SamplerState,
    rasterizer: ID3D11RasterizerState,
    targets: Targets,
    /// Shader views of the textures read, by texture.
    reads: HashMap<usize, ID3D11ShaderResourceView>,
}

impl ViewPass {
    pub fn new(device: &ID3D11Device) -> Result<Self> {
        let desc = D3D11_SAMPLER_DESC {
            Filter: D3D11_FILTER_MIN_MAG_MIP_POINT,
            AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
            MaxLOD: f32::MAX,
            ..Default::default()
        };
        let mut sampler = None;
        // SAFETY: valid descriptor and out-pointer.
        unsafe { device.CreateSamplerState(&desc, Some(&mut sampler)) }.map_err(failed("view sampler"))?;
        Ok(Self {
            vertex: gpu::vertex_shader(device, SOURCE, "views", s!("vs_main"))?,
            pixel: gpu::pixel_shader(device, SOURCE, "views", s!("ps_main"))?,
            constants: Constants::new(device)?,
            sampler: sampler.ok_or("no view sampler")?,
            rasterizer: gpu::rasterizer(device, false)?,
            targets: Targets::default(),
            reads: HashMap::new(),
        })
    }

    /// Draws `eye` as `view` over `image` (its swapchain image, made in `format`). False, drawing
    /// nothing, when the view cannot be drawn (a depth view without depth) or is the plain picture.
    pub fn draw(&mut self, device: &ID3D11Device, context: &ID3D11DeviceContext, image: &ID3D11Texture2D, format: DXGI_FORMAT, view: View, eye: &Eye<'_>) -> Result<bool> {
        let mode = match view {
            View::Picture => return Ok(false),
            View::Depth => 1,
            View::DepthEdges => 2,
            View::FrameAge => 3,
            View::Difference => 4,
            View::GameBuffer => 5,
        };
        if (view.needs_depth() && eye.depth.is_none()) || (view == View::GameBuffer && eye.buffer.is_none()) {
            return Ok(false);
        }
        let desc = texture_desc(image);
        let target = self.targets.view(device, image, format)?;
        let target_srgb = is_srgb(self.targets.format_of(image).unwrap_or(format));
        let source_srgb = is_srgb(texture_desc(eye.image).Format);
        let encoding = match (source_srgb, target_srgb) {
            (false, true) => 1,
            (true, false) => 2,
            _ => 0,
        };
        let mut constants = ViewConstants { mode, age: eye.age, encoding, range: eye.range.max(0.1), depth_rect: [0.0, 0.0, 1.0, 1.0], inverse_at_0: 0.0, inverse_at_1: 0.0, depth_step: [0.0; 2], logarithmic: u32::from(eye.logarithmic), unused: [0; 3] };
        let depth_view = match eye.depth {
            Some(depth) => {
                let size = texture_desc(depth.texture);
                let [x, y, w, h] = depth.rect.map(|v| v as f32);
                constants.depth_rect = [x / size.Width as f32, y / size.Height as f32, w / size.Width as f32, h / size.Height as f32];
                constants.depth_step = [1.0 / w.max(1.0), 1.0 / h.max(1.0)];
                let inverse = |d: f32| if d.is_finite() && d > 0.0 { 1.0 / d } else { 0.0 };
                (constants.inverse_at_0, constants.inverse_at_1) = (inverse(depth.distances.0), inverse(depth.distances.1));
                Some(self.read(device, depth.texture, depth_read_format(size.Format))?)
            }
            None => None,
        };
        let buffer_view = match eye.buffer {
            Some(buffer) => Some(self.read(device, buffer, DXGI_FORMAT_UNKNOWN)?),
            None => None,
        };
        let reads = [Some(self.read(device, eye.image, DXGI_FORMAT_UNKNOWN)?), Some(self.read(device, eye.other, DXGI_FORMAT_UNKNOWN)?), depth_view, buffer_view];
        self.constants.write(context, &constants);
        gpu::bind_target(context, &target, desc.Width, desc.Height);
        // SAFETY: our own live shaders, views and states, bound for one draw on the viewer's
        // immediate context, which nothing else draws with; the views are unbound after.
        unsafe {
            context.IASetInputLayout(None);
            context.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            context.VSSetShader(&self.vertex, None);
            context.PSSetShader(&self.pixel, None);
            context.PSSetConstantBuffers(0, Some(&[Some(self.constants.buffer.clone())]));
            context.PSSetShaderResources(0, Some(&reads));
            context.PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
            context.RSSetState(&self.rasterizer);
            context.OMSetBlendState(None, None, u32::MAX);
            context.OMSetDepthStencilState(None, 0);
            context.Draw(3, 0);
            context.PSSetShaderResources(0, Some(&[None, None, None, None]));
        }
        gpu::unbind_target(context);
        Ok(true)
    }

    /// A shader view of `texture`, as `format` (unknown: its own).
    fn read(&mut self, device: &ID3D11Device, texture: &ID3D11Texture2D, format: DXGI_FORMAT) -> Result<ID3D11ShaderResourceView> {
        let key = texture.as_raw() as usize;
        if let Some(view) = self.reads.get(&key) {
            return Ok(view.clone());
        }
        let desc = D3D11_SHADER_RESOURCE_VIEW_DESC {
            Format: format,
            ViewDimension: D3D_SRV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_SRV { MostDetailedMip: 0, MipLevels: 1 } },
        };
        let mut view = None;
        // SAFETY: a view of a live texture with shader-resource binding, in a format of its family.
        unsafe { device.CreateShaderResourceView(texture, (format != DXGI_FORMAT_UNKNOWN).then_some(&desc as *const _), Some(&mut view)) }.map_err(failed("view read"))?;
        let view = view.ok_or("no view read")?;
        // Depth textures are made again when their size changes: views of ones gone are let go.
        if self.reads.len() >= 16 {
            self.reads.clear();
        }
        self.reads.insert(key, view.clone());
        Ok(view)
    }
}

fn is_srgb(format: DXGI_FORMAT) -> bool {
    matches!(format, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB)
}

/// The format a depth texture's depth is read as.
fn depth_read_format(format: DXGI_FORMAT) -> DXGI_FORMAT {
    match format {
        DXGI_FORMAT_R16_TYPELESS | DXGI_FORMAT_D16_UNORM | DXGI_FORMAT_R16_UNORM => DXGI_FORMAT_R16_UNORM,
        _ => DXGI_FORMAT_R32_FLOAT,
    }
}
