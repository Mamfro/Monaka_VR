//! egui's output drawn with D3D11 into a render target: its textures (the font atlas) kept in step
//! with each frame's changes, its meshes drawn clipped to their rectangles.

use crate::Result;
use crate::gpu::{self, Constants, failed};
use egui::TexturesDelta;
use egui::epaint::{ClippedPrimitive, ImageData, Primitive, TextureId};
use monaka_channel::d3d::{create_texture_with, simple_texture_desc};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D::D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::s;

const SOURCE: &str = include_str!("painter.hlsl");

#[repr(C)]
#[derive(Clone, Copy)]
struct Vertex {
    position: [f32; 2],
    uv: [f32; 2],
    colour: [u8; 4],
}

/// The `Screen` constant buffer of `painter.hlsl`.
#[repr(C)]
#[derive(Clone, Copy)]
struct Screen {
    size: [f32; 2],
    linearize: f32,
    unused: f32,
}

struct Texture {
    id: TextureId,
    texture: ID3D11Texture2D,
    view: ID3D11ShaderResourceView,
}

pub struct Painter {
    vertex: ID3D11VertexShader,
    pixel: ID3D11PixelShader,
    layout: ID3D11InputLayout,
    constants: Constants<Screen>,
    blend: ID3D11BlendState,
    rasterizer: ID3D11RasterizerState,
    sampler: ID3D11SamplerState,
    vertices: Option<(ID3D11Buffer, usize)>,
    indices: Option<(ID3D11Buffer, usize)>,
    textures: Vec<Texture>,
}

impl Painter {
    pub fn new(device: &ID3D11Device) -> Result<Self> {
        let (vertex, code) = gpu::vertex_shader_and_code(device, SOURCE, "menu painter", s!("vs_main"))?;
        let elements = [
            element(s!("POSITION"), DXGI_FORMAT_R32G32_FLOAT, 0),
            element(s!("TEXCOORD"), DXGI_FORMAT_R32G32_FLOAT, 8),
            element(s!("COLOR"), DXGI_FORMAT_R8G8B8A8_UNORM, 16),
        ];
        let mut layout = None;
        // SAFETY: element descriptions of our own vertex, against the shader's own bytecode.
        unsafe { device.CreateInputLayout(&elements, &code, Some(&mut layout)) }.map_err(failed("input layout"))?;
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
        unsafe { device.CreateSamplerState(&desc, Some(&mut sampler)) }.map_err(failed("sampler"))?;
        Ok(Self {
            vertex,
            pixel: gpu::pixel_shader(device, SOURCE, "menu painter", s!("ps_main"))?,
            layout: layout.ok_or("no input layout")?,
            constants: Constants::new(device)?,
            blend: gpu::premultiplied(device, false)?,
            rasterizer: gpu::rasterizer(device, true)?,
            sampler: sampler.ok_or("no sampler")?,
            vertices: None,
            indices: None,
            textures: Vec::new(),
        })
    }

    /// Draws `primitives` into `target` (`size` pixels, `pixels_per_point` of them to egui's
    /// point), after applying `textures`' changes. `linearize`: the target is sRGB.
    #[allow(clippy::too_many_arguments)]
    pub fn paint(
        &mut self,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        target: &ID3D11RenderTargetView,
        size: [u32; 2],
        pixels_per_point: f32,
        linearize: bool,
        primitives: &[ClippedPrimitive],
        textures: &mut TexturesDelta,
    ) -> Result<()> {
        let applied = self.apply(device, context, textures);
        let drawn = applied.and_then(|()| self.draw(device, context, target, size, pixels_per_point, linearize, primitives));
        for id in &textures.free {
            self.textures.retain(|t| t.id != *id);
        }
        textures.clear();
        drawn
    }

