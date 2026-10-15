//! A game's HUD as its own layer, laid over each eye (`hud.hlsl`). The game crate redirects its
//! HUD draws into the layer's two targets, each draw once onto black and once onto white
//! ([`HudLayer::targets`]); [`HudLayer::composite`] then lays the layer over any eye image at that
//! eye's place. Both eyes get the same HUD from the same moment, so it cannot flicker between them.

use crate::{ConstantBuffer, Result, SavedCompute, compute_shader, shader_view, uav_texture};
use monaka_channel::d3d;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R8G8B8A8_TYPELESS, DXGI_FORMAT_R8G8B8A8_UNORM};
use windows::core::{Interface, s};

pub const HUD_SOURCE: &str = include_str!("hud.hlsl");

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Constants {
    width: u32,
    height: u32,
    layer_width: u32,
    layer_height: u32,
    rect: [f32; 4],
}

struct Target {
    view: ID3D11RenderTargetView,
    srv: ID3D11ShaderResourceView,
}

pub struct HudLayer {
    device: ID3D11Device,
    shader: ID3D11ComputeShader,
    constants: ConstantBuffer<Constants>,
    targets: Option<[Target; 2]>,
    size: (u32, u32),
    outputs: [Option<(ID3D11Texture2D, ID3D11UnorderedAccessView)>; 2],
}

/// An RGBA8 texture readable as UNORM (any of the family's formats, copied as stored).
fn unorm_view(device: &ID3D11Device, texture: &ID3D11Texture2D) -> Result<ID3D11ShaderResourceView> {
    shader_view(device, texture, DXGI_FORMAT_R8G8B8A8_UNORM)
}

impl HudLayer {
    pub fn new(device: &ID3D11Device) -> Result<Self> {
        Ok(Self {
            device: device.clone(),
            shader: compute_shader(device, &monaka_core::hud::with_hlsl(HUD_SOURCE), "hud.hlsl", s!("Composite"))?,
            constants: ConstantBuffer::new(device)?,
            targets: None,
            size: (0, 0),
            outputs: [None, None],
        })
    }

    /// Starts a frame's HUD: the black and white targets (`width` x `height`, the game's HUD
    /// layout size), cleared. Draw every HUD draw into both.
    pub fn begin(&mut self, context: &ID3D11DeviceContext, width: u32, height: u32) -> Result<[ID3D11RenderTargetView; 2]> {
        if self.targets.is_none() || self.size != (width, height) {
            let make = || -> Result<Target> {
                let mut desc = d3d::simple_texture_desc(width, height, DXGI_FORMAT_R8G8B8A8_TYPELESS);
                desc.BindFlags = (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32;
                let texture = d3d::create_texture(&self.device, &desc)?;
                let rtv_desc = D3D11_RENDER_TARGET_VIEW_DESC {
                    Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                    ViewDimension: D3D11_RTV_DIMENSION_TEXTURE2D,
                    Anonymous: D3D11_RENDER_TARGET_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_RTV { MipSlice: 0 } },
                };
                let mut view = None;
                // SAFETY: a view of our own texture.
                unsafe { self.device.CreateRenderTargetView(&texture, Some(&rtv_desc), Some(&mut view))? };
                Ok(Target { view: view.expect("view created"), srv: unorm_view(&self.device, &texture)? })
            };
            self.targets = Some([make()?, make()?]);
            self.size = (width, height);
        }
        let targets = self.targets.as_ref().expect("made above");
        // SAFETY: clears our own targets on the game's context.
        unsafe {
            context.ClearRenderTargetView(&targets[0].view, &[0.0, 0.0, 0.0, 0.0]);
            context.ClearRenderTargetView(&targets[1].view, &[1.0, 1.0, 1.0, 1.0]);
        }
        Ok([targets[0].view.clone(), targets[1].view.clone()])
    }

    /// The frame's layer as drawn onto black (for checks: what the game drew into it), once begun.
    pub fn drawn_on_black(&self) -> Option<ID3D11Texture2D> {
        let srv = &self.targets.as_ref()?[0].srv;
        // SAFETY: COM call on our own view.
        unsafe { srv.GetResource() }.ok()?.cast::<ID3D11Texture2D>().ok()
    }

    /// `base` with the HUD laid over it at `rect` (x, y, width, height in `base` pixels), into an
    /// RGBA8 image kept per eye slot. `base` must be in the RGBA8 family and shader-readable.
    pub fn composite(&mut self, context: &ID3D11DeviceContext, slot: usize, base: &ID3D11Texture2D, rect: [f32; 4]) -> Result<&ID3D11Texture2D> {
        let desc = d3d::texture_desc(base);
        let resized = self.outputs[slot].as_ref().is_none_or(|(t, _)| {
            let d = d3d::texture_desc(t);
            d.Width != desc.Width || d.Height != desc.Height
        });
        if resized {
            self.outputs[slot] = Some(uav_texture(&self.device, desc.Width, desc.Height, DXGI_FORMAT_R8G8B8A8_UNORM)?);
        }
        let base_view = unorm_view(&self.device, base)?;
        let (output, uav) = self.outputs[slot].as_ref().expect("made above");
        let Some(targets) = &self.targets else { return Ok(output) };
        let constants = Constants { width: desc.Width, height: desc.Height, layer_width: self.size.0, layer_height: self.size.1, rect };
        let saved = SavedCompute::take(context);
        self.constants.update(context, &constants);
        self.constants.bind(context);
        // SAFETY: binds our shader and views on the game's context, dispatches once, unbinds and
        // restores what was bound.
        unsafe {
            context.CSSetShaderResources(0, Some(&[Some(base_view), Some(targets[0].srv.clone()), Some(targets[1].srv.clone())]));
            context.CSSetUnorderedAccessViews(0, 1, Some([Some(uav.clone())].as_ptr()), None);
            context.CSSetShader(&self.shader, None);
            context.Dispatch(desc.Width.div_ceil(8), desc.Height.div_ceil(8), 1);
            context.CSSetUnorderedAccessViews(0, 1, Some([None].as_ptr()), None);
            context.CSSetShaderResources(0, Some(&[None, None, None]));
        }
        saved.restore(context);
        Ok(output)
    }
}

#[cfg(test)]
mod tests;
