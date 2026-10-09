//! Stereo pairs between two D3D11 devices: two named NT textures with keyed mutexes plus a small
//! header. One writer, one reader. Neither ever waits: key [`KEY_UNREAD`] marks an unread pair,
//! and the writer reclaims it so an idle reader never stops production.
//!
//! Wire-compatible with `GpuStereoChannel` in `gpu_stereo_channel.h`.

use crate::d3d::{self, adapter_luid};
use crate::shared::{Create, NamedMutex, OwnedHandle, SharedBlock};
use crate::{ChannelName, Error, Result};
use monaka_core::protocol::{KEY_FREE, KEY_UNREAD, KEYED_MAGIC, KEYED_VERSION, KeyedHeader, Luid, dimensions_ok};
use windows::Win32::Foundation::{S_OK, WAIT_ABANDONED};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX,
    D3D11_RESOURCE_MISC_SHARED_NTHANDLE, ID3D11Device, ID3D11Device1, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_R8G8B8A8_TYPELESS, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB,
};
use windows::Win32::Graphics::Dxgi::{DXGI_SHARED_RESOURCE_READ, DXGI_SHARED_RESOURCE_WRITE, IDXGIKeyedMutex, IDXGIResource1};
use windows::core::{HRESULT, HSTRING, Interface};

/// What a reader learns before opening: enough to pick the producer's GPU.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Description {
    pub width: u32,
    pub height: u32,
    pub adapter: Luid,
}

/// One received pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pair {
    pub sequence: u64,
    pub tick: u64,
}

/// Reads the header without opening the textures.
pub fn describe(name: &ChannelName) -> Option<Description> {
    let mut block = SharedBlock::<KeyedHeader>::open(&name.header(), &name.header_lock(), false).ok()?;
    let header = block.lock(0).strict().ok()?;
    header.is_valid().then(|| Description { width: header.width, height: header.height, adapter: header.adapter })
}

/// The two eye textures and their keyed mutexes.
struct Eyes {
    textures: [ID3D11Texture2D; 2],
    keyed: [IDXGIKeyedMutex; 2],
    healthy: bool,
}

impl Eyes {
    fn new(textures: [ID3D11Texture2D; 2]) -> Result<Self> {
        let keyed = [textures[0].cast()?, textures[1].cast()?];
        Ok(Self { textures, keyed, healthy: true })
    }

    /// `AcquireSync` without waiting. Its success codes (timeout, abandoned) carry the answer, so
    /// the raw HRESULT is read through the vtable instead of the wrapper that folds them into Ok.
    fn acquire_one(&mut self, eye: usize, key: u64) -> bool {
        let mutex = &self.keyed[eye];
        // SAFETY: calls the interface's own AcquireSync slot on a live interface pointer.
        let result: HRESULT = unsafe { (Interface::vtable(mutex).AcquireSync)(Interface::as_raw(mutex), key, 0) };
        if result == HRESULT(WAIT_ABANDONED.0 as i32) {
            self.healthy = false;
        }
        result == S_OK
    }

    fn acquire(&mut self, key: u64) -> bool {
        if !self.acquire_one(0, key) {
            return false;
        }
        if !self.acquire_one(1, key) {
            // SAFETY: releases the key we just took on the left eye.
            let _ = unsafe { self.keyed[0].ReleaseSync(key) };
            return false;
        }
        true
    }

    fn release(&mut self, key: u64) -> bool {
        // SAFETY: releases keys this side holds.
        let ok = unsafe { self.keyed[0].ReleaseSync(key).is_ok() & self.keyed[1].ReleaseSync(key).is_ok() };
        self.healthy &= ok;
        self.healthy
    }
}

fn rgba8(format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT) -> bool {
    [DXGI_FORMAT_R8G8B8A8_TYPELESS, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB].contains(&format)
}

/// A caller's own eye texture: on our device, our size, RGBA8, plain.
fn compatible(texture: &ID3D11Texture2D, device: &ID3D11Device, width: u32, height: u32) -> bool {
    let desc = d3d::texture_desc(texture);
    d3d::texture_device(texture).is_ok_and(|owner| &owner == device)
        && desc.Width == width
        && desc.Height == height
        && desc.ArraySize == 1
        && desc.MipLevels == 1
        && desc.SampleDesc.Count == 1
        && rgba8(desc.Format)
}

