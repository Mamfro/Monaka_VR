//! Stereo pairs from a D3D12 producer ([`FenceProducer`]; a D3D11 one is [`crate::fence11`]) to a
//! D3D11 reader: [`FENCE_SLOTS`] slots of paired eye
//! textures and two shared fences. Handles travel by `DuplicateHandle` out of the producer process.
//! Neither side ever waits; the slot rules are in [`monaka_core::fence_rules`].
//!
//! A producer may send each eye's depth with a pair ([`FenceProducer::publish_with_depth`]): every
//! slot then has a depth texture per eye beside its images, made on the first pair with depth and
//! copied with the images on the same command list, so the `produced` fence covers both. The
//! depth textures and which pair each slot's depth belongs to are in the depth block beside the
//! header ([`DepthBlock`]): a reader that never asks for depth never opens it.

mod header;

use crate::d3d::{self, adapter_luid, luid};
use crate::d3d12::{self, Barriers, Recorder, subresource, transition};
use crate::shared::{NamedMutex, OwnedHandle, SharedBlock};
use crate::{ChannelName, Error, Result};
pub(crate) use header::{HeaderWriter, Shared};
use monaka_core::fence_rules::reader_take;
use monaka_contract::protocol::{Block, DepthBlock, DepthParams, DepthSlot, FENCE_SLOTS, FenceHeader, Luid, depth_slot_format, dimensions_ok, dxgi_format, fence_format_ok};
use windows::Win32::Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle, GENERIC_ALL, HANDLE};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_SHADER_RESOURCE, ID3D11Device, ID3D11Device1, ID3D11Device5, ID3D11DeviceContext, ID3D11DeviceContext4, ID3D11Fence, ID3D11Texture2D,
};
use windows::Win32::Graphics::Direct3D12::{
    D3D12_COMMAND_LIST_TYPE_DIRECT, D3D12_FENCE_FLAG_SHARED, D3D12_HEAP_FLAG_SHARED, D3D12_HEAP_PROPERTIES, D3D12_HEAP_TYPE_DEFAULT,
    D3D12_RESOURCE_DESC, D3D12_RESOURCE_DIMENSION_TEXTURE2D, D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET, D3D12_RESOURCE_FLAG_NONE,
    D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATES, ID3D12CommandQueue,
    D3D12_RESOURCE_FLAG_ALLOW_SIMULTANEOUS_ACCESS, D3D12_RESOURCE_FLAGS, ID3D12Device, ID3D12DeviceChild, ID3D12Fence, ID3D12Resource,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_SAMPLE_DESC};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcess, PROCESS_DUP_HANDLE};
use windows::core::Interface;

/// One received pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pair {
    pub sequence: u64,
    pub tick: u64,
}

/// One eye's depth for [`FenceProducer::publish_with_depth`].
#[derive(Clone, Copy)]
pub struct EyeDepth<'a> {
    /// The game's depth texture, or [`FenceProducer::held_depth`]: 32-bit float depth (with or
    /// without stencil) or 16-bit depth ([`depth_copyable`]), not multisampled. Its first
    /// subresource (the top mip's depth plane) is copied whole.
    pub texture: &'a ID3D12Resource,
    /// Its state when the copy is queued; it is left in it.
    pub state: D3D12_RESOURCE_STATES,
    /// The part of it that lines up with the eye image: left, top, width, height in texels.
    /// `None`: all of it.
    pub rect: Option<[u32; 4]>,
}

/// Both eyes' depth with one pair, and how it maps to distance.
#[derive(Clone, Copy)]
pub struct Depth<'a> {
    pub eyes: [EyeDepth<'a>; 2],
    /// Near and far in metres (far may be infinite), and whether the depth is reversed.
    pub params: DepthParams,
}

/// Whether `texture` can go to [`FenceProducer::publish_with_depth`] or [`FenceProducer::hold_depth`]:
/// a plain 2D texture of 32-bit float or 16-bit depth. 24-bit depth needs converting first.
pub fn depth_copyable(texture: &ID3D12Resource) -> bool {
    // SAFETY: reads a resource descriptor.
    let desc = unsafe { texture.GetDesc() };
    desc.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE2D
        && desc.SampleDesc.Count == 1
        && dimensions_ok(desc.Width as u32, desc.Height)
        && desc.Width <= u32::MAX as u64
        && depth_slot_format(desc.Format.0 as u32).is_some()
}

/// Whether `rect` (left, top, width, height) is a non-empty part of a `width` x `height` texture.
fn rect_inside(rect: [u32; 4], width: u32, height: u32) -> bool {
    let [left, top, w, h] = rect;
    w > 0 && h > 0 && left.checked_add(w).is_some_and(|right| right <= width) && top.checked_add(h).is_some_and(|bottom| bottom <= height)
}

