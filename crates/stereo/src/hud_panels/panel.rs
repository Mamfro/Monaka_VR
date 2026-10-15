//! A HUD piece's panel on a hand, on a D3D11 device of the producer's own or the game's: the piece
//! drawn twice, onto black and onto white ([`Panel::targets`]; difference matting, the HUD
//! layer's own way, [`monaka_core::hud::HLSL`]), resolved into premultiplied RGBA and published on
//! its own fence channel, `<channel>-panel-<piece>`, which the viewer places on a hand.
//! A panel made masked has a depth-stencil of its own beside each target, cleared with it, so a
//! HUD that masks its draws through the stencil (a bar's fill) is masked on the hand too.

use monaka_channel::ChannelName;
use monaka_channel::d3d;
use monaka_channel::d3d12::compute::{blob_bytes, compile};
use crate::hud_panels::PanelOut;
use monaka_channel::fence11::LazyProducer11;
use monaka_producer::log;
use windows::Win32::Graphics::Direct3D::D3D_SRV_DIMENSION_TEXTURE2D;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_FORMAT_D24_UNORM_S8_UINT, DXGI_FORMAT_R8G8B8A8_TYPELESS, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB};
use windows::core::s;

const RESOLVE: &str = r"
cbuffer Size : register(b0) { uint W, H, pad0, pad1; };
Texture2D<float4> Black : register(t0);
Texture2D<float4> White : register(t1);
RWTexture2D<unorm float4> Out : register(u0);
[numthreads(8, 8, 1)] void Resolve(uint3 id : SV_DispatchThreadID) {
    if (id.x >= W || id.y >= H) return;
    float3 black = Black.Load(int3(id.xy, 0)).rgb, white = White.Load(int3(id.xy, 0)).rgb;
    float a = saturate(1 - dot(white - black, float3(1, 1, 1) / 3));
    Out[id.xy] = float4(saturate(black), a);
}
";

/// One drawn copy of the piece: its texture and the views to clear and read it, and its
/// depth-stencil when the panel is masked.
struct Target {
    texture: ID3D11Texture2D,
    rtv: ID3D11RenderTargetView,
    srv: ID3D11ShaderResourceView,
    dsv: Option<ID3D11DepthStencilView>,
}

/// One piece's panel.
pub struct Panel {
    piece: &'static str,
    size: (u32, u32),
    targets: [Target; 2],
    out: ID3D11Texture2D,
    out_uav: ID3D11UnorderedAccessView,
    resolve: ID3D11ComputeShader,
    constants: ID3D11Buffer,
    device: ID3D11Device,
    writer: LazyProducer11,
    failures: u64,
}

fn target(device: &ID3D11Device, size: (u32, u32), format: DXGI_FORMAT, masked: bool) -> Result<Target, String> {
    let mut desc = d3d::simple_texture_desc(size.0, size.1, DXGI_FORMAT_R8G8B8A8_TYPELESS);
    desc.BindFlags = (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32;
    let texture = d3d::create_texture(device, &desc).map_err(|e| e.to_string())?;
    let rtv_desc = D3D11_RENDER_TARGET_VIEW_DESC {
        Format: format,
        ViewDimension: D3D11_RTV_DIMENSION_TEXTURE2D,
        Anonymous: D3D11_RENDER_TARGET_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_RTV { MipSlice: 0 } },
    };
    let (mut rtv, mut srv) = (None, None);
    let srv_desc = D3D11_SHADER_RESOURCE_VIEW_DESC {
        Format: DXGI_FORMAT_R8G8B8A8_UNORM,
        ViewDimension: D3D_SRV_DIMENSION_TEXTURE2D,
        Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_SRV { MostDetailedMip: 0, MipLevels: 1 } },
    };
    // SAFETY: views of our own texture in formats of its family, into locals.
    unsafe {
        device.CreateRenderTargetView(&texture, Some(&rtv_desc), Some(&mut rtv)).map_err(|e| e.to_string())?;
        device.CreateShaderResourceView(&texture, Some(&srv_desc), Some(&mut srv)).map_err(|e| e.to_string())?;
    }
    let dsv = if masked { Some(depth_stencil(device, size)?) } else { None };
    Ok(Target { texture, rtv: rtv.ok_or("render target view")?, srv: srv.ok_or("shader resource view")?, dsv })
}

