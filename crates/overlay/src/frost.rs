//! The game blurred behind the home's panels, so they read as frosted glass
//! ([`crate::ring_page::Look::frosted`]). A panel is a layer the compositor shows on its own, and
//! it cannot blur what lies behind one; so before the headset gets each eye, the viewer blurs the
//! part of it behind each panel itself, as that eye saw the panel. The runtime then moves the blur
//! with the eye's image, as it does the debug drawings.

use crate::Result;
use crate::blit::plain;
use crate::eye::EyeView;
use crate::gpu::{self, Constants, Targets, failed};
use crate::quad::Rectangle;
use monaka_channel::d3d::{create_texture, simple_texture_desc, texture_desc};
use windows::Win32::Graphics::Direct3D::{D3D_SRV_DIMENSION_TEXTURE2D, D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::{Interface, s};

const SOURCE: &str = include_str!("frost.hlsl");

/// How many times smaller each way the eye is blurred.
const SMALLER: u32 = 4;

/// The `Frost` constant buffer of `frost.hlsl`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct FrostConstants {
    view: [[f32; 4]; 3],
    tangents: [f32; 4],
    corners: [[f32; 4]; 4],
    texels: [f32; 4],
    target: [f32; 4],
    panel: [f32; 4],
    mode: [u32; 4],
}

/// One of the small images the blur goes back and forth between.
struct Small {
    target: ID3D11RenderTargetView,
    view: ID3D11ShaderResourceView,
}

pub struct FrostPass {
    full: ID3D11VertexShader,
    down: ID3D11PixelShader,
    blur: ID3D11PixelShader,
    panel_vertex: ID3D11VertexShader,
    panel_pixel: ID3D11PixelShader,
    constants: Constants<FrostConstants>,
    sampler: ID3D11SamplerState,
    rasterizer: ID3D11RasterizerState,
    blend: ID3D11BlendState,
    targets: Targets,
    /// The small images, and the eye size they were made for.
    small: Option<([u32; 2], [Small; 2])>,
    /// Views of the eyes read, kept while they are the same textures, and whether each reads as
    /// linear (a typed sRGB image).
    sources: Vec<(usize, ID3D11ShaderResourceView, bool)>,
}