    fn apply(&mut self, device: &ID3D11Device, context: &ID3D11DeviceContext, textures: &TexturesDelta) -> Result<()> {
        for (id, deltas) in &textures.set {
            for delta in deltas {
                self.set_texture(device, context, *id, delta)?;
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn draw(&mut self, device: &ID3D11Device, context: &ID3D11DeviceContext, target: &ID3D11RenderTargetView, size: [u32; 2], pixels_per_point: f32, linearize: bool, primitives: &[ClippedPrimitive]) -> Result<()> {
        let mut vertices = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        // Each mesh: (clip rectangle, texture, first index, index count, first vertex).
        let mut draws = Vec::new();
        for clipped in primitives {
            let Primitive::Mesh(mesh) = &clipped.primitive else { continue };
            if mesh.indices.is_empty() {
                continue;
            }
            draws.push((clipped.clip_rect, mesh.texture_id, indices.len() as u32, mesh.indices.len() as u32, vertices.len() as i32));
            vertices.extend(mesh.vertices.iter().map(|v| Vertex { position: [v.pos.x, v.pos.y], uv: [v.uv.x, v.uv.y], colour: v.color.to_array() }));
            indices.extend_from_slice(&mesh.indices);
        }
        if !draws.is_empty() {
            let vertex_buffer = write_dynamic(device, context, &mut self.vertices, &vertices, D3D11_BIND_VERTEX_BUFFER)?;
            let index_buffer = write_dynamic(device, context, &mut self.indices, &indices, D3D11_BIND_INDEX_BUFFER)?;
            let points = [size[0] as f32 / pixels_per_point, size[1] as f32 / pixels_per_point];
            self.constants.write(context, &Screen { size: points, linearize: if linearize { 1.0 } else { 0.0 }, unused: 0.0 });
            gpu::bind_target(context, target, size[0], size[1]);
            let stride = size_of::<Vertex>() as u32;
            // SAFETY: our own live shaders, buffers, views and states, bound for these draws on the
            // viewer's immediate context, which nothing else draws with.
            unsafe {
                context.IASetInputLayout(&self.layout);
                context.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
                context.IASetVertexBuffers(0, 1, Some(&Some(vertex_buffer)), Some(&stride), Some(&0));
                context.IASetIndexBuffer(&index_buffer, DXGI_FORMAT_R32_UINT, 0);
                context.VSSetShader(&self.vertex, None);
                context.VSSetConstantBuffers(0, Some(&[Some(self.constants.buffer.clone())]));
                context.PSSetShader(&self.pixel, None);
                context.PSSetConstantBuffers(0, Some(&[Some(self.constants.buffer.clone())]));
                context.PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
                context.RSSetState(&self.rasterizer);
                context.OMSetBlendState(&self.blend, None, u32::MAX);
                context.OMSetDepthStencilState(None, 0);
                for (clip, texture, first, count, base) in draws {
                    let Some(view) = self.textures.iter().find(|t| t.id == texture).map(|t| t.view.clone()) else { continue };
                    let rect = RECT {
                        left: (clip.min.x * pixels_per_point).clamp(0.0, size[0] as f32) as i32,
                        top: (clip.min.y * pixels_per_point).clamp(0.0, size[1] as f32) as i32,
                        right: (clip.max.x * pixels_per_point).clamp(0.0, size[0] as f32).ceil() as i32,
                        bottom: (clip.max.y * pixels_per_point).clamp(0.0, size[1] as f32).ceil() as i32,
                    };
                    if rect.right <= rect.left || rect.bottom <= rect.top {
                        continue;
                    }
                    context.RSSetScissorRects(Some(&[rect]));
                    context.PSSetShaderResources(0, Some(&[Some(view)]));
                    context.DrawIndexed(count, first, base);
                }
                context.PSSetShaderResources(0, Some(&[None]));
            }
            gpu::unbind_target(context);
        }
        Ok(())
    }

    fn set_texture(&mut self, device: &ID3D11Device, context: &ID3D11DeviceContext, id: TextureId, delta: &egui::epaint::ImageDelta) -> Result<()> {
        let ImageData::Color(image) = &delta.image;
        let [width, height] = image.size;
        let bytes: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
        match delta.pos {
            None => {
                let mut desc = simple_texture_desc(width as u32, height as u32, DXGI_FORMAT_R8G8B8A8_UNORM);
                desc.BindFlags = D3D11_BIND_SHADER_RESOURCE.0 as u32;
                let texture = create_texture_with(device, &desc, &bytes).map_err(|e| format!("menu texture: {e}"))?;
                let mut view = None;
                // SAFETY: a view of our own texture in its own format.
                unsafe { device.CreateShaderResourceView(&texture, None, Some(&mut view)) }.map_err(failed("menu texture view"))?;
                self.textures.retain(|t| t.id != id);
                self.textures.push(Texture { id, texture, view: view.ok_or("no menu texture view")? });
            }
            Some([x, y]) => {
                let Some(target) = self.textures.iter().find(|t| t.id == id) else { return Ok(()) };
                let region = D3D11_BOX { left: x as u32, top: y as u32, front: 0, right: (x + width) as u32, bottom: (y + height) as u32, back: 1 };
                // SAFETY: `bytes` holds the region's rows, `width` texels of 4 bytes each, read
                // during the call; the region lies in our texture (egui's own patch).
                unsafe { context.UpdateSubresource(&target.texture, 0, Some(&region), bytes.as_ptr().cast(), (width * 4) as u32, 0) };
            }
        }
        Ok(())
    }
}

fn element(name: windows::core::PCSTR, format: DXGI_FORMAT, offset: u32) -> D3D11_INPUT_ELEMENT_DESC {
    D3D11_INPUT_ELEMENT_DESC { SemanticName: name, SemanticIndex: 0, Format: format, InputSlot: 0, AlignedByteOffset: offset, InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA, InstanceDataStepRate: 0 }
}

/// Writes `items` into a dynamic buffer bound as `bind`, made (again) when they do not fit.
fn write_dynamic<T: Copy>(device: &ID3D11Device, context: &ID3D11DeviceContext, slot: &mut Option<(ID3D11Buffer, usize)>, items: &[T], bind: D3D11_BIND_FLAG) -> Result<ID3D11Buffer> {
    if slot.as_ref().is_none_or(|(_, capacity)| *capacity < items.len()) {
        let capacity = items.len().next_power_of_two().max(1024);
        let desc = D3D11_BUFFER_DESC { ByteWidth: (capacity * size_of::<T>()) as u32, Usage: D3D11_USAGE_DYNAMIC, BindFlags: bind.0 as u32, CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32, ..Default::default() };
        let mut buffer = None;
        // SAFETY: valid descriptor and out-pointer.
        unsafe { device.CreateBuffer(&desc, None, Some(&mut buffer)) }.map_err(failed("menu buffer"))?;
        *slot = Some((buffer.ok_or("no menu buffer")?, capacity));
    }
    let (buffer, _) = slot.as_ref().ok_or("no menu buffer")?;
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    // SAFETY: a dynamic buffer of ours with room for `items`, mapped for writing and unmapped
    // before anything reads it.
    unsafe {
        context.Map(buffer, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut mapped)).map_err(failed("map"))?;
        std::ptr::copy_nonoverlapping(items.as_ptr(), mapped.pData.cast::<T>(), items.len());
        context.Unmap(buffer, 0);
    }
    Ok(buffer.clone())
}