/// A pair's depth as [`FenceReader::read_with_depth`] copied it into the reader's own textures
/// ([`FenceReader::depth_textures`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReadDepth {
    pub params: DepthParams,
    /// Per eye, the part of its depth texture that lines up with the eye image: left, top, width,
    /// height in texels.
    pub rects: [[u32; 4]; 2],
    /// The depth textures' typeless `DXGI_FORMAT` (R32 or R16) and size.
    pub format: u32,
    pub width: u32,
    pub height: u32,
}

/// Duplicates a handle value published by the producer into this process.
fn duplicate(producer: &OwnedHandle, value: u64) -> Result<OwnedHandle> {
    let mut local = HANDLE::default();
    // SAFETY: duplicates a handle value published by the producer into this process.
    unsafe { DuplicateHandle(producer.raw(), HANDLE(value as usize as *mut _), GetCurrentProcess(), &mut local, 0, false, DUPLICATE_SAME_ACCESS)? };
    OwnedHandle::new(local).ok_or(Error::Incompatible("published handle"))
}

/// Opens a texture the producer shared on our D3D11 device.
fn open_texture(producer: &OwnedHandle, device: &ID3D11Device1, value: u64) -> Result<ID3D11Texture2D> {
    let local = duplicate(producer, value)?;
    // SAFETY: opens a shared D3D12 texture on our D3D11 device; D3D keeps its own reference.
    Ok(unsafe { device.OpenSharedResource1(local.raw()) }?)
}

/// One set of the producer's depth textures, opened on the reader's device, and the reader's own
/// copies of the newest pair's depth.
struct ReaderDepth {
    slots: [[ID3D11Texture2D; 2]; FENCE_SLOTS],
    copies: [ID3D11Texture2D; 2],
    format: u32,
    width: u32,
    height: u32,
}

