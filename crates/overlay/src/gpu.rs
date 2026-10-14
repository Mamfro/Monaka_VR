//! The D3D11 pieces the passes share: shaders from HLSL source, constant and structured buffers
//! written each frame, render targets on swapchain images, and premultiplied blending.

use crate::Result;
use monaka_channel::d3d12::compute::{blob_bytes, compile_for};
use std::marker::PhantomData;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::{Interface, PCSTR, s};

pub fn failed(what: &str) -> impl Fn(windows::core::Error) -> String + '_ {
    move |e| format!("{what}: {e}")
}

pub fn vertex_shader(device: &ID3D11Device, source: &str, name: &str, entry: PCSTR) -> Result<ID3D11VertexShader> {
    vertex_shader_and_code(device, source, name, entry).map(|(shader, _)| shader)
}

/// A vertex shader and its bytecode (for an input layout made against it).
pub fn vertex_shader_and_code(device: &ID3D11Device, source: &str, name: &str, entry: PCSTR) -> Result<(ID3D11VertexShader, Vec<u8>)> {
    let code = compile_for(source, name, entry, s!("vs_5_0"))?;
    let mut shader = None;
    // SAFETY: bytecode the compiler just made, and a local out-pointer.
    unsafe { device.CreateVertexShader(blob_bytes(&code), None, Some(&mut shader)) }.map_err(failed(name))?;
    Ok((shader.ok_or_else(|| format!("{name}: no vertex shader"))?, blob_bytes(&code).to_vec()))
}

pub fn pixel_shader(device: &ID3D11Device, source: &str, name: &str, entry: PCSTR) -> Result<ID3D11PixelShader> {
    let code = compile_for(source, name, entry, s!("ps_5_0"))?;
    let mut shader = None;
    // SAFETY: bytecode the compiler just made, and a local out-pointer.
    unsafe { device.CreatePixelShader(blob_bytes(&code), None, Some(&mut shader)) }.map_err(failed(name))?;
    shader.ok_or_else(|| format!("{name}: no pixel shader"))
}

/// A constant buffer holding one `T`, laid out as its HLSL `cbuffer`, written whole.
pub struct Constants<T: Copy> {
    pub buffer: ID3D11Buffer,
    layout: PhantomData<T>,
}

impl<T: Copy> Constants<T> {
    pub fn new(device: &ID3D11Device) -> Result<Self> {
        const { assert!(size_of::<T>().is_multiple_of(16), "D3D11 constant buffers are whole 16-byte registers") };
        let desc = D3D11_BUFFER_DESC { ByteWidth: size_of::<T>() as u32, Usage: D3D11_USAGE_DEFAULT, BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32, ..Default::default() };
        let mut buffer = None;
        // SAFETY: valid descriptor and out-pointer.
        unsafe { device.CreateBuffer(&desc, None, Some(&mut buffer)) }.map_err(failed("constant buffer"))?;
        Ok(Self { buffer: buffer.ok_or("no constant buffer")?, layout: PhantomData })
    }

    pub fn write(&self, context: &ID3D11DeviceContext, value: &T) {
        // SAFETY: `value` is a whole `T`, the buffer's exact size, read during the call.
        unsafe { context.UpdateSubresource(&self.buffer, 0, None, (value as *const T).cast(), 0, 0) };
    }
}

/// A structured buffer of `T` written each frame, read by shaders through its view; grown when a
/// frame has more than it holds.
pub struct Structured<T: Copy> {
    buffer: Option<(ID3D11Buffer, ID3D11ShaderResourceView)>,
    capacity: usize,
    layout: PhantomData<T>,
}

impl<T: Copy> Structured<T> {
    pub const fn new() -> Self {
        Self { buffer: None, capacity: 0, layout: PhantomData }
    }

    /// Writes `items` and gives the view reading them.
    pub fn write(&mut self, device: &ID3D11Device, context: &ID3D11DeviceContext, items: &[T]) -> Result<ID3D11ShaderResourceView> {
        if self.buffer.is_none() || items.len() > self.capacity {
            let capacity = items.len().next_power_of_two().max(256);
            let stride = size_of::<T>() as u32;
            let desc = D3D11_BUFFER_DESC {
                ByteWidth: stride * capacity as u32,
                Usage: D3D11_USAGE_DYNAMIC,
                BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
                MiscFlags: D3D11_RESOURCE_MISC_BUFFER_STRUCTURED.0 as u32,
                StructureByteStride: stride,
            };
            let mut buffer = None;
            // SAFETY: valid descriptor and out-pointer.
            unsafe { device.CreateBuffer(&desc, None, Some(&mut buffer)) }.map_err(failed("structured buffer"))?;
            let buffer = buffer.ok_or("no structured buffer")?;
            let view_desc = D3D11_SHADER_RESOURCE_VIEW_DESC {
                Format: DXGI_FORMAT_UNKNOWN,
                ViewDimension: windows::Win32::Graphics::Direct3D::D3D11_SRV_DIMENSION_BUFFER,
                Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 { Buffer: D3D11_BUFFER_SRV { Anonymous1: D3D11_BUFFER_SRV_0 { FirstElement: 0 }, Anonymous2: D3D11_BUFFER_SRV_1 { NumElements: capacity as u32 } } },
            };
            let mut view = None;
            // SAFETY: a view of our own structured buffer, all of it.
            unsafe { device.CreateShaderResourceView(&buffer, Some(&view_desc), Some(&mut view)) }.map_err(failed("structured view"))?;
            self.buffer = Some((buffer, view.ok_or("no structured view")?));
            self.capacity = capacity;
        }
        let (buffer, view) = self.buffer.as_ref().ok_or("no structured buffer")?;
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: a dynamic buffer of ours mapped for writing; at most its capacity is written, and
        // it is unmapped before anything reads it.
        unsafe {
            context.Map(buffer, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut mapped)).map_err(failed("map"))?;
            std::ptr::copy_nonoverlapping(items.as_ptr(), mapped.pData.cast::<T>(), items.len().min(self.capacity));
            context.Unmap(buffer, 0);
        }
        Ok(view.clone())
    }
}

