//! An image the producer shares for the debug views (the game's depth, one of its buffers;
//! [`monaka_contract::debug_draw::DebugDepth`]): one texture by name behind a keyed mutex, written by
//! the producer, read by the viewer. Neither waits: a writer finding the texture being read skips
//! that frame, and an unread image is taken back by the next write, so the reader always gets the
//! latest.

use crate::d3d;
use crate::shared::OwnedHandle;
use crate::{Error, Result};
use windows::Win32::Foundation::S_OK;
use windows::Win32::Graphics::Direct3D11::{D3D11_BIND_SHADER_RESOURCE, D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX, D3D11_RESOURCE_MISC_SHARED_NTHANDLE, ID3D11Device, ID3D11Device1, ID3D11DeviceContext, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;
use windows::Win32::Graphics::Dxgi::{DXGI_SHARED_RESOURCE_READ, DXGI_SHARED_RESOURCE_WRITE, IDXGIKeyedMutex, IDXGIResource1};
use windows::core::{HRESULT, HSTRING, Interface};

#[cfg(test)]
mod tests;

/// Key meaning "the writer may fill the image".
const KEY_FREE: u64 = 0;
/// Key meaning "an unread image is waiting".
const KEY_UNREAD: u64 = 1;

/// `AcquireSync` without waiting: whether `key` was taken.
fn acquire(mutex: &IDXGIKeyedMutex, key: u64) -> bool {
    // SAFETY: calls the interface's own AcquireSync slot on a live interface pointer; its success
    // codes (timeout, abandoned) carry the answer, so the raw HRESULT is read.
    let result: HRESULT = unsafe { (Interface::vtable(mutex).AcquireSync)(Interface::as_raw(mutex), key, 0) };
    result == S_OK
}

fn release(mutex: &IDXGIKeyedMutex, key: u64) {
    // SAFETY: releases a key this side holds.
    let _ = unsafe { mutex.ReleaseSync(key) };
}

pub struct ImageWriter {
    texture: ID3D11Texture2D,
    keyed: IDXGIKeyedMutex,
    _handle: OwnedHandle,
    pub width: u32,
    pub height: u32,
    /// A depth was written and not yet taken back, so the next write may reclaim it.
    written: bool,
}

impl ImageWriter {
    /// Makes the shared texture (`width` by `height`, `format`) on `device`, named `name`.
    pub fn create(name: &str, device: &ID3D11Device, width: u32, height: u32, format: DXGI_FORMAT) -> Result<Self> {
        let mut desc = d3d::simple_texture_desc(width, height, format);
        desc.BindFlags = D3D11_BIND_SHADER_RESOURCE.0 as u32;
        desc.MiscFlags = (D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0 | D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX.0) as u32;
        let texture = d3d::create_texture(device, &desc)?;
        let resource: IDXGIResource1 = texture.cast()?;
        let access = (DXGI_SHARED_RESOURCE_READ | DXGI_SHARED_RESOURCE_WRITE).0;
        // SAFETY: creates a named NT handle for a shareable texture we own.
        let handle = unsafe { resource.CreateSharedHandle(None, access, &HSTRING::from(name)) }?;
        let handle = OwnedHandle::new(handle).ok_or(Error::Incompatible("shared image handle"))?;
        Ok(Self { keyed: texture.cast()?, texture, _handle: handle, width, height, written: false })
    }

    /// Copies `source` (this texture's size and format, on its device) in; false when the viewer
    /// is reading it (this frame's image is skipped).
    pub fn write(&mut self, context: &ID3D11DeviceContext, source: &ID3D11Texture2D) -> bool {
        let owned = acquire(&self.keyed, KEY_FREE) || (self.written && acquire(&self.keyed, KEY_UNREAD));
        if !owned {
            return false;
        }
        // SAFETY: copies between same-shaped textures on one device while we hold the key.
        unsafe { context.CopyResource(&self.texture, source) };
        release(&self.keyed, KEY_UNREAD);
        self.written = true;
        true
    }
}

pub struct ImageReader {
    texture: ID3D11Texture2D,
    keyed: IDXGIKeyedMutex,
    pub width: u32,
    pub height: u32,
    pub format: DXGI_FORMAT,
}

impl ImageReader {
    /// Opens the shared texture named `name` on `device` (the producer's GPU).
    pub fn open(name: &str, device: &ID3D11Device) -> Result<Self> {
        let device1: ID3D11Device1 = device.cast()?;
        let access = (DXGI_SHARED_RESOURCE_READ | DXGI_SHARED_RESOURCE_WRITE).0;
        // SAFETY: opens a named shared texture on our device.
        let texture: ID3D11Texture2D = unsafe { device1.OpenSharedResourceByName(&HSTRING::from(name), access) }?;
        let desc = d3d::texture_desc(&texture);
        if desc.SampleDesc.Count != 1 || desc.ArraySize != 1 {
            return Err(Error::Incompatible("shared image shape"));
        }
        Ok(Self { keyed: texture.cast()?, texture, width: desc.Width, height: desc.Height, format: desc.Format })
    }

    /// Copies the latest image into `into` (this size and format, on the reader's device) if one
    /// was written since the last read; false otherwise.
    pub fn read(&mut self, context: &ID3D11DeviceContext, into: &ID3D11Texture2D) -> bool {
        if !acquire(&self.keyed, KEY_UNREAD) {
            return false;
        }
        // SAFETY: copies between same-shaped textures on one device while we hold the key.
        unsafe { context.CopyResource(into, &self.texture) };
        release(&self.keyed, KEY_FREE);
        true
    }
}
