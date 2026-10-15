//! Depth reprojection on the GPU (D3D11): rendered frames redrawn from another camera, with a
//! second frame filling what the first could not see ([`Reprojector`], for alternate-eye rendering
//! with depth); a game's HUD as its own layer laid over each eye ([`HudLayer`]); and motion vectors
//! for temporal upscaling from depth and two cameras of one eye ([`MotionPass`]); a flat frame
//! resampled to the eye size ([`Scaler`]). The math is in
//! [`monaka_core::depth`].
//!
//! Runs on the game's own immediate context: the compute bindings it changes are saved and put
//! back around every call.
//!
//! On D3D12, [`warp12`]: both eyes warped from one rendered frame and its depth on the game's
//! queue (depth stereo), with the game's UI layer laid back over each eye; and [`reproject12`]:
//! a frame redrawn from another camera with a second frame filling its gaps, as [`Reprojector`]
//! does on D3D11 (alternate frame warping), with the same shader, constants and matrix
//! ([`source_matrix`]).

use monaka_channel::d3d;
use monaka_channel::d3d12::compute;
use std::fmt;
use std::marker::PhantomData;
use windows::Win32::Graphics::Direct3D::{D3D_SRV_DIMENSION_TEXTURE2D, ID3DBlob};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::PCSTR;

mod hud;
mod motion;
mod reproject;
mod scale;
pub mod reproject12;
pub mod warp12;
pub use hud::{HUD_SOURCE, HudLayer};
pub use motion::{EyeView, MOTION_SOURCE, MotionPass, ObjectMotion};
pub use reproject::{Frame, REPROJECT_SOURCE, Reprojector, View, source_matrix};
pub use scale::{SCALE_SOURCE, Scaler, fit};

