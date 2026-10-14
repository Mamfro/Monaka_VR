//! A picture (premultiplied) on a flat rectangle in tracking space drawn into an eye's image as
//! that eye saw it: a layer the compositor shows on its own (the debug menu) put into a capture.

use crate::Result;
use crate::eye::EyeView;
use crate::gpu::{self, Constants, Targets, failed};
use monaka_core::math::{Quat, Vec3, rotate};
use windows::Win32::Graphics::Direct3D::D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;
use windows::core::s;

const SOURCE: &str = include_str!("quad.hlsl");

/// A rectangle facing along its pose's +Z (its front seen from there), `size` metres.
#[derive(Clone, Copy, Debug)]
pub struct Rectangle {
    pub orientation: Quat,
    pub position: Vec3,
    pub size: [f32; 2],
}

impl Rectangle {
    /// Its corners: top left, top right, bottom left, bottom right.
    pub fn corners(&self) -> [Vec3; 4] {
        let [w, h] = self.size.map(|s| s * 0.5);
        [[-w, h], [w, h], [-w, -h], [w, -h]].map(|[x, y]| {
            let at = rotate(self.orientation, [x, y, 0.0]);
            [0, 1, 2].map(|k| self.position[k] + at[k])
        })
    }
}

/// The `Quad` constant buffer of `quad.hlsl`.
#[repr(C)]
#[derive(Clone, Copy)]
struct QuadConstants {
    view: [[f32; 4]; 3],
    tangents: [f32; 4],
    corners: [[f32; 4]; 4],
}

pub struct QuadPass {
    vertex: ID3D11VertexShader,
    pixel: ID3D11PixelShader,
    constants: Constants<QuadConstants>,
    sampler: ID3D11SamplerState,
    blend: ID3D11BlendState,
    rasterizer: ID3D11RasterizerState,
    targets: Targets,
}

impl QuadPass {
    pub fn new(device: &ID3D11Device) -> Result<Self> {
        let desc = D3D11_SAMPLER_DESC {
            Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
            AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
            MaxLOD: f32::MAX,
            ..Default::default()
        };
        let mut sampler = None;
        // SAFETY: valid descriptor and out-pointer.
        unsafe { device.CreateSamplerState(&desc, Some(&mut sampler)) }.map_err(failed("quad sampler"))?;
        Ok(Self {
            vertex: gpu::vertex_shader(device, SOURCE, "quad", s!("vs_main"))?,
            pixel: gpu::pixel_shader(device, SOURCE, "quad", s!("ps_main"))?,
            constants: Constants::new(device)?,
            sampler: sampler.ok_or("no quad sampler")?,
            blend: gpu::premultiplied(device, true)?,
            rasterizer: gpu::rasterizer(device, false)?,
            targets: Targets::default(),
        })
    }

    /// Draws `picture` on `rectangle` over `image` (made in `format`) as `eye` saw it. The image's
    /// alpha is kept.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(&mut self, device: &ID3D11Device, context: &ID3D11DeviceContext, image: &ID3D11Texture2D, format: DXGI_FORMAT, eye: &EyeView, picture: &ID3D11Texture2D, rectangle: &Rectangle) -> Result<()> {
        let desc = monaka_channel::d3d::texture_desc(image);
        let target = self.targets.view(device, image, format)?;
        let mut view = None;
        // SAFETY: a view of a live texture with shader-resource binding, in its own format.
        unsafe { device.CreateShaderResourceView(picture, None, Some(&mut view)) }.map_err(failed("quad picture view"))?;
        let corners = rectangle.corners().map(|[x, y, z]| [x, y, z, 1.0]);
        self.constants.write(context, &QuadConstants { view: eye.view_rows(), tangents: eye.tangents(), corners });
        gpu::bind_target(context, &target, desc.Width, desc.Height);
        // SAFETY: our own live shaders, views and states, bound for one draw on the viewer's
        // immediate context, which nothing else draws with; the view is unbound after.
        unsafe {
            context.IASetInputLayout(None);
            context.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            context.VSSetShader(&self.vertex, None);
            context.VSSetConstantBuffers(0, Some(&[Some(self.constants.buffer.clone())]));
            context.PSSetShader(&self.pixel, None);
            context.PSSetShaderResources(0, Some(&[view]));
            context.PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
            context.RSSetState(&self.rasterizer);
            context.OMSetBlendState(&self.blend, None, u32::MAX);
            context.OMSetDepthStencilState(None, 0);
            context.Draw(6, 0);
            context.PSSetShaderResources(0, Some(&[None]));
        }
        gpu::unbind_target(context);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
