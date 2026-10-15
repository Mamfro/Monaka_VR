//! The fence channel ([`crate::fence`]) written from a D3D11 game: the same header, slots and
//! shared fences, made with D3D11.4's shareable textures and fences, so the reader cannot tell the
//! producer's API. It takes the game's own back buffer format (10-bit included); typeless and sRGB
//! images travel in their family's plain format ([`fence_format_of`]), copied in as stored.
//!
//! A producer may send each eye's depth with a pair ([`FenceProducer11::publish_with_depth`]), as
//! the D3D12 producer does: every slot then has a depth texture per eye, made on the first pair
//! with depth and copied with the images, so the `produced` fence covers both.
//!
//! [`LazyProducer11`] makes the channel at the first frame's size and format, for a publisher
//! that learns them only then.

use crate::d3d::{self, adapter_luid};
use crate::fence::{HeaderWriter, Shared};
use crate::shared::OwnedHandle;
use crate::{ChannelName, Error, Result};
use monaka_contract::protocol::{DepthParams, DepthSlot, FENCE_SLOTS, dimensions_ok, dxgi_format, fence_format_of};
use windows::Win32::Foundation::{GENERIC_ALL, HANDLE};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_BOX, D3D11_FENCE_FLAG_SHARED, D3D11_RESOURCE_MISC_SHARED,
    D3D11_RESOURCE_MISC_SHARED_NTHANDLE, ID3D11Device, ID3D11Device5, ID3D11DeviceContext, ID3D11DeviceContext4, ID3D11Fence,
    ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;
use windows::Win32::Graphics::Dxgi::{DXGI_SHARED_RESOURCE_READ, DXGI_SHARED_RESOURCE_WRITE, IDXGIResource1};
use windows::core::{Interface, PCWSTR};

/// One eye's depth for [`FenceProducer11::publish_with_depth`].
#[derive(Clone, Copy)]
pub struct EyeDepth<'a> {
    /// 32-bit float or 16-bit depth ([`depth_copyable`]), not multisampled, on the producer's
    /// device. Its first subresource is copied whole.
    pub texture: &'a ID3D11Texture2D,
    /// The part of it that lines up with the eye image: left, top, width, height in texels.
    /// `None`: all of it.
    pub rect: Option<[u32; 4]>,
}

/// Both eyes' depth with one pair, and how it maps to distance.
#[derive(Clone, Copy)]
pub struct Depth<'a> {
    pub eyes: [EyeDepth<'a>; 2],
    pub params: DepthParams,
}

/// The fence channel's depth format for a D3D11 depth texture of `format`: R32 or R16, copied as
/// stored. Only one-plane formats: D3D11 cannot copy the depth plane out of a depth-stencil format.
fn depth_format(format: u32) -> Option<u32> {
    use dxgi_format::*;
    match format {
        R32_TYPELESS | D32_FLOAT | R32_FLOAT => Some(R32_TYPELESS),
        R16_TYPELESS | D16_UNORM | R16_UNORM => Some(R16_TYPELESS),
        _ => None,
    }
}

/// Whether `texture` can go to [`FenceProducer11::publish_with_depth`]: a plain 2D texture of
/// 32-bit float or 16-bit depth, without stencil.
pub fn depth_copyable(texture: &ID3D11Texture2D) -> bool {
    let desc = d3d::texture_desc(texture);
    desc.SampleDesc.Count == 1 && desc.ArraySize == 1 && dimensions_ok(desc.Width, desc.Height) && depth_format(desc.Format.0 as u32).is_some()
}

/// The shared depth textures of every slot, one per eye.
struct DepthSlots {
    textures: [[ID3D11Texture2D; 2]; FENCE_SLOTS],
    format: u32,
    width: u32,
    height: u32,
    _handles: Vec<OwnedHandle>,
}