#[derive(Debug)]
pub enum Error {
    Os(windows::core::Error),
    Shader(String),
    /// A texture these passes cannot read (format, multisampling or size).
    Unsupported(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Os(e) => write!(f, "{e}"),
            Error::Shader(message) => write!(f, "shader: {message}"),
            Error::Unsupported(what) => write!(f, "unsupported {what}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<windows::core::Error> for Error {
    fn from(e: windows::core::Error) -> Self {
        Error::Os(e)
    }
}

impl From<monaka_channel::Error> for Error {
    fn from(e: monaka_channel::Error) -> Self {
        match e {
            monaka_channel::Error::Os(e) => Error::Os(e),
            _ => Error::Unsupported("texture"),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// A copy target the shaders read.
struct Readable {
    texture: ID3D11Texture2D,
    view: ID3D11ShaderResourceView,
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
}

/// Compiles compute shader `entry` of `source` (cs_5_0) with the workspace's one HLSL compile
/// helper, which is API-neutral.
fn compile(source: &str, name: &str, entry: PCSTR) -> Result<ID3DBlob> {
    compute::compile(source, name, entry).map_err(Error::Shader)
}

/// A compute shader from `entry` of `source`.
fn compute_shader(device: &ID3D11Device, source: &str, name: &str, entry: PCSTR) -> Result<ID3D11ComputeShader> {
    let code = compile(source, name, entry)?;
    let mut shader = None;
    // SAFETY: bytecode the compiler just made, and a local out-pointer.
    unsafe { device.CreateComputeShader(compute::blob_bytes(&code), None, Some(&mut shader))? };
    Ok(shader.expect("shader created"))
}

/// A constant buffer holding one `T`, laid out as its HLSL `cbuffer`, written whole and bound at
/// b0 of the compute stage.
struct ConstantBuffer<T: Copy> {
    buffer: ID3D11Buffer,
    layout: PhantomData<T>,
}

impl<T: Copy> ConstantBuffer<T> {
    fn new(device: &ID3D11Device) -> Result<Self> {
        const { assert!(size_of::<T>().is_multiple_of(16), "D3D11 constant buffers are whole 16-byte registers") };
        let desc = D3D11_BUFFER_DESC {
            ByteWidth: size_of::<T>() as u32,
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
            ..Default::default()
        };
        let mut buffer = None;
        // SAFETY: valid descriptor and out-pointer.
        unsafe { device.CreateBuffer(&desc, None, Some(&mut buffer))? };
        Ok(Self { buffer: buffer.expect("buffer created"), layout: PhantomData })
    }

    /// Writes `value`, queued on `context`: each dispatch sees the value written before it.
    fn update(&self, context: &ID3D11DeviceContext, value: &T) {
        // SAFETY: `value` is a whole `T`, the buffer's exact size, and is read during the call.
        unsafe { context.UpdateSubresource(&self.buffer, 0, None, (value as *const T).cast(), 0, 0) };
    }

    fn bind(&self, context: &ID3D11DeviceContext) {
        // SAFETY: binds our own buffer; callers restore the game's bindings after.
        unsafe { context.CSSetConstantBuffers(0, Some(&[Some(self.buffer.clone())])) };
    }
}

/// A texture the shaders write and later read (unordered access and shader resource), with its
/// unordered access view in `format`.
fn uav_texture(device: &ID3D11Device, width: u32, height: u32, format: DXGI_FORMAT) -> Result<(ID3D11Texture2D, ID3D11UnorderedAccessView)> {
    let mut desc = d3d::simple_texture_desc(width, height, format);
    desc.BindFlags = (D3D11_BIND_UNORDERED_ACCESS.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32;
    let texture = d3d::create_texture(device, &desc)?;
    let mut uav = None;
    // SAFETY: a view of our own texture in its own format.
    unsafe { device.CreateUnorderedAccessView(&texture, Some(&uav_desc(format)), Some(&mut uav))? };
    Ok((texture, uav.expect("view created")))
}

/// A shader view of `texture` read as `format` (a format of its typeless family, so an sRGB or
/// typeless texture can be read as stored).
fn shader_view(device: &ID3D11Device, texture: &ID3D11Texture2D, format: DXGI_FORMAT) -> Result<ID3D11ShaderResourceView> {
    let mut view = None;
    // SAFETY: a view of a live texture with shader-resource binding, in a format of its family.
    unsafe { device.CreateShaderResourceView(texture, Some(&srv_desc(format)), Some(&mut view))? };
    Ok(view.expect("view created"))
}

/// Typeless storage and the shader view format for a colour texture, read as UNORM so an sRGB
/// back buffer is copied as stored.
fn color_formats(format: DXGI_FORMAT) -> Option<(DXGI_FORMAT, DXGI_FORMAT)> {
    match format {
        DXGI_FORMAT_R8G8B8A8_TYPELESS | DXGI_FORMAT_R8G8B8A8_UNORM | DXGI_FORMAT_R8G8B8A8_UNORM_SRGB => {
            Some((DXGI_FORMAT_R8G8B8A8_TYPELESS, DXGI_FORMAT_R8G8B8A8_UNORM))
        }
        DXGI_FORMAT_B8G8R8A8_TYPELESS | DXGI_FORMAT_B8G8R8A8_UNORM | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB => {
            Some((DXGI_FORMAT_B8G8R8A8_TYPELESS, DXGI_FORMAT_B8G8R8A8_UNORM))
        }
        DXGI_FORMAT_R10G10B10A2_TYPELESS | DXGI_FORMAT_R10G10B10A2_UNORM => Some((DXGI_FORMAT_R10G10B10A2_TYPELESS, DXGI_FORMAT_R10G10B10A2_UNORM)),
        _ => None,
    }
}

/// Typeless storage and the shader view format reading the depth plane of a depth texture.
fn depth_formats(format: DXGI_FORMAT) -> Option<(DXGI_FORMAT, DXGI_FORMAT)> {
    match format {
        DXGI_FORMAT_D24_UNORM_S8_UINT | DXGI_FORMAT_R24G8_TYPELESS => Some((DXGI_FORMAT_R24G8_TYPELESS, DXGI_FORMAT_R24_UNORM_X8_TYPELESS)),
        DXGI_FORMAT_D32_FLOAT | DXGI_FORMAT_R32_TYPELESS | DXGI_FORMAT_R32_FLOAT => Some((DXGI_FORMAT_R32_TYPELESS, DXGI_FORMAT_R32_FLOAT)),
        DXGI_FORMAT_D32_FLOAT_S8X24_UINT | DXGI_FORMAT_R32G8X24_TYPELESS => {
            Some((DXGI_FORMAT_R32G8X24_TYPELESS, DXGI_FORMAT_R32_FLOAT_X8X24_TYPELESS))
        }
        DXGI_FORMAT_D16_UNORM | DXGI_FORMAT_R16_TYPELESS | DXGI_FORMAT_R16_UNORM => Some((DXGI_FORMAT_R16_TYPELESS, DXGI_FORMAT_R16_UNORM)),
        _ => None,
    }
}

/// A private readable copy target shaped like `source`, reused while the shape holds.
fn readable(device: &ID3D11Device, slot: Option<Readable>, source: &D3D11_TEXTURE2D_DESC, formats: (DXGI_FORMAT, DXGI_FORMAT)) -> Result<Readable> {
    if let Some(existing) = slot
        && existing.width == source.Width
        && existing.height == source.Height
        && existing.format == source.Format
    {
        return Ok(existing);
    }
    let mut desc = d3d::simple_texture_desc(source.Width, source.Height, formats.0);
    desc.BindFlags = D3D11_BIND_SHADER_RESOURCE.0 as u32;
    let texture = d3d::create_texture(device, &desc)?;
    let view = shader_view(device, &texture, formats.1)?;
    Ok(Readable { texture, view, width: source.Width, height: source.Height, format: source.Format })
}

fn srv_desc(format: DXGI_FORMAT) -> D3D11_SHADER_RESOURCE_VIEW_DESC {
    D3D11_SHADER_RESOURCE_VIEW_DESC {
        Format: format,
        ViewDimension: D3D_SRV_DIMENSION_TEXTURE2D,
        Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_SRV { MostDetailedMip: 0, MipLevels: 1 } },
    }
}

fn uav_desc(format: DXGI_FORMAT) -> D3D11_UNORDERED_ACCESS_VIEW_DESC {
    D3D11_UNORDERED_ACCESS_VIEW_DESC {
        Format: format,
        ViewDimension: D3D11_UAV_DIMENSION_TEXTURE2D,
        Anonymous: D3D11_UNORDERED_ACCESS_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_UAV { MipSlice: 0 } },
    }
}

/// The compute bindings the passes change on the game's context.
struct SavedCompute {
    shader: Option<ID3D11ComputeShader>,
    resources: [Option<ID3D11ShaderResourceView>; 6],
    uavs: [Option<ID3D11UnorderedAccessView>; 3],
    constants: [Option<ID3D11Buffer>; 1],
}

impl SavedCompute {
    fn take(context: &ID3D11DeviceContext) -> Self {
        let mut saved = Self { shader: None, resources: Default::default(), uavs: Default::default(), constants: Default::default() };
        // SAFETY: reads the context's current bindings into locals.
        unsafe {
            context.CSGetShader(&mut saved.shader, None, None);
            context.CSGetShaderResources(0, Some(&mut saved.resources));
            context.CSGetUnorderedAccessViews(0, Some(&mut saved.uavs));
            context.CSGetConstantBuffers(0, Some(&mut saved.constants));
        }
        saved
    }

    fn restore(self, context: &ID3D11DeviceContext) {
        // SAFETY: puts back the bindings read in `take`.
        unsafe {
            context.CSSetShader(self.shader.as_ref(), None);
            context.CSSetShaderResources(0, Some(&self.resources));
            context.CSSetUnorderedAccessViews(0, 3, Some(self.uavs.as_ptr()), None);
            context.CSSetConstantBuffers(0, Some(&self.constants));
        }
    }
}

#[cfg(test)]
mod tests;