fn depth_stencil(device: &ID3D11Device, size: (u32, u32)) -> Result<ID3D11DepthStencilView, String> {
    let mut desc = d3d::simple_texture_desc(size.0, size.1, DXGI_FORMAT_D24_UNORM_S8_UINT);
    desc.BindFlags = D3D11_BIND_DEPTH_STENCIL.0 as u32;
    let texture = d3d::create_texture(device, &desc).map_err(|e| e.to_string())?;
    let mut dsv = None;
    // SAFETY: a view of our own texture, into a local.
    unsafe { device.CreateDepthStencilView(&texture, None, Some(&mut dsv)) }.map_err(|e| e.to_string())?;
    dsv.ok_or_else(|| "depth-stencil view".to_owned())
}

impl Panel {
    /// Piece `piece`'s panel of `size` on `device`, published on `<channel>-panel-<piece>`, drawn
    /// into through views of `format` (RGBA8, plain or sRGB: the game's own HUD target's, so its
    /// texels are what the eye image's would be); `masked`: with depth-stencils.
    pub fn new(device: &ID3D11Device, channel: &ChannelName, piece: &'static str, size: (u32, u32), format: DXGI_FORMAT, masked: bool) -> Result<Self, String> {
        if !matches!(format, DXGI_FORMAT_R8G8B8A8_UNORM | DXGI_FORMAT_R8G8B8A8_UNORM_SRGB) {
            return Err(format!("the HUD draws into format {}, not RGBA8", format.0));
        }
        let name = channel.panel(piece).ok_or_else(|| format!("no channel name for the {piece} panel"))?;
        let targets = [target(device, size, format, masked)?, target(device, size, format, masked)?];
        let mut out_desc = d3d::simple_texture_desc(size.0, size.1, DXGI_FORMAT_R8G8B8A8_UNORM);
        out_desc.BindFlags = (D3D11_BIND_UNORDERED_ACCESS.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32;
        let out = d3d::create_texture(device, &out_desc).map_err(|e| e.to_string())?;
        let code = compile(RESOLVE, "panel resolve", s!("Resolve"))?;
        let buffer_desc = D3D11_BUFFER_DESC { ByteWidth: 16, Usage: D3D11_USAGE_DEFAULT, BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32, ..Default::default() };
        let (mut out_uav, mut resolve, mut constants) = (None, None, None);
        // SAFETY: a view of our own texture, a shader from bytecode the compiler just made, a
        // buffer from a valid descriptor; all into locals.
        unsafe {
            device.CreateUnorderedAccessView(&out, None, Some(&mut out_uav)).map_err(|e| e.to_string())?;
            device.CreateComputeShader(blob_bytes(&code), None, Some(&mut resolve)).map_err(|e| e.to_string())?;
            device.CreateBuffer(&buffer_desc, None, Some(&mut constants)).map_err(|e| e.to_string())?;
        }
        Ok(Self {
            piece,
            size,
            targets,
            out,
            out_uav: out_uav.ok_or("unordered access view")?,
            resolve: resolve.ok_or("resolve shader")?,
            constants: constants.ok_or("constant buffer")?,
            device: device.clone(),
            writer: LazyProducer11::mono(name),
            failures: 0,
        })
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// The piece's name (its channel's and the viewer's).
    pub fn piece(&self) -> &'static str {
        self.piece
    }

    /// The piece drawn onto black and onto white (RGBA8, `size`), for this frame: drawn into, or
    /// copied over from a capture.
    pub fn targets(&self) -> [&ID3D11Texture2D; 2] {
        [&self.targets[0].texture, &self.targets[1].texture]
    }

    /// Their render target views (the format the panel was made with).
    pub fn target_views(&self) -> [&ID3D11RenderTargetView; 2] {
        [&self.targets[0].rtv, &self.targets[1].rtv]
    }

    /// Each target's depth-stencil view, when the panel is masked.
    pub fn depth_views(&self) -> [Option<&ID3D11DepthStencilView>; 2] {
        [self.targets[0].dsv.as_ref(), self.targets[1].dsv.as_ref()]
    }

    /// Clears the targets for a new frame: onto black, onto white (and their depth-stencils).
    pub fn clear(&self, context: &ID3D11DeviceContext) {
        // SAFETY: clears our own targets.
        unsafe {
            context.ClearRenderTargetView(&self.targets[0].rtv, &[0.0, 0.0, 0.0, 0.0]);
            context.ClearRenderTargetView(&self.targets[1].rtv, &[1.0, 1.0, 1.0, 1.0]);
            for dsv in self.targets.iter().filter_map(|t| t.dsv.as_ref()) {
                context.ClearDepthStencilView(dsv, D3D11_CLEAR_DEPTH.0 | D3D11_CLEAR_STENCIL.0, 1.0, 0);
            }
        }
    }

    pub fn device(&self) -> &ID3D11Device {
        &self.device
    }

    /// Publishes the panel as `out` says: the piece as drawn into [`Panel::targets`], or cleared.
    /// Whether it went out.
    pub fn publish(&mut self, context: &ID3D11DeviceContext, out: PanelOut) -> bool {
        if out == PanelOut::Cleared {
            self.clear(context);
        }
        self.resolve(context);
        let piece = self.piece;
        let Some(writer) = self.writer.get(&self.device, self.size.0, self.size.1, DXGI_FORMAT_R8G8B8A8_UNORM).ready(|note| log!("HUD panel {piece}: {note}")) else { return false };
        match writer.publish_one(context, &self.out, monaka_channel::tick()) {
            Ok(published) => published,
            Err(e) => {
                if self.failures == 0 {
                    log!("HUD panel {piece} publish failed: {e}");
                }
                self.failures += 1;
                false
            }
        }
    }

    /// The black and white draws into premultiplied RGBA; the context's compute bindings are given
    /// back after.
    fn resolve(&self, context: &ID3D11DeviceContext) {
        let constants = [self.size.0, self.size.1, 0, 0];
        let mut shader = None;
        let mut views: [Option<ID3D11ShaderResourceView>; 2] = [None, None];
        let mut uav = [None];
        let mut buffers = [None];
        // SAFETY: reads the context's compute bindings, binds ours, dispatches once and gives
        // them back.
        unsafe {
            context.CSGetShader(&mut shader, None, None);
            context.CSGetShaderResources(0, Some(&mut views));
            context.CSGetUnorderedAccessViews(0, Some(&mut uav));
            context.CSGetConstantBuffers(0, Some(&mut buffers));
            context.UpdateSubresource(&self.constants, 0, None, constants.as_ptr().cast(), 0, 0);
            context.CSSetConstantBuffers(0, Some(&[Some(self.constants.clone())]));
            context.CSSetShaderResources(0, Some(&[Some(self.targets[0].srv.clone()), Some(self.targets[1].srv.clone())]));
            context.CSSetUnorderedAccessViews(0, 1, Some([Some(self.out_uav.clone())].as_ptr()), None);
            context.CSSetShader(&self.resolve, None);
            context.Dispatch(self.size.0.div_ceil(8), self.size.1.div_ceil(8), 1);
            context.CSSetUnorderedAccessViews(0, 1, Some(uav.as_ptr()), None);
            context.CSSetShaderResources(0, Some(&views));
            context.CSSetConstantBuffers(0, Some(&buffers));
            context.CSSetShader(shader.as_ref(), None);
        }
    }

    /// The resolved panel (premultiplied RGBA), for a probe.
    pub fn out(&self) -> &ID3D11Texture2D {
        &self.out
    }
}

#[cfg(test)]
mod tests;