/// [`FENCE_SLOTS`] pairs of shareable `width` x `height` textures in `format`, bound as `bind`, and
/// their shared handle values (the handles kept in `handles`).
/// `mono`: one texture a slot, published as both eyes' ([`FenceProducer11::create_mono`]).
fn shared_slots(device: &ID3D11Device, width: u32, height: u32, format: DXGI_FORMAT, bind: u32, mono: bool, handles: &mut Vec<OwnedHandle>) -> Result<([[u64; 2]; FENCE_SLOTS], [[ID3D11Texture2D; 2]; FENCE_SLOTS])> {
    let mut desc = d3d::simple_texture_desc(width, height, format);
    desc.BindFlags = bind;
    desc.MiscFlags = (D3D11_RESOURCE_MISC_SHARED.0 | D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0) as u32;
    let mut published = [[0u64; 2]; FENCE_SLOTS];
    let mut slots = Vec::with_capacity(FENCE_SLOTS);
    for slot in published.iter_mut() {
        let left = d3d::create_texture(device, &desc)?;
        let right = if mono { left.clone() } else { d3d::create_texture(device, &desc)? };
        let pair = [left, right];
        for (eye, texture) in pair.iter().enumerate() {
            if mono && eye == 1 {
                slot[1] = slot[0];
                continue;
            }
            let resource: IDXGIResource1 = texture.cast()?;
            let access = (DXGI_SHARED_RESOURCE_READ | DXGI_SHARED_RESOURCE_WRITE).0;
            // SAFETY: an unnamed NT handle for a shareable texture we own.
            slot[eye] = keep(handles, unsafe { resource.CreateSharedHandle(None, access, PCWSTR::null()) }?)?;
        }
        slots.push(pair);
    }
    Ok((published, slots.try_into().map_err(|_| Error::Incompatible("slots"))?))
}

fn same_box(a: &D3D11_BOX, b: &D3D11_BOX) -> bool {
    (a.left, a.top, a.front, a.right, a.bottom, a.back) == (b.left, b.top, b.front, b.right, b.bottom, b.back)
}

/// Keeps `handle` open in `handles`; its value as the header publishes it.
fn keep(handles: &mut Vec<OwnedHandle>, handle: HANDLE) -> Result<u64> {
    let value = handle.0 as usize as u64;
    handles.push(OwnedHandle::new(handle).ok_or(Error::Incompatible("shared handle"))?);
    Ok(value)
}

/// Whether `rect` (left, top, width, height) is a non-empty part of a `width` x `height` texture.
fn rect_inside(rect: [u32; 4], width: u32, height: u32) -> bool {
    let [left, top, w, h] = rect;
    w > 0 && h > 0 && left.checked_add(w).is_some_and(|right| right <= width) && top.checked_add(h).is_some_and(|bottom| bottom <= height)
}

/// The depth textures' format and size for `depth`, and each eye's rect: both eyes alike and
/// copyable, the rects inside them, the planes valid.
fn depth_shape(depth: &Depth<'_>, device: &ID3D11Device) -> Result<(u32, u32, u32, [[u32; 4]; 2])> {
    if !depth.params.is_valid() {
        return Err(Error::Incompatible("depth near and far"));
    }
    let mut shape = None;
    let mut rects = [[0; 4]; 2];
    for (eye, rect) in depth.eyes.iter().zip(&mut rects) {
        if !depth_copyable(eye.texture) || d3d::texture_device(eye.texture).ok().as_ref() != Some(device) {
            return Err(Error::Incompatible("eye depth format"));
        }
        let desc = d3d::texture_desc(eye.texture);
        let this = (depth_format(desc.Format.0 as u32).expect("checked by depth_copyable"), desc.Width, desc.Height);
        if shape.is_some_and(|other| other != this) {
            return Err(Error::Incompatible("eye depths differ in size or format"));
        }
        shape = Some(this);
        *rect = eye.rect.unwrap_or([0, 0, this.1, this.2]);
        if !rect_inside(*rect, this.1, this.2) {
            return Err(Error::Incompatible("eye depth rect"));
        }
    }
    let (format, width, height) = shape.expect("two eyes");
    Ok((format, width, height, rects))
}

pub struct FenceProducer11 {
    header: HeaderWriter,
    device: ID3D11Device,
    textures: [[ID3D11Texture2D; 2]; FENCE_SLOTS],
    produced: ID3D11Fence,
    consumed: ID3D11Fence,
    _handles: Vec<OwnedHandle>,
    width: u32,
    height: u32,
    /// The channel's format (plain UNORM; images of its family are copied in).
    format: u32,
    sequence: u64,
    /// The depth textures, once a pair has come with depth.
    depth: Option<DepthSlots>,
    with_depth: u64,
    /// One image a pair ([`FenceProducer11::create_mono`]).
    mono: bool,
}

