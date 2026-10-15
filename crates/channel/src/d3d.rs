//! Small Direct3D helpers shared by the transports, the viewer and the tests.

use crate::Result;
use monaka_contract::protocol::Luid;
use windows::Win32::Foundation::LUID;
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_FLAG, D3D11_DEVICE_CONTEXT_IMMEDIATE, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE,
    D3D11_BIND_SHADER_RESOURCE, D3D11_SDK_VERSION, D3D11_SUBRESOURCE_DATA, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext,
    ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIAdapter, IDXGIAdapter1, IDXGIDevice, IDXGIFactory4, IDXGISwapChain};
use windows::core::Interface;

pub fn luid(value: LUID) -> Luid {
    Luid { low: value.LowPart, high: value.HighPart }
}

pub fn adapter_luid(device: &ID3D11Device) -> Result<Luid> {
    let dxgi: IDXGIDevice = device.cast()?;
    // SAFETY: COM calls on live interfaces.
    let desc = unsafe { dxgi.GetAdapter()?.GetDesc()? };
    Ok(luid(desc.AdapterLuid))
}

/// The DXGI adapter with this LUID.
pub fn find_adapter(adapter: Luid) -> Result<IDXGIAdapter1> {
    // SAFETY: COM factory calls.
    unsafe {
        let factory: IDXGIFactory4 = CreateDXGIFactory1()?;
        Ok(factory.EnumAdapterByLuid(LUID { LowPart: adapter.low, HighPart: adapter.high })?)
    }
}

/// The adapter's name, memory and driver version, for logs.
pub fn describe_adapter(adapter: &IDXGIAdapter1) -> String {
    // SAFETY: COM calls on a live adapter.
    let (desc, umd) = unsafe { (adapter.GetDesc1(), adapter.CheckInterfaceSupport(&IDXGIDevice::IID)) };
    let Ok(desc) = desc else { return "(no description)".into() };
    let end = desc.Description.iter().position(|&c| c == 0).unwrap_or(desc.Description.len());
    let name = String::from_utf16_lossy(&desc.Description[..end]);
    let driver = umd.map_or_else(|_| "unknown".into(), |v| format!("{}.{}.{}.{}", v >> 48, (v >> 32) & 0xffff, (v >> 16) & 0xffff, v & 0xffff));
    format!("{name}, {} MB, driver {driver}", desc.DedicatedVideoMemory / (1024 * 1024))
}

/// A hardware D3D11 device (feature level 11.0 or better) and its immediate context on `adapter`,
/// or on the default adapter.
pub fn create_device(adapter: Option<&IDXGIAdapter1>) -> Result<(ID3D11Device, ID3D11DeviceContext, D3D_FEATURE_LEVEL)> {
    let levels = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];
    let mut device = None;
    let mut context = None;
    let mut level = D3D_FEATURE_LEVEL::default();
    let adapter: Option<IDXGIAdapter> = adapter.map(|a| a.cast()).transpose()?;
    let driver = if adapter.is_some() { D3D_DRIVER_TYPE_UNKNOWN } else { windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE };
    // SAFETY: out-pointers are valid locals.
    unsafe {
        D3D11CreateDevice(
            adapter.as_ref(),
            driver,
            Default::default(),
            D3D11_CREATE_DEVICE_FLAG(0),
            Some(&levels),
            D3D11_SDK_VERSION,
            Some(&mut device),
            Some(&mut level),
            Some(&mut context),
        )?;
    }
    Ok((device.expect("device created"), context.expect("context created"), level))
}

pub fn texture_desc(texture: &ID3D11Texture2D) -> D3D11_TEXTURE2D_DESC {
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    // SAFETY: fills a local descriptor.
    unsafe { texture.GetDesc(&mut desc) };
    desc
}

/// The texture a render target view draws into.
pub fn view_texture(view: &windows::Win32::Graphics::Direct3D11::ID3D11RenderTargetView) -> Result<ID3D11Texture2D> {
    // SAFETY: plain queries on a live view.
    Ok(unsafe { view.GetResource() }?.cast::<ID3D11Texture2D>()?)
}

pub fn texture_device(texture: &ID3D11Texture2D) -> Result<ID3D11Device> {
    // SAFETY: COM call on a live interface.
    Ok(unsafe { texture.GetDevice() }?)
}

/// An immediate context belonging to `device`; the transports copy on it.
pub fn is_immediate_context_of(context: &ID3D11DeviceContext, device: &ID3D11Device) -> bool {
    // SAFETY: COM calls on live interfaces.
    unsafe { context.GetType() == D3D11_DEVICE_CONTEXT_IMMEDIATE && context.GetDevice().is_ok_and(|owner| &owner == device) }
}

/// A plain 2D texture: one mip, one layer, no multisampling.
pub fn simple_texture_desc(width: u32, height: u32, format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT) -> D3D11_TEXTURE2D_DESC {
    D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        ..Default::default()
    }
}

