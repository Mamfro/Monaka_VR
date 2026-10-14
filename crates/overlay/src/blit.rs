//! An image drawn into a rectangle of a target, scaled with linear filtering: the 2D window's
//! picture of an eye. The image's stored values reach the plain target as they are (what the game
//! showed): a typeless image is read as its plain format, and a typed sRGB one (read as linear) is
//! encoded again.

use crate::Result;
use crate::gpu::{self, Constants, failed};
use monaka_channel::d3d::texture_desc;
use windows::Win32::Graphics::Direct3D::{D3D_SRV_DIMENSION_TEXTURE2D, D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::{Interface, s};

#[cfg(test)]
mod tests;

const SOURCE: &str = r"
Texture2D image : register(t0);
SamplerState linear_clamp : register(s0);
cbuffer Blit : register(b0) { uint encode; uint3 unused; };

float3 to_srgb(float3 c) {
    return c <= 0.0031308 ? c * 12.92 : 1.055 * pow(c, 1.0 / 2.4) - 0.055;
}

struct Out { float4 position : SV_Position; float2 uv : TEXCOORD0; };

// One triangle over the viewport.
Out vs_main(uint id : SV_VertexID) {
    Out o;
    o.uv = float2((id << 1) & 2, id & 2);
    o.position = float4(o.uv * float2(2.0, -2.0) + float2(-1.0, 1.0), 0.0, 1.0);
    return o;
}

float4 ps_main(Out i) : SV_Target {
    float3 c = image.Sample(linear_clamp, i.uv).rgb;
    return float4(encode != 0 ? to_srgb(saturate(c)) : c, 1.0);
}
";

/// The `Blit` constant buffer.
#[repr(C)]
#[derive(Clone, Copy)]
struct BlitConstants {
    encode: u32,
    unused: [u32; 3],
}

pub struct BlitPass {
    vertex: ID3D11VertexShader,
    pixel: ID3D11PixelShader,
    sampler: ID3D11SamplerState,
    rasterizer: ID3D11RasterizerState,
    constants: Constants<BlitConstants>,
    /// The view of the last image drawn, kept while it is the same texture, and whether it reads
    /// as linear (a typed sRGB image).
    view: Option<(usize, ID3D11ShaderResourceView, bool)>,
}

impl BlitPass {
    pub fn new(device: &ID3D11Device) -> Result<Self> {
        let vertex = gpu::vertex_shader(device, SOURCE, "blit", s!("vs_main"))?;
        let pixel = gpu::pixel_shader(device, SOURCE, "blit", s!("ps_main"))?;
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
        unsafe { device.CreateSamplerState(&desc, Some(&mut sampler)) }.map_err(failed("blit sampler"))?;
        Ok(Self { vertex, pixel, sampler: sampler.ok_or("no blit sampler")?, rasterizer: gpu::rasterizer(device, false)?, constants: Constants::new(device)?, view: None })
    }

    /// Draws `image` into `rect` (left, top, width, height in pixels) of `target`.
    pub fn draw(&mut self, device: &ID3D11Device, context: &ID3D11DeviceContext, target: &ID3D11RenderTargetView, image: &ID3D11Texture2D, rect: [f32; 4]) -> Result<()> {
        let key = image.as_raw() as usize;
        if self.view.as_ref().is_none_or(|(k, _, _)| *k != key) {
            let stored = texture_desc(image).Format;
            let linear = matches!(stored, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB);
            let desc = D3D11_SHADER_RESOURCE_VIEW_DESC {
                Format: plain(stored),
                ViewDimension: D3D_SRV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_SRV { MostDetailedMip: 0, MipLevels: 1 } },
            };
            let mut view = None;
            // SAFETY: a view of a live texture in a format of its family.
            unsafe { device.CreateShaderResourceView(image, Some(&desc), Some(&mut view)) }.map_err(failed("blit image view"))?;
            self.view = Some((key, view.ok_or("no blit image view")?, linear));
        }
        let Some((_, view, linear)) = &self.view else { return Ok(()) };
        self.constants.write(context, &BlitConstants { encode: u32::from(*linear), unused: [0; 3] });
        let viewport = D3D11_VIEWPORT { TopLeftX: rect[0], TopLeftY: rect[1], Width: rect[2], Height: rect[3], MinDepth: 0.0, MaxDepth: 1.0 };
        // SAFETY: our own shaders, states and views, bound for one draw; the image's view is unbound
        // after it, so the image can be written again.
        unsafe {
            context.OMSetRenderTargets(Some(&[Some(target.clone())]), None);
            context.OMSetBlendState(None, None, u32::MAX);
            context.RSSetViewports(Some(&[viewport]));
            context.RSSetState(&self.rasterizer);
            context.IASetInputLayout(None);
            context.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            context.VSSetShader(&self.vertex, None);
            context.PSSetShader(&self.pixel, None);
            context.PSSetConstantBuffers(0, Some(&[Some(self.constants.buffer.clone())]));
            context.PSSetShaderResources(0, Some(&[Some(view.clone())]));
            context.PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
            context.Draw(3, 0);
            context.PSSetShaderResources(0, Some(&[None]));
        }
        gpu::unbind_target(context);
        Ok(())
    }
}

/// The format to read a texture stored as `format` with: a typeless one as its plain typed one,
/// a typed one as itself.
fn plain(format: DXGI_FORMAT) -> DXGI_FORMAT {
    match format {
        DXGI_FORMAT_R8G8B8A8_TYPELESS => DXGI_FORMAT_R8G8B8A8_UNORM,
        DXGI_FORMAT_B8G8R8A8_TYPELESS => DXGI_FORMAT_B8G8R8A8_UNORM,
        DXGI_FORMAT_R10G10B10A2_TYPELESS => DXGI_FORMAT_R10G10B10A2_UNORM,
        DXGI_FORMAT_R16G16B16A16_TYPELESS => DXGI_FORMAT_R16G16B16A16_FLOAT,
        other => other,
    }
}

/// Where an image `image` (width, height) goes in a target `target` big, as large as fits with its
/// shape kept, centred: left, top, width, height.
pub fn fitted(image: [u32; 2], target: [u32; 2]) -> [f32; 4] {
    let [iw, ih] = image.map(|v| v.max(1) as f32);
    let [tw, th] = target.map(|v| v as f32);
    let scale = (tw / iw).min(th / ih);
    let (w, h) = (iw * scale, ih * scale);
    [(tw - w) * 0.5, (th - h) * 0.5, w, h]
}