impl FenceProducer11 {
    /// Claims `name` and creates the shared slots on `device` (D3D11.4 or later, for shared fences),
    /// for images of `format`'s family ([`fence_format_of`]).
    pub fn create(name: &ChannelName, device: &ID3D11Device, width: u32, height: u32, format: DXGI_FORMAT) -> Result<Self> {
        Self::create_with(name, device, width, height, format, false)
    }

    /// [`FenceProducer11::create`] for one image a pair (a HUD panel): a texture a slot, published
    /// as both eyes', so a reader that copies both eyes still gets the image in each. Published with
    /// [`FenceProducer11::publish_one`].
    pub fn create_mono(name: &ChannelName, device: &ID3D11Device, width: u32, height: u32, format: DXGI_FORMAT) -> Result<Self> {
        Self::create_with(name, device, width, height, format, true)
    }

    fn create_with(name: &ChannelName, device: &ID3D11Device, width: u32, height: u32, format: DXGI_FORMAT, mono: bool) -> Result<Self> {
        let channel_format = fence_format_of(format.0 as u32);
        let Some(channel_format) = channel_format.filter(|_| dimensions_ok(width, height)) else { return Err(Error::Incompatible("eye size or format")) };
        let device5: ID3D11Device5 = device.cast()?;
        let adapter = adapter_luid(device)?;
        let mut header = HeaderWriter::claim(name)?;
        let mut handles = Vec::new();
        let bind = (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32;
        let (published, textures) = shared_slots(device, width, height, DXGI_FORMAT(channel_format as i32), bind, mono, &mut handles)?;
        let fence = || -> Result<ID3D11Fence> {
            let mut fence = None;
            // SAFETY: creates a shareable fence on our device into a local out-pointer.
            unsafe { device5.CreateFence(0, D3D11_FENCE_FLAG_SHARED, &mut fence) }?;
            fence.ok_or(Error::Incompatible("shared fence"))
        };
        let (produced, consumed) = (fence()?, fence()?);
        // SAFETY: unnamed NT handles for our own fences.
        let produced_handle = keep(&mut handles, unsafe { produced.CreateSharedHandle(None, GENERIC_ALL.0, PCWSTR::null()) }?)?;
        let consumed_handle = keep(&mut handles, unsafe { consumed.CreateSharedHandle(None, GENERIC_ALL.0, PCWSTR::null()) }?)?;
        header.publish(Shared { width, height, format: channel_format, adapter, textures: published, produced: produced_handle, consumed: consumed_handle })?;
        Ok(Self {
            header,
            device: device.clone(),
            textures,
            produced,
            consumed,
            _handles: handles,
            width,
            height,
            format: channel_format,
            sequence: 0,
            depth: None,
            with_depth: 0,
            mono,
        })
    }

    /// The channel's eye size.
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Whether images of `format` travel on this channel (its format family).
    pub fn carries(&self, format: DXGI_FORMAT) -> bool {
        fence_format_of(format.0 as u32) == Some(self.format)
    }

    /// The newest pair whose copy has finished on the GPU (what a reader can take).
    pub fn completed(&self) -> u64 {
        // SAFETY: reads a fence value.
        unsafe { self.produced.GetCompletedValue() }
    }

    /// The sequence the next successful publish will carry, so its pose record can go out first.
    pub fn next_sequence(&self) -> u64 {
        self.sequence + 1
    }

    /// Pairs published with depth.
    pub fn pairs_with_depth(&self) -> u64 {
        self.with_depth
    }

    /// Copies the two eyes, regions `regions` of `source` (each the channel's size), into a free
    /// slot and publishes them once the copies have run on the GPU. `Ok(false)`: skipped because no
    /// slot was free; it never waits. `context` is the device's immediate context.
    pub fn publish(&mut self, context: &ID3D11DeviceContext, source: &ID3D11Texture2D, regions: [D3D11_BOX; 2], tick: u64) -> Result<bool> {
        self.publish_with_depth(context, [(source, regions[0]), (source, regions[1])], None, tick)
    }

    /// [`FenceProducer11::publish`] with each eye from its own texture: `(texture, region)` for the
    /// left eye, then the right.
    pub fn publish_eyes(&mut self, context: &ID3D11DeviceContext, eyes: [(&ID3D11Texture2D, D3D11_BOX); 2], tick: u64) -> Result<bool> {
        self.publish_with_depth(context, eyes, None, tick)
    }

    /// [`FenceProducer11::publish`] with each eye a whole texture of the channel's size.
    pub fn publish_whole(&mut self, context: &ID3D11DeviceContext, eyes: [&ID3D11Texture2D; 2], tick: u64) -> Result<bool> {
        self.publish_whole_with_depth(context, eyes, None, tick)
    }

    /// One whole image of the channel's size as the pair, on a channel made by
    /// [`FenceProducer11::create_mono`] (one copy), or as both eyes on any other.
    pub fn publish_one(&mut self, context: &ID3D11DeviceContext, image: &ID3D11Texture2D, tick: u64) -> Result<bool> {
        self.publish_whole_with_depth(context, [image, image], None, tick)
    }

    /// [`FenceProducer11::publish_whole`] with each eye's depth beside its image.
    pub fn publish_whole_with_depth(&mut self, context: &ID3D11DeviceContext, eyes: [&ID3D11Texture2D; 2], depth: Option<Depth<'_>>, tick: u64) -> Result<bool> {
        let whole = D3D11_BOX { left: 0, top: 0, front: 0, right: self.width, bottom: self.height, back: 1 };
        self.publish_with_depth(context, [(eyes[0], whole), (eyes[1], whole)], depth, tick)
    }

    /// [`FenceProducer11::publish_eyes`] with each eye's depth (`None`: this pair has none). The first pair with depth makes
    /// the channel's depth textures at the depth's size; a later size or format makes them anew. A
    /// depth the channel cannot carry is refused with the pair, as a wrong image is.
    pub fn publish_with_depth(&mut self, context: &ID3D11DeviceContext, eyes: [(&ID3D11Texture2D, D3D11_BOX); 2], depth: Option<Depth<'_>>, tick: u64) -> Result<bool> {
        if self.mono && (eyes[0].0 != eyes[1].0 || !same_box(&eyes[0].1, &eyes[1].1)) {
            return Err(Error::Incompatible("two images on a channel for one"));
        }
        for (source, region) in eyes {
            let desc = d3d::texture_desc(source);
            let fits = region.right - region.left == self.width && region.bottom - region.top == self.height && region.right <= desc.Width && region.bottom <= desc.Height;
            if d3d::texture_device(source).ok().as_ref() != Some(&self.device) || !self.carries(desc.Format) || desc.SampleDesc.Count != 1 || !fits {
                return Err(Error::Incompatible("eye regions"));
            }
        }
        if !d3d::is_immediate_context_of(context, &self.device) {
            return Err(Error::Incompatible("context"));
        }
        let depth = depth.filter(|_| !self.header.depth_refused());
        let shape = depth.as_ref().map(|d| depth_shape(d, &self.device)).transpose()?;
        let context4: ID3D11DeviceContext4 = context.cast()?;
        if let Some((format, width, height, _)) = shape
            && self.depth.as_ref().is_none_or(|d| (d.format, d.width, d.height) != (format, width, height))
        {
            // Replaced textures are freed by D3D once the GPU is done with them.
            let mut handles = Vec::new();
            let (published, textures) = shared_slots(&self.device, width, height, DXGI_FORMAT(format as i32), D3D11_BIND_SHADER_RESOURCE.0 as u32, false, &mut handles)?;
            if !self.header.new_depth(format, width, height, published) {
                return Ok(false);
            }
            self.depth = Some(DepthSlots { textures, format, width, height, _handles: handles });
        }
        let sequence = self.sequence + 1;
        let held = match (&depth, shape) {
            (Some(depth), Some((.., rects))) => DepthSlot { sequence, params: depth.params, rects },
            _ => DepthSlot::default(),
        };
        // SAFETY: reads a fence value.
        let Some(marked) = self.header.mark(unsafe { self.consumed.GetCompletedValue() }, sequence, tick, held) else { return Ok(false) };
        let slot = marked.slot;
        // This sequence is spent from here on, signalled or not, so no later pair shares it.
        self.sequence = sequence;
        let depth_copies = depth.as_ref().zip(self.depth.as_ref()).map(|(depth, slots)| (depth.eyes, &slots.textures[slot]));
        // SAFETY: in-bounds regions of same-family textures on our device into our own slot, each
        // eye's depth whole into a same-family slot texture of its size, then the shared fence
        // signalled behind the copies.
        let signalled = unsafe {
            for (target, (source, region)) in self.textures[slot].iter().zip(&eyes).take(if self.mono { 1 } else { 2 }) {
                context.CopySubresourceRegion(target, 0, 0, 0, 0, *source, 0, Some(region));
            }
            if let Some((eyes, targets)) = depth_copies {
                for (target, eye) in targets.iter().zip(eyes) {
                    context.CopySubresourceRegion(target, 0, 0, 0, 0, eye.texture, 0, None);
                }
            }
            let signalled = context4.Signal(&self.produced, sequence);
            context.Flush();
            signalled
        };
        if let Err(e) = signalled {
            // As in the D3D12 producer: the mark names a pair that is never signalled.
            self.header.unmark(marked);
            return Err(e.into());
        }
        self.header.commit(marked, sequence, tick);
        self.with_depth += u64::from(depth_copies.is_some());
        Ok(true)
    }
}

/// A [`FenceProducer11`] made at the first frame's size and format. The viewer opens the channel
/// as made and keeps it, so frames of another size or format family cannot be published:
/// [`LazyProducer11::get`] refuses them (a producer that switches resolution does it before its
/// first frame).
pub struct LazyProducer11 {
    channel: ChannelName,
    /// Made for one image a pair ([`FenceProducer11::create_mono`]).
    mono: bool,
    producer: Option<FenceProducer11>,
    failed: bool,
    refused: Option<(u32, u32, i32)>,
}

/// What [`LazyProducer11::get`] gives: the producer when this frame can be published, and a line
/// worth logging (the channel was made, could not be, or a frame is refused), once each.
pub struct Lazy<'a> {
    pub producer: Option<&'a mut FenceProducer11>,
    pub note: Option<String>,
}