impl ReaderDepth {
    fn open(producer: &OwnedHandle, device: &ID3D11Device, depth: &DepthBlock) -> Result<Self> {
        let (format, width, height) = (depth.format, depth.width, depth.height);
        if !matches!(format, dxgi_format::R32_TYPELESS | dxgi_format::R16_TYPELESS) || !dimensions_ok(width, height) {
            return Err(Error::Incompatible("depth textures"));
        }
        let device1: ID3D11Device1 = device.cast()?;
        let mut slots = Vec::with_capacity(FENCE_SLOTS);
        for pair in depth.textures {
            let pair = [open_texture(producer, &device1, pair[0])?, open_texture(producer, &device1, pair[1])?];
            let shaped = |t: &ID3D11Texture2D| {
                let desc = d3d::texture_desc(t);
                desc.Width == width && desc.Height == height && desc.Format.0 as u32 == format
            };
            if !pair.iter().all(shaped) {
                return Err(Error::Incompatible("depth textures"));
            }
            slots.push(pair);
        }
        let slots = slots.try_into().map_err(|_| Error::Incompatible("slots"))?;
        let mut desc = d3d::simple_texture_desc(width, height, DXGI_FORMAT(format as i32));
        desc.BindFlags = D3D11_BIND_SHADER_RESOURCE.0 as u32;
        let copies = [d3d::create_texture(device, &desc)?, d3d::create_texture(device, &desc)?];
        Ok(Self { slots, copies, format, width, height })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Description {
    pub width: u32,
    pub height: u32,
    pub adapter: Luid,
    /// The producer's `DXGI_FORMAT` (the game's back buffer format).
    pub format: u32,
}

/// Reads the header without opening anything else.
pub fn describe(name: &ChannelName) -> Option<Description> {
    let mut block = SharedBlock::<FenceHeader>::open(&name.header(), &name.header_lock(), false).ok()?;
    let header = block.lock(0).tolerant()?;
    header
        .is_valid()
        .then(|| Description { width: header.width, height: header.height, adapter: header.adapter, format: header.format })
}

/// The reader's depth block: not opened yet, open, or refused for the run (another major's).
enum DepthSource {
    Untried,
    Open(SharedBlock<DepthBlock>),
    Off,
}

pub struct FenceReader {
    channel: ChannelName,
    header: SharedBlock<FenceHeader>,
    depth_block: DepthSource,
    _lease: NamedMutex,
    device: ID3D11Device,
    /// The producer process, for opening the depth textures it makes later.
    producer: OwnedHandle,
    textures: [[ID3D11Texture2D; 2]; FENCE_SLOTS],
    produced: ID3D11Fence,
    consumed: ID3D11Fence,
    description: Description,
    /// The producer's depth textures as last opened, and the set they are (0: none tried yet). A
    /// set that would not open stays `None` until the producer makes another.
    depth: Option<Box<ReaderDepth>>,
    depth_generation: u32,
    /// Reads that found nothing: the header locked by the producer, or no finished pair newer
    /// than the one shown (for diagnostics).
    pub busy: u64,
    pub idle: u64,
}

impl FenceReader {
    /// Opens the channel on `device`, which must be on the producer's adapter. Only one reader may
    /// hold a channel.
    pub fn open(name: &ChannelName, device: &ID3D11Device) -> Result<Self> {
        let description = describe(name).ok_or(Error::Incompatible("no valid fence header"))?;
        let adapter = adapter_luid(device)?;
        if adapter != description.adapter {
            return Err(Error::Incompatible("reader is on a different GPU than the producer"));
        }
        let device1: ID3D11Device1 = device.cast()?;
        let device5: ID3D11Device5 = device.cast()?;
        let (lease, taken) = NamedMutex::create(&name.reader_lease())?;
        if taken {
            return Err(Error::ReaderTaken);
        }
        let mut header = SharedBlock::<FenceHeader>::open(&name.header(), &name.header_lock(), true)?;
        let record = header.lock(0).tolerant().ok_or(Error::Busy)?;
        let same = record.is_valid()
            && record.width == description.width
            && record.height == description.height
            && record.format == description.format
            && record.adapter == adapter;
        if !same {
            return Err(Error::Incompatible("fence header changed while opening"));
        }
        // SAFETY: opening the producer process only to duplicate its handles.
        let producer = OwnedHandle::new(unsafe { OpenProcess(PROCESS_DUP_HANDLE, false, record.producer_pid) }?)
            .ok_or(Error::Incompatible("producer process"))?;
        let mut slots = Vec::with_capacity(FENCE_SLOTS);
        for slot in record.textures {
            let left = open_texture(&producer, &device1, slot[0])?;
            // A channel for one image publishes its one texture as both eyes'.
            let right = if slot[1] == slot[0] { left.clone() } else { open_texture(&producer, &device1, slot[1])? };
            slots.push([left, right]);
        }
        let textures: [[ID3D11Texture2D; 2]; FENCE_SLOTS] = slots.try_into().map_err(|_| Error::Incompatible("slots"))?;
        let open_fence = |value: u64| -> Result<ID3D11Fence> {
            let local = duplicate(&producer, value)?;
            let mut fence = None;
            // SAFETY: opens a shared fence on our device into a local out-pointer.
            unsafe { device5.OpenSharedFence(local.raw(), &mut fence) }?;
            fence.ok_or(Error::Incompatible("shared fence"))
        };
        let produced = open_fence(record.produced)?;
        let consumed = open_fence(record.consumed)?;
        drop(record);
        Ok(Self {
            channel: name.clone(),
            header,
            depth_block: DepthSource::Untried,
            _lease: lease,
            device: device.clone(),
            producer,
            textures,
            produced,
            consumed,
            description,
            depth: None,
            depth_generation: 0,
            busy: 0,
            idle: 0,
        })
    }

    pub fn description(&self) -> Description {
        self.description
    }

    /// Copies the newest pair into `eyes` once its GPU copy has finished and it is newer than
    /// `seen`, then signals `consumed` so the producer may reuse the slot. `Ok(None)`: nothing new.
    pub fn read(&mut self, context: &ID3D11DeviceContext, eyes: [&ID3D11Texture2D; 2], seen: u64) -> Result<Option<Pair>> {
        Ok(self.take(context, eyes, seen, false)?.map(|(pair, _)| pair))
    }

    /// [`FenceReader::read`] of the left eye alone into `image` (a channel for one image, a HUD
    /// panel).
    pub fn read_one(&mut self, context: &ID3D11DeviceContext, image: &ID3D11Texture2D, seen: u64) -> Result<Option<Pair>> {
        Ok(self.take(context, [image, image], seen, false)?.map(|(pair, _)| pair))
    }

    /// [`FenceReader::read`], also copying the pair's depth into [`FenceReader::depth_textures`]
    /// when the producer sent it with the pair (`None` beside a pair without depth).
    pub fn read_with_depth(&mut self, context: &ID3D11DeviceContext, eyes: [&ID3D11Texture2D; 2], seen: u64) -> Result<Option<(Pair, Option<ReadDepth>)>> {
        self.take(context, eyes, seen, true)
    }

    /// The reader's copies of the last depth [`FenceReader::read_with_depth`] returned, left eye
    /// first; replaced (and reshaped) when the producer makes new depth textures.
    pub fn depth_textures(&self) -> Option<[&ID3D11Texture2D; 2]> {
        self.depth.as_ref().map(|d| [&d.copies[0], &d.copies[1]])
    }

    fn take(&mut self, context: &ID3D11DeviceContext, eyes: [&ID3D11Texture2D; 2], seen: u64, with_depth: bool) -> Result<Option<(Pair, Option<ReadDepth>)>> {
        let shape_ok = |t: &ID3D11Texture2D| {
            let desc = d3d::texture_desc(t);
            d3d::texture_device(t).is_ok_and(|owner| owner == self.device)
                && desc.Width == self.description.width
                && desc.Height == self.description.height
                && desc.Format.0 as u32 == self.description.format
                && desc.ArraySize == 1
                && desc.MipLevels == 1
                && desc.SampleDesc.Count == 1
        };
        if !eyes.iter().all(|t| shape_ok(t)) {
            return Err(Error::Incompatible("eye textures"));
        }
        let context4: ID3D11DeviceContext4 = context.cast()?;
        let (slot, pair) = {
            let Some(mut record) = self.header.lock(0).tolerant() else {
                self.busy += 1;
                return Ok(None);
            };
            // SAFETY: reads a fence value.
            let produced = unsafe { self.produced.GetCompletedValue() };
            let Some((slot, sequence)) = reader_take(&record, seen, produced) else {
                self.idle += 1;
                return Ok(None);
            };
            record.reader_claim = sequence;
            record.reader_slot = slot;
            let slot = slot as usize;
            (slot, Pair { sequence, tick: record.slot_ticks[slot] })
        };
        // The claim keeps the producer off this slot, its depth included, until the next read.
        let depth = if with_depth { self.depth_for(slot, pair.sequence) } else { None };
        // SAFETY: copies a completed slot into our textures, then signals when the copy has run.
        unsafe {
            // The same target for both eyes ([`FenceReader::read_one`]): the left eye alone.
            let count = if eyes[0] == eyes[1] { 1 } else { 2 };
            for (target, source) in eyes.into_iter().zip(&self.textures[slot]).take(count) {
                context.CopyResource(target, source);
            }
            if let (Some(_), Some(opened)) = (depth, &self.depth) {
                for (target, source) in opened.copies.iter().zip(&opened.slots[slot]) {
                    context.CopyResource(target, source);
                }
            }
            context4.Signal(&self.consumed, pair.sequence)?;
            context.Flush();
        }
        Ok(Some((pair, depth)))
    }

    /// The depth of pair `sequence` in `slot`, when the producer sent it with the pair: the depth
    /// block opened on first asking, and the producer's newest depth textures opened on its
    /// device.
    fn depth_for(&mut self, slot: usize, sequence: u64) -> Option<ReadDepth> {
        if matches!(self.depth_block, DepthSource::Untried) {
            let (name, lock) = self.channel.block(Block::Depth);
            self.depth_block = match SharedBlock::<DepthBlock>::open_versioned(&name, &lock, 0) {
                Ok(block) => DepthSource::Open(block),
                // Busy: the producer is starting it; asked again with the next pair.
                Err(Error::Busy) => DepthSource::Untried,
                Err(_) => DepthSource::Off,
            };
        }
        let DepthSource::Open(block) = &mut self.depth_block else { return None };
        let record = block.lock(0).tolerant()?;
        // New depth textures are opened under the depth block's lock, while their handles are
        // surely open in the producer.
        if record.generation != self.depth_generation {
            self.depth_generation = record.generation;
            self.depth = None;
            if record.generation != 0 {
                self.depth = ReaderDepth::open(&self.producer, &self.device, &record).ok().map(Box::new);
            }
        }
        let held = record.slots[slot];
        (held.sequence == sequence && held.params.is_valid())
            .then_some(self.depth.as_ref())
            .flatten()
            .map(|d| ReadDepth { params: held.params, rects: held.rects, format: d.format, width: d.width, height: d.height })
    }

    /// Whether the depth block would not open, so no pair will come with depth this run.
    pub fn depth_refused(&self) -> bool {
        matches!(self.depth_block, DepthSource::Off)
    }
}

/// GPU objects of a producer; leaked on purpose if the GPU never finishes with them.
struct ProducerGpu {
    device: ID3D12Device,
    queue: ID3D12CommandQueue,
    textures: SlotTextures,
    consumed: ID3D12Fence,
    produced: ID3D12Fence,
    /// Completes on the shared `produced` fence, signalled with each pair's sequence.
    recorder: Recorder,
    _handles: Vec<OwnedHandle>,
    /// The depth textures, once a pair has come with depth.
    depth: Option<DepthGpu>,
    /// Depth textures replaced by new ones, kept until the GPU has finished the pair (by sequence)
    /// last copied into them.
    retired: Vec<(u64, DepthGpu)>,
}

/// The shared depth textures of every slot, one per eye.
struct DepthGpu {
    textures: SlotTextures,
    format: u32,
    width: u32,
    height: u32,
    _handles: Vec<OwnedHandle>,
}

/// [`FenceProducer::hold`]'s private eye copy, on its own command lists and fence.
struct HoldGpu {
    texture: ID3D12Resource,
    recorder: Recorder,
}

pub struct FenceProducer {
    header: HeaderWriter,
    gpu: Option<ProducerGpu>,
    hold: Option<HoldGpu>,
    hold_depth: Option<HoldGpu>,
    /// Depth hold copies replaced by a new size, kept until the GPU has finished the last pair (by
    /// sequence) that could have been copied from them, and their own copy.
    retired_holds: Vec<(u64, HoldGpu)>,
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
    sequence: u64,
    /// One image a pair ([`FenceProducer::create_mono`]).
    mono: bool,
}

const RECORDINGS: usize = 4;

/// The eye images' slot textures: render targets, which D3D11 opens in their formats.
const IMAGE_FLAGS: D3D12_RESOURCE_FLAGS = D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET;
/// The depth slot textures: D3D11 opens single-channel formats (R32, R16) only when shared for
/// simultaneous access.
const DEPTH_FLAGS: D3D12_RESOURCE_FLAGS = D3D12_RESOURCE_FLAGS(D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET.0 | D3D12_RESOURCE_FLAG_ALLOW_SIMULTANEOUS_ACCESS.0);

/// A texture per eye in every slot.
type SlotTextures = [[ID3D12Resource; 2]; FENCE_SLOTS];

/// [`FENCE_SLOTS`] pairs of shareable `width` x `height` textures in `format` on `device` (with
/// `flags`), in the common state, and their shared handle values (the handles kept in `handles`).
/// `mono`: one texture a slot, published as both eyes' ([`FenceProducer::create_mono`]).
fn shared_slots(
    device: &ID3D12Device,
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
    flags: D3D12_RESOURCE_FLAGS,
    mono: bool,
    handles: &mut Vec<OwnedHandle>,
) -> Result<([[u64; 2]; FENCE_SLOTS], SlotTextures)> {
    let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() };
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        Width: width as u64,
        Height: height,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Flags: flags,
        ..Default::default()
    };
    let mut published = [[0u64; 2]; FENCE_SLOTS];
    let mut slots = Vec::with_capacity(FENCE_SLOTS);
    for slot in published.iter_mut() {
        let mut pair: Vec<ID3D12Resource> = Vec::with_capacity(2);
        for eye in 0..2 {
            if mono && eye == 1 {
                slot[1] = slot[0];
                pair.push(pair[0].clone());
                continue;
            }
            let mut texture: Option<ID3D12Resource> = None;
            // SAFETY: creation with a valid descriptor into a local out-pointer.
            unsafe { device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_SHARED, &desc, D3D12_RESOURCE_STATE_COMMON, None, &mut texture)? };
            let texture = texture.ok_or(Error::Incompatible("slot texture"))?;
            slot[eye] = share(device, &texture.cast()?, handles)?;
            pair.push(texture);
        }
        slots.push([pair.remove(0), pair.remove(0)]);
    }
    Ok((published, slots.try_into().map_err(|_| Error::Incompatible("slots"))?))
}

