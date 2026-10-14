//! Debug lines over an eye image ([`monaka_core::debug_draw::Primitive`]s in tracking space), drawn
//! with the eye's own pose and field of view into the image itself, a few pixels wide whatever
//! their distance, over everything (no depth test). The image's alpha is left as it was.

use crate::Result;
use crate::eye::EyeView;
use crate::gpu::{self, Constants, Structured, Targets};
use monaka_core::debug_draw::Primitive;
use windows::Win32::Graphics::Direct3D::D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;
use windows::core::s;

#[cfg(test)]
mod tests;

const SOURCE: &str = include_str!("lines.hlsl");
/// Nothing nearer the eye than this is drawn (metres).
const NEAR: f32 = 0.02;

/// The `Eye` constant buffer of `lines.hlsl`.
#[repr(C)]
#[derive(Clone, Copy)]
struct EyeConstants {
    view: [[f32; 4]; 3],
    tangents: [f32; 4],
    size: [f32; 2],
    near: f32,
    half_width: f32,
}

pub struct LinePass {
    vertex: ID3D11VertexShader,
    pixel: ID3D11PixelShader,
    constants: Constants<EyeConstants>,
    primitives: Structured<Primitive>,
    blend: ID3D11BlendState,
    rasterizer: ID3D11RasterizerState,
    targets: Targets,
}

impl LinePass {
    pub fn new(device: &ID3D11Device) -> Result<Self> {
        Ok(Self {
            vertex: gpu::vertex_shader(device, SOURCE, "lines", s!("vs_main"))?,
            pixel: gpu::pixel_shader(device, SOURCE, "lines", s!("ps_main"))?,
            constants: Constants::new(device)?,
            primitives: Structured::new(),
            blend: gpu::premultiplied(device, true)?,
            rasterizer: gpu::rasterizer(device, false)?,
            targets: Targets::default(),
        })
    }

    /// Draws `primitives` over `image` (an eye's swapchain image, made in `format`) as seen by
    /// `eye`, `width` pixels wide.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(&mut self, device: &ID3D11Device, context: &ID3D11DeviceContext, image: &ID3D11Texture2D, format: DXGI_FORMAT, eye: &EyeView, primitives: &[Primitive], width: f32) -> Result<()> {
        if primitives.is_empty() {
            return Ok(());
        }
        let desc = monaka_channel::d3d::texture_desc(image);
        let target = self.targets.view(device, image, format)?;
        let view = self.primitives.write(device, context, primitives)?;
        self.constants.write(context, &EyeConstants { view: eye.view_rows(), tangents: eye.tangents(), size: [desc.Width as f32, desc.Height as f32], near: NEAR, half_width: width * 0.5 });
        gpu::bind_target(context, &target, desc.Width, desc.Height);
        // SAFETY: our own live shaders, buffers and states, bound for one draw on the viewer's
        // immediate context, which nothing else draws with.
        unsafe {
            context.IASetInputLayout(None);
            context.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            context.VSSetShader(&self.vertex, None);
            context.VSSetConstantBuffers(0, Some(&[Some(self.constants.buffer.clone())]));
            context.VSSetShaderResources(0, Some(&[Some(view)]));
            context.PSSetShader(&self.pixel, None);
            context.RSSetState(&self.rasterizer);
            context.OMSetBlendState(&self.blend, None, u32::MAX);
            context.OMSetDepthStencilState(None, 0);
            context.DrawInstanced(6, primitives.len() as u32, 0, 0);
            context.VSSetShaderResources(0, Some(&[None]));
        }
        gpu::unbind_target(context);
        Ok(())
    }
}