pub struct KeyedWriter {
    header: SharedBlock<KeyedHeader>,
    device: ID3D11Device,
    eyes: Eyes,
    _handles: [OwnedHandle; 2],
    width: u32,
    height: u32,
    sequence: u64,
}

impl KeyedWriter {
    pub fn create(name: &ChannelName, device: &ID3D11Device, width: u32, height: u32) -> Result<Self> {
        if !dimensions_ok(width, height) {
            return Err(Error::Incompatible("eye size"));
        }
        let adapter = adapter_luid(device)?;
        let mut header = SharedBlock::<KeyedHeader>::create(&name.header(), &name.header_lock(), Create::Exclusive)?;
        let mut record = header.lock(0).strict()?;

        let mut desc = d3d::simple_texture_desc(width, height, DXGI_FORMAT_R8G8B8A8_UNORM);
        desc.BindFlags = (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32;
        desc.MiscFlags = (D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0 | D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX.0) as u32;
        let textures = [d3d::create_texture(device, &desc)?, d3d::create_texture(device, &desc)?];
        let share = |eye: usize| -> Result<OwnedHandle> {
            let resource: IDXGIResource1 = textures[eye].cast()?;
            let access = (DXGI_SHARED_RESOURCE_READ | DXGI_SHARED_RESOURCE_WRITE).0;
            // SAFETY: creates a named NT handle for a shareable texture we own.
            let handle = unsafe { resource.CreateSharedHandle(None, access, &HSTRING::from(name.eye_texture(eye))) }?;
            Ok(OwnedHandle::new(handle).expect("valid shared handle"))
        };
        let handles = [share(0)?, share(1)?];

        *record = KeyedHeader { magic: KEYED_MAGIC, version: KEYED_VERSION, width, height, adapter, sequence: 0, tick: 0 };
        drop(record);
        Ok(Self { header, device: device.clone(), eyes: Eyes::new(textures)?, _handles: handles, width, height, sequence: 0 })
    }

    /// The sequence the next successful publish will carry, so its pose record can go out first.
    pub fn next_sequence(&self) -> u64 {
        self.sequence + 1
    }

    /// Copies both eyes into the channel. `Ok(false)` means skipped (lock or keys busy).
    pub fn publish(&mut self, context: &ID3D11DeviceContext, eyes: [&ID3D11Texture2D; 2], tick: u64) -> Result<bool> {
        if !self.eyes.healthy {
            return Err(Error::Unhealthy);
        }
        if !d3d::is_immediate_context_of(context, &self.device)
            || !eyes.iter().all(|t| compatible(t, &self.device, self.width, self.height))
        {
            return Err(Error::Incompatible("context or eye textures"));
        }
        let Ok(mut record) = self.header.lock(0).strict() else { return Ok(false) };
        // An unread pair (KEY_UNREAD) is reclaimed, so the reader always gets the latest image.
        let owned = self.eyes.acquire(KEY_FREE) || (self.eyes.healthy && record.sequence != 0 && self.eyes.acquire(KEY_UNREAD));
        if !owned {
            return if self.eyes.healthy { Ok(false) } else { Err(Error::Unhealthy) };
        }
        // SAFETY: copies between same-shaped textures on our device while we hold both keys.
        unsafe {
            for (channel, source) in self.eyes.textures.iter().zip(eyes) {
                context.CopyResource(channel, source);
            }
            context.Flush();
        }
        if !self.eyes.release(KEY_UNREAD) {
            return Err(Error::Unhealthy);
        }
        record.sequence += 1;
        record.tick = tick;
        self.sequence = record.sequence;
        Ok(true)
    }
}

/// A [`KeyedWriter`] made at the first frame's size. The viewer opens the channel at that size
/// and keeps it, so frames of any other size cannot be published: [`LazyWriter::get`] refuses
/// them (a producer that switches resolution does it before its first frame).
pub struct LazyWriter {
    channel: ChannelName,
    writer: Option<KeyedWriter>,
    failed: bool,
    refused: Option<(u32, u32)>,
}

/// What [`LazyWriter::get`] gives: the writer when this frame can be published, and a line worth
/// logging (the channel was made, could not be, or a frame size is refused), once each.
pub struct Lazy<'a> {
    pub writer: Option<&'a mut KeyedWriter>,
    pub note: Option<String>,
}

