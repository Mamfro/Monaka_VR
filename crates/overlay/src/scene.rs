//! The home scene: a room round the player while no game shows, from captures of a 3D
//! scene ([`files`]). Each pixel of a capture is a tile at its own distance, so the room has depth
//! in both eyes and leaning shows it from a little to the side; where the middle capture could not
//! see, the others fill in, and what none saw shows the middle's skirts (what lies behind drawn on
//! over the gap). The colours are those captured at the time of day now, faded into the next as it
//! comes.

pub mod files;
pub mod pack;
#[cfg(test)]
mod tests;

use crate::Result;
use crate::eye::EyeView;
use crate::gpu::{self, Constants, Targets, failed};
use files::Minutes;
use monaka_channel::d3d::{create_texture, create_texture_with, simple_texture_desc, texture_desc};
use windows::Win32::Graphics::Direct3D::{D3D_SRV_DIMENSION_BUFFER, D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::s;

const SOURCE: &str = include_str!("scene.hlsl");

/// The near plane (metres): nearer than any capture's surfaces, so none is cut.
const NEAR: f32 = 0.05;
/// What shows where no capture saw: the dark of the home without a scene (sRGB 12, 12, 16).
const BACKGROUND: [u8; 3] = [12, 12, 16];
/// How much further from their own capture point each other capture's tiles are pushed, one step
/// more for each (nearest the eye first): where two captures saw the same surface the middle's
/// tiles win, then the nearer capture's, and no two fight over a pixel.
const PUSH: f32 = 0.015;
/// Times kept on the GPU, the least lately drawn let go first: the one shown and the one it fades
/// into.
const TIMES_KEPT: usize = 2;

/// The `Scene` constant buffer of `scene.hlsl`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct SceneConstants {
    view: [[f32; 4]; 3],
    tangents: [f32; 4],
    offset: [f32; 4],
    grid: [u32; 4],
    fade: [f32; 4],
}

/// A capture point on the GPU: the pixels it draws (none: all, in order), each one's corners,
/// every pixel's own distance (its neighbours read it), the middle's skirts.
struct Place {
    offset: [f32; 3],
    corners: ID3D11ShaderResourceView,
    tiles: Option<ID3D11ShaderResourceView>,
    distances: ID3D11ShaderResourceView,
    count: u32,
    skirts: ID3D11ShaderResourceView,
    skirt_count: u32,
}

pub struct ScenePass {
    vertex: ID3D11VertexShader,
    skirt: ID3D11VertexShader,
    pixel: ID3D11PixelShader,
    constants: Constants<SceneConstants>,
    rasterizer: ID3D11RasterizerState,
    /// Tiles: the nearer wins, and each marks its pixels drawn (stencil 1).
    nearer: ID3D11DepthStencilState,
    /// Skirts: only on pixels no tile was drawn on, the nearer winning.
    gaps: ID3D11DepthStencilState,
    targets: Targets,
    depth: Option<([u32; 2], ID3D11DepthStencilView)>,
    face: u32,
    /// The dither the scene is shown with ([`files::Loaded`]).
    shades: u32,
    dither: f32,
    places: Vec<Place>,
    /// The times' colours on the GPU, one image per place, least lately drawn first.
    times: Vec<(Minutes, Vec<ID3D11ShaderResourceView>)>,
}