impl FrostPass {
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
        unsafe { device.CreateSamplerState(&desc, Some(&mut sampler)) }.map_err(failed("frost sampler"))?;
        Ok(Self {
            full: gpu::vertex_shader(device, SOURCE, "frost", s!("vs_full"))?,
            down: gpu::pixel_shader(device, SOURCE, "frost", s!("ps_down"))?,
            blur: gpu::pixel_shader(device, SOURCE, "frost", s!("ps_blur"))?,
            panel_vertex: gpu::vertex_shader(device, SOURCE, "frost", s!("vs_panel"))?,
            panel_pixel: gpu::pixel_shader(device, SOURCE, "frost", s!("ps_panel"))?,
            constants: Constants::new(device)?,
            sampler: sampler.ok_or("no frost sampler")?,
            rasterizer: gpu::rasterizer(device, false)?,
            blend: gpu::premultiplied(device, true)?,
            targets: Targets::default(),
            small: None,
            sources: Vec::new(),
        })
    }

    /// Blurs `image` (made in `format`, a copy of `source`) behind each of `panels` as `eye` saw
    /// them, inside the panels' rounded corners. The image's alpha is kept.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(&mut self, device: &ID3D11Device, context: &ID3D11DeviceContext, image: &ID3D11Texture2D, format: DXGI_FORMAT, source: &ID3D11Texture2D, eye: &EyeView, panels: &[Rectangle]) -> Result<()> {
        if panels.is_empty() {
            return Ok(());
        }
        let desc = texture_desc(image);
        let size = [desc.Width, desc.Height];
        let small_size = size.map(|v| (v / SMALLER).max(1));
        if self.small.as_ref().is_none_or(|(made, _)| *made != size) {
            self.small = Some((size, [small(device, small_size)?, small(device, small_size)?]));
        }
        let (read, read_linear) = self.source(device, source)?;
        let target = self.targets.view(device, image, format)?;
        let write_linear = self.targets.format_of(image).is_some_and(is_srgb);
        let Some((_, [first, second])) = &self.small else { return Ok(()) };
        let [w, h] = size.map(|v| v as f32);
        let [sw, sh] = small_size.map(|v| v as f32);
        let mut constants = FrostConstants { view: eye.view_rows(), tangents: eye.tangents(), ..Default::default() };
        // Small, then across, then down.
        constants.texels = [1.0 / w, 1.0 / h, 0.0, 0.0];
        self.constants.write(context, &constants);
        self.full_pass(context, &first.target, small_size, &self.down, &read);
        constants.texels = [1.0 / sw, 1.0 / sh, 1.0 / sw, 0.0];
        self.constants.write(context, &constants);
        self.full_pass(context, &second.target, small_size, &self.blur, &first.view);
        constants.texels = [1.0 / sw, 1.0 / sh, 0.0, 1.0 / sh];
        self.constants.write(context, &constants);
        self.full_pass(context, &first.target, small_size, &self.blur, &second.view);
        // Back into the eye, behind each panel.
        constants.target = [w, h, 0.0, 0.0];
        constants.panel = panel_shape();
        constants.mode = [match (read_linear, write_linear) {
            (true, false) => 1,
            (false, true) => 2,
            _ => 0,
        }, 0, 0, 0];
        gpu::bind_target(context, &target, desc.Width, desc.Height);
        // SAFETY: our own live shaders, views and states, bound for these draws on the viewer's
        // immediate context, which nothing else draws with; the view is unbound after.
        unsafe {
            context.IASetInputLayout(None);
            context.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            context.VSSetShader(&self.panel_vertex, None);
            context.VSSetConstantBuffers(0, Some(&[Some(self.constants.buffer.clone())]));
            context.PSSetShader(&self.panel_pixel, None);
            context.PSSetConstantBuffers(0, Some(&[Some(self.constants.buffer.clone())]));
            context.PSSetShaderResources(0, Some(&[Some(first.view.clone())]));
            context.PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
            context.RSSetState(&self.rasterizer);
            context.OMSetBlendState(&self.blend, None, u32::MAX);
            context.OMSetDepthStencilState(None, 0);
        }
        for panel in panels {
            constants.corners = panel.corners().map(|[x, y, z]| [x, y, z, 1.0]);
            self.constants.write(context, &constants);
            // SAFETY: as above.
            unsafe { context.Draw(6, 0) };
        }
        // SAFETY: binds nothing.
        unsafe { context.PSSetShaderResources(0, Some(&[None])) };
        gpu::unbind_target(context);
        Ok(())
    }

    /// One triangle over `target` (`size`), `pixel` reading `read`.
    fn full_pass(&self, context: &ID3D11DeviceContext, target: &ID3D11RenderTargetView, size: [u32; 2], pixel: &ID3D11PixelShader, read: &ID3D11ShaderResourceView) {
        gpu::bind_target(context, target, size[0], size[1]);
        // SAFETY: our own live shaders, views and states, bound for one draw on the viewer's
        // immediate context; what was read is unbound after, so it can be drawn into next.
        unsafe {
            context.IASetInputLayout(None);
            context.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            context.VSSetShader(&self.full, None);
            context.PSSetShader(pixel, None);
            context.PSSetConstantBuffers(0, Some(&[Some(self.constants.buffer.clone())]));
            context.PSSetShaderResources(0, Some(&[Some(read.clone())]));
            context.PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
            context.RSSetState(&self.rasterizer);
            context.OMSetBlendState(None, None, u32::MAX);
            context.OMSetDepthStencilState(None, 0);
            context.Draw(3, 0);
            context.PSSetShaderResources(0, Some(&[None]));
        }
        gpu::unbind_target(context);
    }

    /// A view reading `source` (a typeless one as its plain format), and whether it reads as linear.
    fn source(&mut self, device: &ID3D11Device, source: &ID3D11Texture2D) -> Result<(ID3D11ShaderResourceView, bool)> {
        let key = source.as_raw() as usize;
        if let Some((_, view, linear)) = self.sources.iter().find(|(k, _, _)| *k == key) {
            return Ok((view.clone(), *linear));
        }
        let format = plain(texture_desc(source).Format);
        let desc = D3D11_SHADER_RESOURCE_VIEW_DESC {
            Format: format,
            ViewDimension: D3D_SRV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_SRV { MostDetailedMip: 0, MipLevels: 1 } },
        };
        let mut view = None;
        // SAFETY: a view of a live texture in a format of its family.
        unsafe { device.CreateShaderResourceView(source, Some(&desc), Some(&mut view)) }.map_err(failed("frost eye view"))?;
        let view = view.ok_or("no frost eye view")?;
        // The eyes are made again with a new game: views of ones gone are let go.
        if self.sources.len() >= 4 {
            self.sources.remove(0);
        }
        self.sources.push((key, view.clone(), is_srgb(format)));
        Ok((view, is_srgb(format)))
    }
}

/// A small image `size` big, drawn into and read.
fn small(device: &ID3D11Device, size: [u32; 2]) -> Result<Small> {
    let mut desc = simple_texture_desc(size[0], size[1], DXGI_FORMAT_R16G16B16A16_FLOAT);
    desc.BindFlags = (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32;
    let texture = create_texture(device, &desc).map_err(|e| format!("frost image: {e}"))?;
    let (mut target, mut view) = (None, None);
    // SAFETY: views of our own live texture, in its own format.
    unsafe {
        device.CreateRenderTargetView(&texture, None, Some(&mut target)).map_err(failed("frost target"))?;
        device.CreateShaderResourceView(&texture, None, Some(&mut view)).map_err(failed("frost view"))?;
    }
    Ok(Small { target: target.ok_or("no frost target")?, view: view.ok_or("no frost view")? })
}

/// A ring panel's size and the radius of its corners, in its own pixels.
fn panel_shape() -> [f32; 4] {
    let [w, h] = crate::home::place::PIXELS.map(|v| v as f32);
    [w, h, crate::ring_page::CORNER as f32 * crate::canvas::PIXELS_PER_POINT, 0.0]
}

fn is_srgb(format: DXGI_FORMAT) -> bool {
    matches!(format, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB)
}

#[cfg(test)]
mod tests;