impl<'a> Lazy<'a> {
    /// The producer, after handing any note to `log`.
    pub fn ready(self, log: impl FnOnce(&str)) -> Option<&'a mut FenceProducer11> {
        if let Some(note) = &self.note {
            log(note);
        }
        self.producer
    }
}

impl LazyProducer11 {
    pub fn new(channel: ChannelName) -> Self {
        Self { channel, mono: false, producer: None, failed: false, refused: None }
    }

    /// [`LazyProducer11::new`] for a channel of one image a pair (a HUD panel).
    pub fn mono(channel: ChannelName) -> Self {
        Self { mono: true, ..Self::new(channel) }
    }

    pub fn channel(&self) -> &ChannelName {
        &self.channel
    }

    /// The channel's eye size, once made.
    pub fn size(&self) -> Option<(u32, u32)> {
        self.producer.as_ref().map(FenceProducer11::size)
    }

    /// Pairs published with depth.
    pub fn pairs_with_depth(&self) -> u64 {
        self.producer.as_ref().map_or(0, FenceProducer11::pairs_with_depth)
    }

    /// The producer, once [`LazyProducer11::get`] made it.
    pub fn made(&mut self) -> Option<&mut FenceProducer11> {
        self.producer.as_mut()
    }

    /// The producer for a `width` x `height` frame of `format` on `device`, making the channel at
    /// the first one.
    pub fn get(&mut self, device: &ID3D11Device, width: u32, height: u32, format: DXGI_FORMAT) -> Lazy<'_> {
        if self.failed {
            return Lazy { producer: None, note: None };
        }
        let mut note = None;
        if self.producer.is_none() {
            match FenceProducer11::create_with(&self.channel, device, width, height, format, self.mono) {
                Ok(producer) => {
                    note = Some(format!("GPU channel {} created at {width}x{height} format {}", self.channel.as_str(), format.0));
                    self.producer = Some(producer);
                }
                Err(e) => {
                    self.failed = true;
                    return Lazy { producer: None, note: Some(format!("GPU channel unavailable ({width}x{height} format {}): {e}", format.0)) };
                }
            }
        }
        let producer = self.producer.as_mut().expect("made above");
        if producer.size() != (width, height) || !producer.carries(format) {
            let first = self.refused.replace((width, height, format.0)) != Some((width, height, format.0));
            let (w, h) = producer.size();
            let note = first.then(|| format!("frame changed to {width}x{height} format {}; the viewer keeps the channel's {w}x{h}, so those frames are not published", format.0));
            return Lazy { producer: None, note };
        }
        Lazy { producer: Some(producer), note }
    }
}

#[cfg(test)]
mod tests;