/// Render target views of swapchain images, made once per image (a swapchain keeps a few).
#[derive(Default)]
pub struct Targets {
    views: Vec<(usize, ID3D11RenderTargetView, DXGI_FORMAT)>,
}

impl Targets {
    /// A render target view of `image`. A typeless image (some runtimes hand those out) is drawn as
    /// `format`, the format its swapchain was made with; a typed one as its own.
    pub fn view(&mut self, device: &ID3D11Device, image: &ID3D11Texture2D, format: DXGI_FORMAT) -> Result<ID3D11RenderTargetView> {
        let key = image.as_raw() as usize;
        if let Some((_, view, _)) = self.views.iter().find(|(k, _, _)| *k == key) {
            return Ok(view.clone());
        }
        let desc = monaka_channel::d3d::texture_desc(image);
        let typed = if is_typeless(desc.Format) { format } else { desc.Format };
        let view_desc = D3D11_RENDER_TARGET_VIEW_DESC { Format: typed, ViewDimension: D3D11_RTV_DIMENSION_TEXTURE2D, Anonymous: D3D11_RENDER_TARGET_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_RTV { MipSlice: 0 } } };
        let mut view = None;
        // SAFETY: a view of a live texture in a format of its family.
        unsafe { device.CreateRenderTargetView(image, Some(&view_desc), Some(&mut view)) }.map_err(failed("render target view"))?;
        let view = view.ok_or("no render target view")?;
        // Swapchains are made again now and then (a resize): views of images gone are let go.
        if self.views.len() >= 16 {
            self.views.remove(0);
        }
        self.views.push((key, view.clone(), typed));
        Ok(view)
    }

    /// The format `image`'s view draws in, once it has one.
    pub fn format_of(&self, image: &ID3D11Texture2D) -> Option<DXGI_FORMAT> {
        let key = image.as_raw() as usize;
        self.views.iter().find(|(k, _, _)| *k == key).map(|(_, _, format)| *format)
    }
}

fn is_typeless(format: DXGI_FORMAT) -> bool {
    matches!(format, DXGI_FORMAT_R8G8B8A8_TYPELESS | DXGI_FORMAT_B8G8R8A8_TYPELESS | DXGI_FORMAT_R10G10B10A2_TYPELESS | DXGI_FORMAT_R16G16B16A16_TYPELESS)
}

/// Blending for premultiplied colour over what is there. `keep_alpha`: the target's alpha is left
/// as it was (an eye image's alpha can carry where its pixels came from).
pub fn premultiplied(device: &ID3D11Device, keep_alpha: bool) -> Result<ID3D11BlendState> {
    let mask = if keep_alpha { (D3D11_COLOR_WRITE_ENABLE_RED.0 | D3D11_COLOR_WRITE_ENABLE_GREEN.0 | D3D11_COLOR_WRITE_ENABLE_BLUE.0) as u8 } else { D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8 };
    let mut desc = D3D11_BLEND_DESC::default();
    desc.RenderTarget[0] = D3D11_RENDER_TARGET_BLEND_DESC {
        BlendEnable: true.into(),
        SrcBlend: D3D11_BLEND_ONE,
        DestBlend: D3D11_BLEND_INV_SRC_ALPHA,
        BlendOp: D3D11_BLEND_OP_ADD,
        SrcBlendAlpha: D3D11_BLEND_ONE,
        DestBlendAlpha: D3D11_BLEND_INV_SRC_ALPHA,
        BlendOpAlpha: D3D11_BLEND_OP_ADD,
        RenderTargetWriteMask: mask,
    };
    let mut state = None;
    // SAFETY: valid descriptor and out-pointer.
    unsafe { device.CreateBlendState(&desc, Some(&mut state)) }.map_err(failed("blend state"))?;
    state.ok_or_else(|| "no blend state".into())
}

/// Solid triangles, both faces, no depth clipping beyond the near plane the shaders handle.
pub fn rasterizer(device: &ID3D11Device, scissor: bool) -> Result<ID3D11RasterizerState> {
    let desc = D3D11_RASTERIZER_DESC { FillMode: D3D11_FILL_SOLID, CullMode: D3D11_CULL_NONE, DepthClipEnable: true.into(), ScissorEnable: scissor.into(), ..Default::default() };
    let mut state = None;
    // SAFETY: valid descriptor and out-pointer.
    unsafe { device.CreateRasterizerState(&desc, Some(&mut state)) }.map_err(failed("rasterizer state"))?;
    state.ok_or_else(|| "no rasterizer state".into())
}

/// Binds `target` (whole, `width` by `height`) as the only render target.
pub fn bind_target(context: &ID3D11DeviceContext, target: &ID3D11RenderTargetView, width: u32, height: u32) {
    let viewport = D3D11_VIEWPORT { TopLeftX: 0.0, TopLeftY: 0.0, Width: width as f32, Height: height as f32, MinDepth: 0.0, MaxDepth: 1.0 };
    // SAFETY: a live view and a local viewport.
    unsafe {
        context.OMSetRenderTargets(Some(&[Some(target.clone())]), None);
        context.RSSetViewports(Some(&[viewport]));
    }
}

/// Unbinds the render target, so the image can be copied or presented next.
pub fn unbind_target(context: &ID3D11DeviceContext) {
    // SAFETY: binds nothing.
    unsafe { context.OMSetRenderTargets(None, None) };
}