impl ScenePass {
    /// The pass with `loaded`'s places and times on the GPU.
    pub fn new(device: &ID3D11Device, loaded: &files::Loaded) -> Result<Self> {
        let stencil = |pass, func| D3D11_DEPTH_STENCILOP_DESC { StencilFailOp: D3D11_STENCIL_OP_KEEP, StencilDepthFailOp: D3D11_STENCIL_OP_KEEP, StencilPassOp: pass, StencilFunc: func };
        let state = |pass, func| -> Result<ID3D11DepthStencilState> {
            let desc = D3D11_DEPTH_STENCIL_DESC {
                DepthEnable: true.into(),
                DepthWriteMask: D3D11_DEPTH_WRITE_MASK_ALL,
                DepthFunc: D3D11_COMPARISON_GREATER,
                StencilEnable: true.into(),
                StencilReadMask: 0xff,
                StencilWriteMask: 0xff,
                FrontFace: stencil(pass, func),
                BackFace: stencil(pass, func),
            };
            let mut made = None;
            // SAFETY: valid descriptor and out-pointer.
            unsafe { device.CreateDepthStencilState(&desc, Some(&mut made)) }.map_err(failed("scene depth state"))?;
            made.ok_or_else(|| "no scene depth state".into())
        };
        let mut pass = Self {
            vertex: gpu::vertex_shader(device, SOURCE, "scene", s!("vs_main"))?,
            skirt: gpu::vertex_shader(device, SOURCE, "scene", s!("vs_skirt"))?,
            pixel: gpu::pixel_shader(device, SOURCE, "scene", s!("ps_main"))?,
            constants: Constants::new(device)?,
            rasterizer: gpu::rasterizer(device, false)?,
            nearer: state(D3D11_STENCIL_OP_REPLACE, D3D11_COMPARISON_ALWAYS)?,
            gaps: state(D3D11_STENCIL_OP_KEEP, D3D11_COMPARISON_EQUAL)?,
            targets: Targets::default(),
            depth: None,
            face: loaded.face,
            shades: loaded.shades,
            dither: loaded.dither,
            places: Vec::new(),
            times: Vec::new(),
        };
        for place in &loaded.places {
            // Each drawn tile's corners as four 16-bit codes, in the order the tiles are drawn.
            let code = |c: &[f32; 4]| c.map(|d| u32::from(files::distance_code(d)));
            let corners: Vec<u32> = place.tiles.iter().flat_map(|&t| {
                let [a, b, c, d] = code(&place.corners[t as usize]);
                [a | b << 16, c | d << 16]
            }).collect();
            let distances: Vec<u8> = place.corners.iter().flat_map(|c| files::distance_code(c.iter().sum::<f32>() * 0.25).to_le_bytes()).collect();
            let texture = create_texture_with(device, &simple_texture_desc(loaded.face * 6, loaded.face, DXGI_FORMAT_R16_UNORM).with_bind(D3D11_BIND_SHADER_RESOURCE), &distances)
                .map_err(|e| format!("scene distances: {e}"))?;
            let all = place.tiles.len() == place.corners.len() && place.tiles.iter().enumerate().all(|(k, &t)| t as usize == k);
            pass.places.push(Place {
                offset: place.offset,
                corners: buffer_view(device, &corners, 8)?,
                tiles: if all { None } else { Some(tile_list(device, &place.tiles)?) },
                distances: texture_view(device, &texture)?,
                count: place.tiles.len() as u32,
                skirts: tile_list(device, &place.skirts)?,
                skirt_count: place.skirts.len() as u32,
            });
        }
        for (time, images) in &loaded.colours {
            pass.add_time(device, *time, images)?;
        }
        Ok(pass)
    }

    /// Whether `time`'s colours are on the GPU.
    pub fn has_time(&self, time: Minutes) -> bool {
        self.times.iter().any(|(h, _)| *h == time)
    }

    /// Uses `views` (one per place) as `time`'s colours: images already on the GPU, as a made
    /// background paints them ([`crate::pattern`]), in display sRGB.
    pub fn add_time_views(&mut self, time: Minutes, views: Vec<ID3D11ShaderResourceView>) {
        self.times.retain(|(h, _)| *h != time);
        self.times.push((time, views));
        if self.times.len() > TIMES_KEPT {
            self.times.remove(0);
        }
    }

    /// Puts `time`'s colours (one RGBA image per place) on the GPU, letting the oldest go.
    pub fn add_time(&mut self, device: &ID3D11Device, time: Minutes, images: &[Vec<u8>]) -> Result<()> {
        let desc = simple_texture_desc(self.face * 6, self.face, DXGI_FORMAT_R8G8B8A8_UNORM).with_bind(D3D11_BIND_SHADER_RESOURCE);
        let views = images
            .iter()
            .map(|pixels| create_texture_with(device, &desc, pixels).map_err(|e| format!("scene colours: {e}")).and_then(|texture| texture_view(device, &texture)))
            .collect::<Result<Vec<_>>>()?;
        self.times.retain(|(h, _)| *h != time);
        self.times.push((time, views));
        if self.times.len() > TIMES_KEPT {
            self.times.remove(0);
        }
        Ok(())
    }