/// An NT handle for `object` (kept in `handles`), as the value published in the header.
fn share(device: &ID3D12Device, object: &ID3D12DeviceChild, handles: &mut Vec<OwnedHandle>) -> Result<u64> {
    // SAFETY: creates a handle for a live object of `device`.
    let handle = unsafe { device.CreateSharedHandle(object, None, GENERIC_ALL.0, None) }?;
    let value = handle.0 as usize as u64;
    handles.push(OwnedHandle::new(handle).ok_or(Error::Incompatible("shared handle"))?);
    Ok(value)
}

/// The depth textures' format and size for `depth`, and each eye's rect: both eyes plain 2D
/// textures of one size and depth format family, the rects inside them, and the planes valid.
fn depth_shape(depth: &Depth<'_>) -> Result<(u32, u32, u32, [[u32; 4]; 2])> {
    if !depth.params.is_valid() {
        return Err(Error::Incompatible("depth near and far"));
    }
    let mut shape = None;
    let mut rects = [[0; 4]; 2];
    for (eye, rect) in depth.eyes.iter().zip(&mut rects) {
        if !depth_copyable(eye.texture) {
            return Err(Error::Incompatible("eye depth format"));
        }
        // SAFETY: reads a resource descriptor.
        let desc = unsafe { eye.texture.GetDesc() };
        let this = (depth_slot_format(desc.Format.0 as u32).expect("checked by depth_copyable"), desc.Width as u32, desc.Height);
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

impl FenceProducer {
    /// The channel at `buffer`'s size and format (a game's back buffer) on the device of `queue`,
    /// the game's direct queue; and that size. Streamline wraps the device, so the back buffer can
    /// report the wrapper while the queue reports the real device: what must match is the GPU.
    pub fn for_back_buffer(name: &ChannelName, buffer: &ID3D12Resource, queue: &ID3D12CommandQueue) -> std::result::Result<(Self, (u32, u32)), String> {
        let queue_device = d3d12::device_of(queue).map_err(|e| format!("game queue device: {e}"))?;
        let buffer_device = d3d12::device_of(buffer).map_err(|e| format!("back buffer device: {e}"))?;
        // SAFETY: reads a descriptor and adapter ids.
        let (desc, same_gpu) = unsafe { (buffer.GetDesc(), luid(buffer_device.GetAdapterLuid()) == luid(queue_device.GetAdapterLuid())) };
        if !same_gpu {
            return Err("back buffer and game queue are on different GPUs".into());
        }
        let producer = Self::create(name, &queue_device, queue, desc.Width as u32, desc.Height, desc.Format)
            .map_err(|e| format!("fence channel creation failed ({}x{} format {}): {e}", desc.Width, desc.Height, desc.Format.0))?;
        Ok((producer, (desc.Width as u32, desc.Height)))
    }

    /// Claims `name` and creates the shared slots on `queue`, a direct queue of `device`.
    pub fn create(name: &ChannelName, device: &ID3D12Device, queue: &ID3D12CommandQueue, width: u32, height: u32, format: DXGI_FORMAT) -> Result<Self> {
        Self::create_with(name, device, queue, width, height, format, false)
    }

    /// [`FenceProducer::create`] for one image a pair (a HUD panel): a texture a slot, published as
    /// both eyes', so a reader that copies both eyes still gets the image in each. Published with
    /// [`FenceProducer::publish_one`].
    pub fn create_mono(name: &ChannelName, device: &ID3D12Device, queue: &ID3D12CommandQueue, width: u32, height: u32, format: DXGI_FORMAT) -> Result<Self> {
        Self::create_with(name, device, queue, width, height, format, true)
    }

    fn create_with(name: &ChannelName, device: &ID3D12Device, queue: &ID3D12CommandQueue, width: u32, height: u32, format: DXGI_FORMAT, mono: bool) -> Result<Self> {
        if !dimensions_ok(width, height) || !fence_format_ok(format.0 as u32) {
            return Err(Error::Incompatible("eye size or format"));
        }
        // SAFETY: COM queries on live objects; creation calls with valid descriptors.
        unsafe {
            if queue.GetDesc().Type != D3D12_COMMAND_LIST_TYPE_DIRECT {
                return Err(Error::Incompatible("queue must be a direct queue"));
            }
            let mut header = HeaderWriter::claim(name)?;
            let mut handles = Vec::new();
            let (published, textures) = shared_slots(device, width, height, format, IMAGE_FLAGS, mono, &mut handles)?;
            let produced: ID3D12Fence = device.CreateFence(0, D3D12_FENCE_FLAG_SHARED)?;
            let consumed: ID3D12Fence = device.CreateFence(0, D3D12_FENCE_FLAG_SHARED)?;
            let produced_handle = share(device, &produced.cast()?, &mut handles)?;
            let consumed_handle = share(device, &consumed.cast()?, &mut handles)?;
            let recorder = Recorder::on(device, RECORDINGS, produced.clone())?;

            header.publish(Shared {
                width,
                height,
                format: format.0 as u32,
                adapter: luid(device.GetAdapterLuid()),
                textures: published,
                produced: produced_handle,
                consumed: consumed_handle,
            })?;
            let gpu = ProducerGpu {
                device: device.clone(),
                queue: queue.clone(),
                textures,
                consumed,
                produced,
                recorder,
                _handles: handles,
                depth: None,
                retired: Vec::new(),
            };
            Ok(Self { header, gpu: Some(gpu), hold: None, hold_depth: None, retired_holds: Vec::new(), width, height, format, sequence: 0, mono })
        }
    }

    /// The sequence the next successful publish will carry, so its pose record can go out first.
    pub fn next_sequence(&self) -> u64 {
        self.sequence + 1
    }

    /// Copies the two eye images (in the given states) into a free slot on the queue and publishes
    /// them. `Ok(false)`: skipped because no slot or recording was free; it never waits.
    pub fn publish(&mut self, left: (&ID3D12Resource, D3D12_RESOURCE_STATES), right: (&ID3D12Resource, D3D12_RESOURCE_STATES), tick: u64) -> Result<bool> {
        self.publish_with_depth(left, right, None, tick)
    }

    /// One image (in `state`) as the pair, on a channel made by [`FenceProducer::create_mono`]
    /// (one copy), or as both eyes on any other.
    pub fn publish_one(&mut self, image: (&ID3D12Resource, D3D12_RESOURCE_STATES), tick: u64) -> Result<bool> {
        self.publish_with_depth(image, image, None, tick)
    }

    /// [`FenceProducer::publish`] with each eye's depth beside its image (`None`: this pair has
    /// none). The depth is copied on the same command list as the images. The first pair with
    /// depth makes the channel's depth textures, at the depth's size; a later size or format makes
    /// them anew. A depth the channel cannot carry ([`depth_copyable`], a rect outside its texture,
    /// both eyes not alike, invalid planes) is refused with the pair, as a wrong image is.
    pub fn publish_with_depth(
        &mut self,
        left: (&ID3D12Resource, D3D12_RESOURCE_STATES),
        right: (&ID3D12Resource, D3D12_RESOURCE_STATES),
        depth: Option<Depth<'_>>,
        tick: u64,
    ) -> Result<bool> {
        let gpu = self.gpu.as_mut().ok_or(Error::Unhealthy)?;
        if self.mono && left.0 != right.0 {
            return Err(Error::Incompatible("two images on a channel for one"));
        }
        let mono = self.mono;
        let sources = [left, right];
        for (source, _) in sources {
            // SAFETY: reads a resource descriptor.
            let desc = unsafe { source.GetDesc() };
            let plain = desc.Width == self.width as u64 && desc.Height == self.height && desc.Format == self.format;
            if !plain || desc.SampleDesc.Count != 1 || desc.DepthOrArraySize != 1 || desc.MipLevels != 1 {
                return Err(Error::Incompatible("eye image shape"));
            }
        }
        let depth = depth.filter(|_| !self.header.depth_refused());
        let shape = depth.as_ref().map(depth_shape).transpose()?;
        // SAFETY: fence reads, command recording on our own lists, and queue submission.
        unsafe {
            if !gpu.recorder.ready()? {
                return Ok(false);
            }
            let done = gpu.produced.GetCompletedValue();
            gpu.retired.retain(|(last, _)| *last > done);
            self.retired_holds.retain(|(last, hold)| *last > done || !hold.recorder.idle(0));
            if let Some((format, width, height, _)) = shape
                && gpu.depth.as_ref().is_none_or(|d| (d.format, d.width, d.height) != (format, width, height))
            {
                let mut handles = Vec::new();
                let (published, textures) = shared_slots(&gpu.device, width, height, DXGI_FORMAT(format as i32), DEPTH_FLAGS, false, &mut handles)?;
                if !self.header.new_depth(format, width, height, published) {
                    return Ok(false);
                }
                if let Some(old) = gpu.depth.replace(DepthGpu { textures, format, width, height, _handles: handles }) {
                    gpu.retired.push((self.sequence, old));
                }
            }
            let sequence = self.sequence + 1;
            let held = match (&depth, shape) {
                (Some(depth), Some((.., rects))) => DepthSlot { sequence, params: depth.params, rects },
                _ => DepthSlot::default(),
            };
            let Some(marked) = self.header.mark(gpu.consumed.GetCompletedValue(), sequence, tick, held) else { return Ok(false) };
            let slot = marked.slot;
            let list = match gpu.recorder.begin() {
                Ok(Some(list)) => list,
                failed => {
                    // Not copied after all. A mark that stays spends this sequence, so no later
                    // pair shares it.
                    if !self.header.restore(marked) {
                        self.sequence = sequence;
                    }
                    return Ok(failed.map(|_| false)?);
                }
            };
            let list = &list;
            let same = left.0 == right.0;
            let targets = &gpu.textures[slot];
            // Two eyes' colour and depth, each a source and a target.
            let mut before = Barriers::<8>::new();
            let mut after = Barriers::<8>::new();
            for (eye, (source, state)) in sources.iter().enumerate() {
                if !(same && eye == 1) {
                    before.push(transition(source, *state, D3D12_RESOURCE_STATE_COPY_SOURCE));
                    after.push(transition(source, D3D12_RESOURCE_STATE_COPY_SOURCE, *state));
                }
                if !(mono && eye == 1) {
                    before.push(transition(&targets[eye], D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_COPY_DEST));
                    after.push(transition(&targets[eye], D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COMMON));
                }
            }
            let depth_copies = depth.as_ref().zip(gpu.depth.as_ref()).map(|(depth, slots)| (depth.eyes, &slots.textures[slot]));
            if let Some((eyes, targets)) = depth_copies {
                let same = eyes[0].texture == eyes[1].texture;
                for (eye, source) in eyes.iter().enumerate() {
                    if !(same && eye == 1) {
                        before.push(transition(source.texture, source.state, D3D12_RESOURCE_STATE_COPY_SOURCE));
                        after.push(transition(source.texture, D3D12_RESOURCE_STATE_COPY_SOURCE, source.state));
                    }
                    before.push(transition(&targets[eye], D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_COPY_DEST));
                    after.push(transition(&targets[eye], D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COMMON));
                }
            }
            list.ResourceBarrier(before.as_slice());
            for (eye, (source, _)) in sources.iter().enumerate().take(if mono { 1 } else { 2 }) {
                list.CopyResource(&targets[eye], *source);
            }
            if let Some((eyes, targets)) = depth_copies {
                for (target, source) in targets.iter().zip(eyes) {
                    // The top mip's depth plane, whole: a depth-stencil copy may not take a box.
                    list.CopyTextureRegion(&subresource(target, 0), 0, 0, 0, &subresource(source.texture, 0), None);
                }
            }
            list.ResourceBarrier(after.as_slice());
            // This sequence is spent from here on, submitted or not, so no later pair shares it.
            self.sequence = sequence;
            if let Err(e) = gpu.recorder.submit_as(&gpu.queue, sequence) {
                self.header.unmark(marked);
                return Err(e.into());
            }
            self.header.commit(marked, sequence, tick);
        }
        Ok(true)
    }
}

impl FenceProducer {
    /// Copies one eye (in `state`) into a private texture on the queue, so it survives the other eye
    /// being drawn over the same back buffer; publish it later with [`FenceProducer::held`].
    /// `Ok(false)`: skipped because no recording was free; it never waits.
    pub fn hold(&mut self, source: &ID3D12Resource, state: D3D12_RESOURCE_STATES) -> Result<bool> {
        let gpu = self.gpu.as_ref().ok_or(Error::Unhealthy)?;
        // SAFETY: reads a resource descriptor.
        let desc = unsafe { source.GetDesc() };
        let plain = desc.Width == self.width as u64 && desc.Height == self.height && desc.Format == self.format;
        if !plain || desc.SampleDesc.Count != 1 || desc.DepthOrArraySize != 1 || desc.MipLevels != 1 {
            return Err(Error::Incompatible("eye image shape"));
        }
        if self.hold.is_none() {
            let device = d3d12::device_of(&gpu.queue)?;
            let texture = d3d12::texture(&device, self.width, self.height, self.format, D3D12_RESOURCE_FLAG_NONE)?;
            self.hold = Some(HoldGpu { texture, recorder: Recorder::new(&device, RECORDINGS)? });
        }
        let hold = self.hold.as_mut().expect("created above");
        let Some(list) = hold.recorder.begin()? else { return Ok(false) };
        d3d12::copy_whole(&list, &hold.texture, source, state);
        hold.recorder.submit(&gpu.queue)?;
        Ok(true)
    }

    /// The eye kept by the last [`FenceProducer::hold`], in the common state; publish it from the
    /// same queue.
    pub fn held(&self) -> Option<ID3D12Resource> {
        self.hold.as_ref().map(|hold| hold.texture.clone())
    }

    /// [`FenceProducer::hold`] for an eye's depth: copies the depth plane of `source` (in `state`;
    /// [`depth_copyable`]) into a private texture, so it survives the other eye being drawn; publish
    /// it later as [`FenceProducer::held_depth`]. `Ok(false)`: skipped because no recording was
    /// free; it never waits.
    pub fn hold_depth(&mut self, source: &ID3D12Resource, state: D3D12_RESOURCE_STATES) -> Result<bool> {
        let gpu = self.gpu.as_ref().ok_or(Error::Unhealthy)?;
        if !depth_copyable(source) {
            return Err(Error::Incompatible("eye depth format"));
        }
        // SAFETY: reads resource descriptors.
        let desc = unsafe { source.GetDesc() };
        let format = DXGI_FORMAT(depth_slot_format(desc.Format.0 as u32).expect("checked by depth_copyable") as i32);
        // SAFETY: as above.
        let fits = self.hold_depth.as_ref().is_some_and(|h| unsafe {
            let have = h.texture.GetDesc();
            have.Width == desc.Width && have.Height == desc.Height && have.Format == format
        });
        if !fits {
            // A new size replaces the copy. The old one may still be read by a submitted publish
            // (as a pair's depth) or written by its own copy: it is kept until both are done.
            let texture = d3d12::texture(&gpu.device, desc.Width as u32, desc.Height, format, D3D12_RESOURCE_FLAG_NONE)?;
            let fresh = HoldGpu { texture, recorder: Recorder::new(&gpu.device, RECORDINGS)? };
            if let Some(old) = self.hold_depth.replace(fresh) {
                self.retired_holds.push((self.sequence, old));
            }
        }
        let hold = self.hold_depth.as_mut().expect("created above");
        let Some(list) = hold.recorder.begin()? else { return Ok(false) };
        d3d12::copy_plane0(&list, &hold.texture, source, state);
        hold.recorder.submit(&gpu.queue)?;
        Ok(true)
    }

    /// The depth kept by the last [`FenceProducer::hold_depth`] (typeless R32 or R16), in the
    /// common state; publish it from the same queue.
    pub fn held_depth(&self) -> Option<ID3D12Resource> {
        self.hold_depth.as_ref().map(|hold| hold.texture.clone())
    }
}

impl Drop for FenceProducer {
    /// Frees the GPU objects once the GPU is done with them. If it is not done within five seconds
    /// they are leaked on purpose rather than freed under a running copy. The publishes are waited
    /// for first: they read the hold copies, so those are only freed after them.
    fn drop(&mut self) {
        let holds: Vec<HoldGpu> = [self.hold.take(), self.hold_depth.take()].into_iter().flatten().chain(self.retired_holds.drain(..).map(|(_, hold)| hold)).collect();
        if let Some(gpu) = self.gpu.take()
            && !gpu.recorder.idle(5000)
        {
            std::mem::forget(gpu);
            std::mem::forget(holds);
            return;
        }
        for hold in holds {
            if !hold.recorder.idle(5000) {
                std::mem::forget(hold);
            }
        }
    }
}

#[cfg(test)]
mod tests;