impl<'a> Lazy<'a> {
    /// The writer, after handing any note to `log`.
    pub fn ready(self, log: impl FnOnce(&str)) -> Option<&'a mut KeyedWriter> {
        if let Some(note) = &self.note {
            log(note);
        }
        self.writer
    }
}

impl LazyWriter {
    pub fn new(channel: ChannelName) -> Self {
        Self { channel, writer: None, failed: false, refused: None }
    }

    pub fn channel(&self) -> &ChannelName {
        &self.channel
    }

    /// The channel's eye size, once made.
    pub fn size(&self) -> Option<(u32, u32)> {
        self.writer.as_ref().map(|w| (w.width, w.height))
    }

    /// The writer, once [`LazyWriter::get`] made it.
    pub fn made(&mut self) -> Option<&mut KeyedWriter> {
        self.writer.as_mut()
    }

    /// The writer for a `width` x `height` frame on `device`, making the channel at the first one.
    pub fn get(&mut self, device: &ID3D11Device, width: u32, height: u32) -> Lazy<'_> {
        if self.failed {
            return Lazy { writer: None, note: None };
        }
        let mut note = None;
        if self.writer.is_none() {
            match KeyedWriter::create(&self.channel, device, width, height) {
                Ok(writer) => {
                    note = Some(format!("GPU channel {} created at {width}x{height}", self.channel.as_str()));
                    self.writer = Some(writer);
                }
                Err(e) => {
                    self.failed = true;
                    return Lazy { writer: None, note: Some(format!("GPU channel unavailable: {e}")) };
                }
            }
        }
        let writer = self.writer.as_mut().expect("made above");
        if (writer.width, writer.height) != (width, height) {
            let first = self.refused.replace((width, height)) != Some((width, height));
            let note = first.then(|| {
                format!("frame size changed to {width}x{height}; the viewer keeps the channel's {}x{}, so those frames are not published", writer.width, writer.height)
            });
            return Lazy { writer: None, note };
        }
        Lazy { writer: Some(writer), note }
    }
}

pub struct KeyedReader {
    header: SharedBlock<KeyedHeader>,
    _lease: NamedMutex,
    device: ID3D11Device,
    eyes: Eyes,
    width: u32,
    height: u32,
}