    /// Draws the scene into `image` (made in `format`) as `eye` sees it: time `now` faded `fade`
    /// of the way into `next` (`now` alone while `next` is not on the GPU). False, and nothing
    /// drawn, while `now` is not on the GPU.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(&mut self, device: &ID3D11Device, context: &ID3D11DeviceContext, image: &ID3D11Texture2D, format: DXGI_FORMAT, eye: &EyeView, now: Minutes, next: Minutes, fade: f32) -> Result<bool> {
        for time in [next, now] {
            if let Some(at) = self.times.iter().position(|(h, _)| *h == time) {
                let drawn = self.times.remove(at);
                self.times.push(drawn);
            }
        }
        let Some(now_views) = self.times.iter().find(|(h, _)| *h == now).map(|(_, v)| v.clone()) else { return Ok(false) };
        let (next_views, fade) = match self.times.iter().find(|(h, _)| *h == next) {
            Some((_, views)) => (views.clone(), fade),
            None => (now_views.clone(), 0.0),
        };
        let desc = texture_desc(image);
        let size = [desc.Width, desc.Height];
        if self.depth.as_ref().is_none_or(|(made, _)| *made != size) {
            self.depth = Some((size, depth_buffer(device, size)?));
        }
        let Some((_, depth)) = &self.depth else { return Ok(false) };
        let target = self.targets.view(device, image, format)?;
        let linear = self.targets.format_of(image).is_some_and(|f| matches!(f, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB));
        let background = BACKGROUND.map(|c| if linear { srgb_to_linear(c) } else { f32::from(c) / 255.0 });
        // The middle first; then the others, the one nearest the eye first.
        let mut order: Vec<usize> = (1..self.places.len()).collect();
        let away = |place: &Place| (0..3).map(|k| (place.offset[k] - eye.position[k]).powi(2)).sum::<f32>();
        order.sort_by(|&a, &b| away(&self.places[a]).total_cmp(&away(&self.places[b])));
        order.insert(0, 0);
        let viewport = D3D11_VIEWPORT { TopLeftX: 0.0, TopLeftY: 0.0, Width: size[0] as f32, Height: size[1] as f32, MinDepth: 0.0, MaxDepth: 1.0 };
        // SAFETY: our own live shaders, views, buffers and states, bound for these draws on the
        // viewer's immediate context, which nothing else draws with; all are unbound after.
        unsafe {
            context.ClearRenderTargetView(&target, &[background[0], background[1], background[2], 1.0]);
            context.ClearDepthStencilView(depth, D3D11_CLEAR_DEPTH.0 | D3D11_CLEAR_STENCIL.0, 0.0, 0);
            context.OMSetRenderTargets(Some(&[Some(target.clone())]), depth);
            context.RSSetViewports(Some(&[viewport]));
            context.IASetInputLayout(None);
            context.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            context.VSSetShader(&self.vertex, None);
            context.VSSetConstantBuffers(0, Some(&[Some(self.constants.buffer.clone())]));
            context.PSSetShader(&self.pixel, None);
            context.PSSetConstantBuffers(0, Some(&[Some(self.constants.buffer.clone())]));
            context.RSSetState(&self.rasterizer);
            context.OMSetBlendState(None, None, u32::MAX);
            context.OMSetDepthStencilState(&self.nearer, 1);
        }
        // Every place's tiles, then the middle's skirts over what none of them drew.
        let draws = order.iter().enumerate().map(|(rank, &index)| (rank, index, false)).chain([(0, 0, true)]);
        for (rank, index, skirts) in draws {
            let place = &self.places[index];
            let (Some(now_view), Some(next_view)) = (now_views.get(index), next_views.get(index)) else { continue };
            let [x, y, z] = place.offset;
            self.constants.write(
                context,
                &SceneConstants {
                    view: eye.view_rows(),
                    tangents: eye.tangents(),
                    offset: [x, y, z, 1.0 + PUSH * rank as f32],
                    grid: [self.face, self.face * 6, self.shades, u32::from(!skirts && place.tiles.is_none())],
                    fade: [fade, NEAR, self.dither, if linear { 1.0 } else { 0.0 }],
                },
            );
            let (list, count) = if skirts { (Some(place.skirts.clone()), place.skirt_count) } else { (place.tiles.clone(), place.count) };
            let views = [Some(place.corners.clone()), list, Some(now_view.clone()), Some(next_view.clone()), Some(place.distances.clone())];
            // SAFETY: as above.
            unsafe {
                if skirts {
                    context.VSSetShader(&self.skirt, None);
                    context.OMSetDepthStencilState(&self.gaps, 0);
                }
                context.VSSetShaderResources(0, Some(&views));
                context.PSSetShaderResources(0, Some(&views));
                context.Draw(count * 6, 0);
            }
        }
        // SAFETY: binds nothing.
        unsafe {
            context.VSSetShaderResources(0, Some(&[None, None, None, None, None]));
            context.PSSetShaderResources(0, Some(&[None, None, None, None, None]));
            context.OMSetDepthStencilState(None, 0);
            context.OMSetRenderTargets(None, None);
        }
        Ok(true)
    }
}