/// A swapchain's back buffer, its device and the device's immediate context.
pub fn swapchain_parts(chain: &IDXGISwapChain) -> Result<(ID3D11Texture2D, ID3D11Device, ID3D11DeviceContext)> {
    // SAFETY: COM calls on a live swapchain and its device.
    unsafe {
        let buffer: ID3D11Texture2D = chain.GetBuffer(0)?;
        let device: ID3D11Device = chain.GetDevice()?;
        let context = device.GetImmediateContext()?;
        Ok((buffer, device, context))
    }
}

/// A texture of `source`'s shape for keeping a copy of it on the GPU (sampled, no CPU access,
/// not shared): eye copies of a back buffer.
pub fn private_copy_desc(source: &D3D11_TEXTURE2D_DESC) -> D3D11_TEXTURE2D_DESC {
    D3D11_TEXTURE2D_DESC {
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
        ..*source
    }
}

pub fn create_texture(device: &ID3D11Device, desc: &D3D11_TEXTURE2D_DESC) -> Result<ID3D11Texture2D> {
    let mut texture = None;
    // SAFETY: valid descriptor and out-pointer.
    unsafe { device.CreateTexture2D(desc, None, Some(&mut texture))? };
    Ok(texture.expect("texture created"))
}

/// A texture filled from `pixels` (rows packed, [`bytes_per_texel`] each, 4 for a format it does
/// not know): for small fixed images and test inputs.
pub fn create_texture_with(device: &ID3D11Device, desc: &D3D11_TEXTURE2D_DESC, pixels: &[u8]) -> Result<ID3D11Texture2D> {
    let pitch = desc.Width as usize * bytes_per_texel(desc.Format).unwrap_or(4);
    if pixels.len() != pitch * desc.Height as usize {
        return Err(crate::Error::Incompatible("pixel data does not match the texture size"));
    }
    let data = D3D11_SUBRESOURCE_DATA { pSysMem: pixels.as_ptr().cast(), SysMemPitch: pitch as u32, SysMemSlicePitch: 0 };
    let mut texture = None;
    // SAFETY: the pixel data covers the whole texture (checked) and outlives the call.
    unsafe { device.CreateTexture2D(desc, Some(&data), Some(&mut texture))? };
    Ok(texture.expect("texture created"))
}

/// Bytes per texel of the formats read back to the CPU: the 8-bit RGBA family and other 32-bit
/// formats, two- and one-channel formats, the depth formats (D32S8 is 8: 32-bit depth, then the
/// stencil and padding), and the 64- and 128-bit colour formats. `None` for anything else
/// (block-compressed, planar video, unknown).
pub fn bytes_per_texel(format: DXGI_FORMAT) -> Option<usize> {
    Some(match format {
        DXGI_FORMAT_R32G32B32A32_TYPELESS | DXGI_FORMAT_R32G32B32A32_FLOAT | DXGI_FORMAT_R32G32B32A32_UINT | DXGI_FORMAT_R32G32B32A32_SINT => 16,
        DXGI_FORMAT_R16G16B16A16_TYPELESS
        | DXGI_FORMAT_R16G16B16A16_FLOAT
        | DXGI_FORMAT_R16G16B16A16_UNORM
        | DXGI_FORMAT_R16G16B16A16_UINT
        | DXGI_FORMAT_R16G16B16A16_SNORM
        | DXGI_FORMAT_R16G16B16A16_SINT
        | DXGI_FORMAT_R32G32_TYPELESS
        | DXGI_FORMAT_R32G32_FLOAT
        | DXGI_FORMAT_R32G32_UINT
        | DXGI_FORMAT_R32G32_SINT
        | DXGI_FORMAT_R32G8X24_TYPELESS
        | DXGI_FORMAT_D32_FLOAT_S8X24_UINT
        | DXGI_FORMAT_R32_FLOAT_X8X24_TYPELESS
        | DXGI_FORMAT_X32_TYPELESS_G8X24_UINT => 8,
        DXGI_FORMAT_R8G8B8A8_TYPELESS
        | DXGI_FORMAT_R8G8B8A8_UNORM
        | DXGI_FORMAT_R8G8B8A8_UNORM_SRGB
        | DXGI_FORMAT_R8G8B8A8_UINT
        | DXGI_FORMAT_R8G8B8A8_SNORM
        | DXGI_FORMAT_R8G8B8A8_SINT
        | DXGI_FORMAT_B8G8R8A8_TYPELESS
        | DXGI_FORMAT_B8G8R8A8_UNORM
        | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB
        | DXGI_FORMAT_B8G8R8X8_TYPELESS
        | DXGI_FORMAT_B8G8R8X8_UNORM
        | DXGI_FORMAT_B8G8R8X8_UNORM_SRGB
        | DXGI_FORMAT_R10G10B10A2_TYPELESS
        | DXGI_FORMAT_R10G10B10A2_UNORM
        | DXGI_FORMAT_R10G10B10A2_UINT
        | DXGI_FORMAT_R11G11B10_FLOAT
        | DXGI_FORMAT_R16G16_TYPELESS
        | DXGI_FORMAT_R16G16_FLOAT
        | DXGI_FORMAT_R16G16_UNORM
        | DXGI_FORMAT_R16G16_UINT
        | DXGI_FORMAT_R16G16_SNORM
        | DXGI_FORMAT_R16G16_SINT
        | DXGI_FORMAT_R32_TYPELESS
        | DXGI_FORMAT_D32_FLOAT
        | DXGI_FORMAT_R32_FLOAT
        | DXGI_FORMAT_R32_UINT
        | DXGI_FORMAT_R32_SINT
        | DXGI_FORMAT_R24G8_TYPELESS
        | DXGI_FORMAT_D24_UNORM_S8_UINT
        | DXGI_FORMAT_R24_UNORM_X8_TYPELESS
        | DXGI_FORMAT_X24_TYPELESS_G8_UINT => 4,
        DXGI_FORMAT_R8G8_TYPELESS
        | DXGI_FORMAT_R8G8_UNORM
        | DXGI_FORMAT_R8G8_UINT
        | DXGI_FORMAT_R8G8_SNORM
        | DXGI_FORMAT_R8G8_SINT
        | DXGI_FORMAT_R16_TYPELESS
        | DXGI_FORMAT_R16_FLOAT
        | DXGI_FORMAT_D16_UNORM
        | DXGI_FORMAT_R16_UNORM
        | DXGI_FORMAT_R16_UINT
        | DXGI_FORMAT_R16_SNORM
        | DXGI_FORMAT_R16_SINT => 2,
        DXGI_FORMAT_R8_TYPELESS | DXGI_FORMAT_R8_UNORM | DXGI_FORMAT_R8_UINT | DXGI_FORMAT_R8_SNORM | DXGI_FORMAT_R8_SINT | DXGI_FORMAT_A8_UNORM => 1,
        _ => return None,
    })
}