impl KeyedReader {
    /// Opens the channel on `device`, which must be on the producer's adapter. Only one reader may
    /// hold a channel.
    pub fn open(name: &ChannelName, device: &ID3D11Device) -> Result<Self> {
        let described = describe(name).ok_or(Error::Incompatible("no valid keyed header"))?;
        let adapter = adapter_luid(device)?;
        if adapter != described.adapter {
            return Err(Error::Incompatible("reader is on a different GPU than the producer"));
        }
        let (lease, taken) = NamedMutex::create(&name.reader_lease())?;
        if taken {
            return Err(Error::ReaderTaken);
        }
        let mut header = SharedBlock::<KeyedHeader>::open(&name.header(), &name.header_lock(), false)?;
        let record = header.lock(0).strict()?;
        let Description { width, height, .. } = described;
        if !record.is_valid() || record.width != width || record.height != height || record.adapter != adapter {
            return Err(Error::Incompatible("keyed header changed while opening"));
        }
        let device1: ID3D11Device1 = device.cast()?;
        let open = |eye: usize| -> Result<ID3D11Texture2D> {
            let access = (DXGI_SHARED_RESOURCE_READ | DXGI_SHARED_RESOURCE_WRITE).0;
            // SAFETY: opens a named shared texture on our device.
            let texture: ID3D11Texture2D = unsafe { device1.OpenSharedResourceByName(&HSTRING::from(name.eye_texture(eye)), access) }?;
            let desc = d3d::texture_desc(&texture);
            let plain = desc.Width == width && desc.Height == height && desc.Format == DXGI_FORMAT_R8G8B8A8_UNORM;
            if plain && desc.ArraySize == 1 && desc.MipLevels == 1 && desc.SampleDesc.Count == 1 {
                Ok(texture)
            } else {
                Err(Error::Incompatible("shared eye texture shape"))
            }
        };
        let textures = [open(0)?, open(1)?];
        drop(record);
        Ok(Self { header, _lease: lease, device: device.clone(), eyes: Eyes::new(textures)?, width, height })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Copies the newest pair into `eyes` if it is newer than `seen`. `Ok(None)`: nothing new, or busy.
    pub fn read(&mut self, context: &ID3D11DeviceContext, eyes: [&ID3D11Texture2D; 2], seen: u64) -> Result<Option<Pair>> {
        if !self.eyes.healthy {
            return Err(Error::Unhealthy);
        }
        if !d3d::is_immediate_context_of(context, &self.device)
            || !eyes.iter().all(|t| compatible(t, &self.device, self.width, self.height))
        {
            return Err(Error::Incompatible("context or eye textures"));
        }
        let Ok(record) = self.header.lock(0).strict() else { return Ok(None) };
        let (sequence, tick) = (record.sequence, record.tick);
        if sequence == 0 || sequence == seen || !self.eyes.acquire(KEY_UNREAD) {
            return if self.eyes.healthy { Ok(None) } else { Err(Error::Unhealthy) };
        }
        // SAFETY: copies between same-shaped textures while we hold both keys.
        unsafe {
            for (target, channel) in eyes.into_iter().zip(&self.eyes.textures) {
                context.CopyResource(target, channel);
            }
            context.Flush();
        }
        if !self.eyes.release(KEY_FREE) {
            return Err(Error::Unhealthy);
        }
        Ok(Some(Pair { sequence, tick }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::d3d::{create_device, create_texture, read_pixel, simple_texture_desc};
    use windows::Win32::Graphics::Direct3D11::{D3D11_BIND_RENDER_TARGET, ID3D11DeviceContext, ID3D11RenderTargetView};

    fn filled(device: &ID3D11Device, context: &ID3D11DeviceContext, rgba: [f32; 4]) -> ID3D11Texture2D {
        let mut desc = simple_texture_desc(64, 32, DXGI_FORMAT_R8G8B8A8_UNORM);
        desc.BindFlags = D3D11_BIND_RENDER_TARGET.0 as u32;
        let texture = create_texture(device, &desc).unwrap();
        let mut view: Option<ID3D11RenderTargetView> = None;
        unsafe {
            device.CreateRenderTargetView(&texture, None, Some(&mut view)).unwrap();
            context.ClearRenderTargetView(view.as_ref().unwrap(), &rgba);
        }
        texture
    }

    #[test]
    fn pairs_cross_between_devices() {
        let name = ChannelName::new(format!(r"Local\monaka-test-keyed-{}", std::process::id())).unwrap();
        let (writer_device, writer_context, _) = create_device(None).unwrap();
        let mut writer = KeyedWriter::create(&name, &writer_device, 64, 32).unwrap();
        assert!(KeyedWriter::create(&name, &writer_device, 64, 32).is_err(), "a second producer must be refused");

        let described = describe(&name).unwrap();
        let adapter = crate::d3d::find_adapter(described.adapter).unwrap();
        let (reader_device, reader_context, _) = create_device(Some(&adapter)).unwrap();
        let mut reader = KeyedReader::open(&name, &reader_device).unwrap();
        assert!(matches!(KeyedReader::open(&name, &reader_device), Err(Error::ReaderTaken)));

        let target = |_| filled(&reader_device, &reader_context, [0.0; 4]);
        let targets: [ID3D11Texture2D; 2] = std::array::from_fn(target);
        let targets = [&targets[0], &targets[1]];
        assert_eq!(reader.read(&reader_context, targets, 0).unwrap(), None);

        let left = filled(&writer_device, &writer_context, [1.0, 0.0, 0.0, 1.0]);
        let right = filled(&writer_device, &writer_context, [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(writer.next_sequence(), 1);
        assert!(writer.publish(&writer_context, [&left, &right], 42).unwrap());
        // An unread pair is reclaimed rather than blocking the writer.
        assert!(writer.publish(&writer_context, [&left, &right], 43).unwrap());

        let pair = reader.read(&reader_context, targets, 0).unwrap().unwrap();
        assert_eq!(pair, Pair { sequence: 2, tick: 43 });
        assert_eq!(read_pixel(&reader_device, &reader_context, targets[0], 5, 5).unwrap(), [255, 0, 0, 255]);
        assert_eq!(read_pixel(&reader_device, &reader_context, targets[1], 5, 5).unwrap(), [0, 0, 255, 255]);
        assert_eq!(reader.read(&reader_context, targets, pair.sequence).unwrap(), None);
    }
}