trait WithBind {
    fn with_bind(self, bind: D3D11_BIND_FLAG) -> Self;
}

impl WithBind for D3D11_TEXTURE2D_DESC {
    fn with_bind(self, bind: D3D11_BIND_FLAG) -> Self {
        D3D11_TEXTURE2D_DESC { BindFlags: bind.0 as u32, Usage: D3D11_USAGE_IMMUTABLE, ..self }
    }
}

fn texture_view(device: &ID3D11Device, texture: &ID3D11Texture2D) -> Result<ID3D11ShaderResourceView> {
    let mut view = None;
    // SAFETY: a view of our own live texture, in its own format.
    unsafe { device.CreateShaderResourceView(texture, None, Some(&mut view)) }.map_err(failed("scene view"))?;
    view.ok_or_else(|| "no scene view".into())
}

/// The pixels a place draws, as a structured buffer the vertex shader reads.
fn tile_list(device: &ID3D11Device, tiles: &[u32]) -> Result<ID3D11ShaderResourceView> {
    buffer_view(device, tiles, 4)
}

/// `words` as a structured buffer of `stride`-byte entries the shaders read.
fn buffer_view(device: &ID3D11Device, words: &[u32], stride: u32) -> Result<ID3D11ShaderResourceView> {
    let per = stride as usize / 4;
    let count = (words.len() / per).max(1);
    let padded: Vec<u32> = if words.is_empty() { vec![0; per] } else { words.to_vec() };
    let desc = D3D11_BUFFER_DESC {
        ByteWidth: count as u32 * stride,
        Usage: D3D11_USAGE_IMMUTABLE,
        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: D3D11_RESOURCE_MISC_BUFFER_STRUCTURED.0 as u32,
        StructureByteStride: stride,
    };
    let data = D3D11_SUBRESOURCE_DATA { pSysMem: padded.as_ptr().cast(), SysMemPitch: 0, SysMemSlicePitch: 0 };
    let mut buffer = None;
    // SAFETY: the data covers the whole buffer and outlives the call.
    unsafe { device.CreateBuffer(&desc, Some(&data), Some(&mut buffer)) }.map_err(failed("scene tiles"))?;
    let buffer = buffer.ok_or("no scene tiles")?;
    let view_desc = D3D11_SHADER_RESOURCE_VIEW_DESC {
        Format: DXGI_FORMAT_UNKNOWN,
        ViewDimension: D3D_SRV_DIMENSION_BUFFER,
        Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 { Buffer: D3D11_BUFFER_SRV { Anonymous1: D3D11_BUFFER_SRV_0 { FirstElement: 0 }, Anonymous2: D3D11_BUFFER_SRV_1 { NumElements: count as u32 } } },
    };
    let mut view = None;
    // SAFETY: a view of our own structured buffer, all of it.
    unsafe { device.CreateShaderResourceView(&buffer, Some(&view_desc), Some(&mut view)) }.map_err(failed("scene tiles view"))?;
    view.ok_or_else(|| "no scene tiles view".into())
}

fn depth_buffer(device: &ID3D11Device, size: [u32; 2]) -> Result<ID3D11DepthStencilView> {
    // 24 bits: the scene's nearest and farthest (a few centimetres to the sky) still differ in
    // steps far finer than the pushes between capture points.
    let mut desc = simple_texture_desc(size[0], size[1], DXGI_FORMAT_D24_UNORM_S8_UINT);
    desc.BindFlags = D3D11_BIND_DEPTH_STENCIL.0 as u32;
    let texture = create_texture(device, &desc).map_err(|e| format!("scene depth: {e}"))?;
    let mut view = None;
    // SAFETY: a view of our own live depth texture.
    unsafe { device.CreateDepthStencilView(&texture, None, Some(&mut view)) }.map_err(failed("scene depth view"))?;
    view.ok_or_else(|| "no scene depth view".into())
}

fn srgb_to_linear(c: u8) -> f32 {
    let v = f32::from(c) / 255.0;
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}