/// Reads subresource 0 (the top mip of the first layer) of a texture back to the CPU as tightly
/// packed rows: width, height, bytes per texel and the texels (for checks and probes; stalls).
/// Works on any texture of the device that is not multisampled, depth buffers included.
pub fn read_texels(device: &ID3D11Device, context: &ID3D11DeviceContext, texture: &ID3D11Texture2D) -> Result<(u32, u32, usize, Vec<u8>)> {
    let source = texture_desc(texture);
    let texel = bytes_per_texel(source.Format).ok_or(crate::Error::Incompatible("texel format not readable"))?;
    let mut desc = simple_texture_desc(source.Width, source.Height, source.Format);
    desc.Usage = D3D11_USAGE_STAGING;
    desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
    let staging = create_texture(device, &desc)?;
    let row_bytes = source.Width as usize * texel;
    let mut texels = vec![0u8; row_bytes * source.Height as usize];
    // SAFETY: copies subresource 0 into a staging texture of its shape (a whole resource when it
    // has only the one), then reads each mapped row within its pitch.
    unsafe {
        if source.MipLevels == 1 && source.ArraySize == 1 {
            context.CopyResource(&staging, texture);
        } else {
            context.CopySubresourceRegion(&staging, 0, 0, 0, 0, texture, 0, None);
        }
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
        for (y, row) in texels.chunks_exact_mut(row_bytes).enumerate() {
            let source_row = mapped.pData.cast::<u8>().add(y * mapped.RowPitch as usize);
            std::ptr::copy_nonoverlapping(source_row, row.as_mut_ptr(), row_bytes);
        }
        context.Unmap(&staging, 0);
    }
    Ok((source.Width, source.Height, texel, texels))
}

/// Reads a whole texture of 4-byte texels (8-bit RGBA and other 32-bit formats) back to the CPU as
/// tightly packed rows (for checks; stalls).
pub fn read_image(device: &ID3D11Device, context: &ID3D11DeviceContext, texture: &ID3D11Texture2D) -> Result<(u32, u32, Vec<u8>)> {
    match read_texels(device, context, texture)? {
        (width, height, 4, texels) => Ok((width, height, texels)),
        _ => Err(crate::Error::Incompatible("texels are not 4 bytes")),
    }
}

/// Reads one pixel of a texture of 4-byte texels back to the CPU (for checks and tests; stalls).
pub fn read_pixel(device: &ID3D11Device, context: &ID3D11DeviceContext, texture: &ID3D11Texture2D, x: u32, y: u32) -> Result<[u8; 4]> {
    let (width, height, texels) = read_image(device, context, texture)?;
    if x >= width || y >= height {
        return Err(crate::Error::Incompatible("pixel outside the texture"));
    }
    let at = (y * width + x) as usize * 4;
    Ok([texels[at], texels[at + 1], texels[at + 2], texels[at + 3]])
}

/// Copies a whole single-subresource texture into another of the same size and format family.
pub fn copy_texture(context: &ID3D11DeviceContext, target: &ID3D11Texture2D, source: &ID3D11Texture2D) {
    // SAFETY: GPU copy between live textures; D3D validates shape compatibility.
    unsafe { context.CopySubresourceRegion(target, 0, 0, 0, 0, source, 0, None) };
}

/// Submits queued GPU work now.
pub fn flush(context: &ID3D11DeviceContext) {
    // SAFETY: no preconditions.
    unsafe { context.Flush() };
}

#[cfg(test)]
mod tests;
