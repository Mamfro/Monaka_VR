//! Stereo pairs from a D3D12 producer to a D3D11 reader: [`FENCE_SLOTS`] slots of paired eye
//! textures and two shared fences. Handles travel by `DuplicateHandle` out of the producer process.
//! Neither side ever waits; the slot rules are in [`monaka_core::fence_rules`].
//!
//! A producer may send each eye's depth with a pair ([`FenceProducer::publish_with_depth`]): every
//! slot then has a depth texture per eye beside its images, made on the first pair with depth and
//! copied with the images on the same command list, so the `produced` fence covers both.

use crate::d3d::{self, adapter_luid, luid};
use crate::d3d12::{self, Recorder, subresource, transition};
use crate::shared::{Create, NamedMutex, OwnedHandle, SharedBlock};
use crate::{ChannelName, Error, Result};
use monaka_core::fence_rules::{choose_slot, reader_take};
use monaka_core::protocol::{
    DepthParams, DepthSlot, FENCE_MAGIC, FENCE_SLOTS, FENCE_VERSION, FenceDepth, FenceHeader, Luid, NO_SLOT, depth_slot_format, dimensions_ok,
    dxgi_format, fence_format_ok,
};
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
use windows::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId, OpenProcess, PROCESS_DUP_HANDLE};
use windows::core::Interface;

pub use crate::keyed::Pair;

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
    fn open(producer: &OwnedHandle, device: &ID3D11Device, depth: &FenceDepth) -> Result<Self> {
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

pub struct FenceReader {
    header: SharedBlock<FenceHeader>,
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
            slots.push([open_texture(&producer, &device1, slot[0])?, open_texture(&producer, &device1, slot[1])?]);
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
            header,
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
        let (slot, pair, depth) = {
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
            // New depth textures are opened under the lock, while their handles are surely open in
            // the producer.
            if with_depth && record.depth.generation != self.depth_generation {
                self.depth_generation = record.depth.generation;
                self.depth = None;
                if record.depth.generation != 0 {
                    self.depth = ReaderDepth::open(&self.producer, &self.device, &record.depth).ok().map(Box::new);
                }
            }
            let held = record.depth.slots[slot];
            let depth = (with_depth && held.sequence == sequence && held.params.is_valid())
                .then_some(self.depth.as_ref())
                .flatten()
                .map(|d| ReadDepth { params: held.params, rects: held.rects, format: d.format, width: d.width, height: d.height });
            (slot, Pair { sequence, tick: record.slot_ticks[slot] }, depth)
        };
        // SAFETY: copies a completed slot into our textures, then signals when the copy has run.
        unsafe {
            for (target, source) in eyes.into_iter().zip(&self.textures[slot]) {
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
    header: SharedBlock<FenceHeader>,
    gpu: Option<ProducerGpu>,
    hold: Option<HoldGpu>,
    hold_depth: Option<HoldGpu>,
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
    sequence: u64,
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
fn shared_slots(
    device: &ID3D12Device,
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
    flags: D3D12_RESOURCE_FLAGS,
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
        let mut pair = Vec::with_capacity(2);
        for value in slot.iter_mut() {
            let mut texture: Option<ID3D12Resource> = None;
            // SAFETY: creation with a valid descriptor into a local out-pointer.
            unsafe { device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_SHARED, &desc, D3D12_RESOURCE_STATE_COMMON, None, &mut texture)? };
            let texture = texture.ok_or(Error::Incompatible("slot texture"))?;
            *value = share(device, &texture.cast()?, handles)?;
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
        if !dimensions_ok(width, height) || !fence_format_ok(format.0 as u32) {
            return Err(Error::Incompatible("eye size or format"));
        }
        // SAFETY: COM queries on live objects; creation calls with valid descriptors.
        unsafe {
            if queue.GetDesc().Type != D3D12_COMMAND_LIST_TYPE_DIRECT {
                return Err(Error::Incompatible("queue must be a direct queue"));
            }
            let mut header = SharedBlock::<FenceHeader>::create(&name.header(), &name.header_lock(), Create::Exclusive)?;
            let mut record = header.lock(0).tolerant().ok_or(Error::Busy)?;

            let mut handles = Vec::new();
            let (published, textures) = shared_slots(device, width, height, format, IMAGE_FLAGS, &mut handles)?;
            let produced: ID3D12Fence = device.CreateFence(0, D3D12_FENCE_FLAG_SHARED)?;
            let consumed: ID3D12Fence = device.CreateFence(0, D3D12_FENCE_FLAG_SHARED)?;
            let produced_handle = share(device, &produced.cast()?, &mut handles)?;
            let consumed_handle = share(device, &consumed.cast()?, &mut handles)?;
            let recorder = Recorder::on(device, RECORDINGS, produced.clone())?;

            *record = FenceHeader {
                magic: FENCE_MAGIC,
                version: FENCE_VERSION,
                width,
                height,
                format: format.0 as u32,
                producer_pid: GetCurrentProcessId(),
                adapter: luid(device.GetAdapterLuid()),
                textures: published,
                produced: produced_handle,
                consumed: consumed_handle,
                latest_slot: NO_SLOT,
                reader_slot: NO_SLOT,
                ..FenceHeader::default()
            };
            drop(record);
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
            Ok(Self { header, gpu: Some(gpu), hold: None, hold_depth: None, width, height, format, sequence: 0 })
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
        let sources = [left, right];
        for (source, _) in sources {
            // SAFETY: reads a resource descriptor.
            let desc = unsafe { source.GetDesc() };
            let plain = desc.Width == self.width as u64 && desc.Height == self.height && desc.Format == self.format;
            if !plain || desc.SampleDesc.Count != 1 || desc.DepthOrArraySize != 1 || desc.MipLevels != 1 {
                return Err(Error::Incompatible("eye image shape"));
            }
        }
        let shape = depth.as_ref().map(depth_shape).transpose()?;
        // SAFETY: fence reads, command recording on our own lists, and queue submission.
        unsafe {
            if !gpu.recorder.ready()? {
                return Ok(false);
            }
            let done = gpu.produced.GetCompletedValue();
            gpu.retired.retain(|(last, _)| *last > done);
            if let Some((format, width, height, _)) = shape
                && gpu.depth.as_ref().is_none_or(|d| (d.format, d.width, d.height) != (format, width, height))
            {
                let mut handles = Vec::new();
                let (published, textures) = shared_slots(&gpu.device, width, height, DXGI_FORMAT(format as i32), DEPTH_FLAGS, &mut handles)?;
                let Some(mut record) = self.header.lock(0).tolerant() else { return Ok(false) };
                let generation = record.depth.generation.wrapping_add(1).max(1);
                record.depth = FenceDepth { generation, format, width, height, textures: published, ..FenceDepth::default() };
                drop(record);
                if let Some(old) = gpu.depth.replace(DepthGpu { textures, format, width, height, _handles: handles }) {
                    gpu.retired.push((self.sequence, old));
                }
            }
            let sequence = self.sequence + 1;
            let held = match (&depth, shape) {
                (Some(depth), Some((.., rects))) => DepthSlot { sequence, params: depth.params, rects },
                _ => DepthSlot::default(),
            };
            // The slot is marked with this pair before its copy is submitted, in the same lock as it
            // is chosen: the reader never takes it while it is being rewritten.
            let (slot, before) = {
                let Some(mut record) = self.header.lock(0).tolerant() else { return Ok(false) };
                let Some(slot) = choose_slot(&record, gpu.consumed.GetCompletedValue()) else { return Ok(false) };
                let slot = slot as usize;
                let before = (record.slot_sequences[slot], record.slot_ticks[slot], record.depth.slots[slot]);
                (record.slot_sequences[slot], record.slot_ticks[slot], record.depth.slots[slot]) = (sequence, tick, held);
                (slot, before)
            };
            let list = match gpu.recorder.begin() {
                Ok(Some(list)) => list,
                failed => {
                    // Not copied after all: the slot holds what it held.
                    if let Some(mut record) = self.header.lock(0).tolerant() {
                        (record.slot_sequences[slot], record.slot_ticks[slot], record.depth.slots[slot]) = before;
                    }
                    return Ok(failed.map(|_| false)?);
                }
            };
            let list = &list;
            let same = left.0 == right.0;
            let targets = &gpu.textures[slot];
            let mut before = Vec::with_capacity(8);
            let mut after = Vec::with_capacity(8);
            for (eye, (source, state)) in sources.iter().enumerate() {
                if !(same && eye == 1) {
                    before.push(transition(source, *state, D3D12_RESOURCE_STATE_COPY_SOURCE));
                    after.push(transition(source, D3D12_RESOURCE_STATE_COPY_SOURCE, *state));
                }
                before.push(transition(&targets[eye], D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_COPY_DEST));
                after.push(transition(&targets[eye], D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COMMON));
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
            list.ResourceBarrier(&before);
            for (eye, (source, _)) in sources.iter().enumerate() {
                list.CopyResource(&targets[eye], *source);
            }
            if let Some((eyes, targets)) = depth_copies {
                for (target, source) in targets.iter().zip(eyes) {
                    // The top mip's depth plane, whole: a depth-stencil copy may not take a box.
                    list.CopyTextureRegion(&subresource(target, 0), 0, 0, 0, &subresource(source.texture, 0), None);
                }
            }
            list.ResourceBarrier(&after);
            gpu.recorder.submit_as(&gpu.queue, sequence)?;
            // Only the producer writes `sequence`; the lock orders it against a reader's claim.
            let mut record = loop {
                if let Some(record) = self.header.lock(0).tolerant() {
                    break record;
                }
                std::hint::spin_loop();
            };
            record.latest_slot = slot as u32;
            record.tick = tick;
            record.sequence = sequence;
            self.sequence = sequence;
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
            // A new size replaces the copy once the GPU is done with it.
            if self.hold_depth.as_ref().is_some_and(|h| !h.recorder.idle(0)) {
                return Ok(false);
            }
            let texture = d3d12::texture(&gpu.device, desc.Width as u32, desc.Height, format, D3D12_RESOURCE_FLAG_NONE)?;
            self.hold_depth = Some(HoldGpu { texture, recorder: Recorder::new(&gpu.device, RECORDINGS)? });
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
    /// they are leaked on purpose rather than freed under a running copy.
    fn drop(&mut self) {
        for hold in [self.hold.take(), self.hold_depth.take()].into_iter().flatten() {
            if !hold.recorder.idle(5000) {
                std::mem::forget(hold);
            }
        }
        if let Some(gpu) = self.gpu.take()
            && !gpu.recorder.idle(5000)
        {
            std::mem::forget(gpu);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::d3d::{create_device, create_texture, find_adapter, read_pixel, simple_texture_desc};
    use crate::d3d12::wait_fence;
    use std::time::{Duration, Instant};
    use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
    use windows::Win32::Graphics::Direct3D12::{
        D3D12_CLEAR_FLAG_DEPTH, D3D12_COMMAND_QUEUE_DESC, D3D12_DESCRIPTOR_HEAP_DESC, D3D12_DESCRIPTOR_HEAP_TYPE_DSV, D3D12_DESCRIPTOR_HEAP_TYPE_RTV,
        D3D12_HEAP_FLAG_NONE, D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL, D3D12_RESOURCE_STATE_DEPTH_WRITE, D3D12_RESOURCE_STATE_RENDER_TARGET,
        D3D12CreateDevice, ID3D12CommandAllocator, ID3D12CommandList, ID3D12DescriptorHeap, ID3D12GraphicsCommandList, ID3D12PipelineState,
    };
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_D32_FLOAT, DXGI_FORMAT_D32_FLOAT_S8X24_UINT, DXGI_FORMAT_R8G8B8A8_UNORM};

    const RT: D3D12_RESOURCE_STATES = D3D12_RESOURCE_STATE_RENDER_TARGET;

    fn gpu12() -> (ID3D12Device, ID3D12CommandQueue) {
        unsafe {
            let mut device: Option<ID3D12Device> = None;
            D3D12CreateDevice(None::<&windows::core::IUnknown>, D3D_FEATURE_LEVEL_11_0, &mut device).unwrap();
            let device = device.unwrap();
            let queue = device.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC { Type: D3D12_COMMAND_LIST_TYPE_DIRECT, ..Default::default() }).unwrap();
            (device, queue)
        }
    }

    /// A 64x32 render target cleared to `rgba`, left in the render-target state, GPU work finished.
    fn cleared(device: &ID3D12Device, queue: &ID3D12CommandQueue, rgba: [f32; 4]) -> ID3D12Resource {
        unsafe {
            let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() };
            let desc = D3D12_RESOURCE_DESC {
                Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
                Width: 64,
                Height: 32,
                DepthOrArraySize: 1,
                MipLevels: 1,
                Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Flags: D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
                ..Default::default()
            };
            let mut texture: Option<ID3D12Resource> = None;
            device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, RT, None, &mut texture).unwrap();
            let texture = texture.unwrap();
            let views: ID3D12DescriptorHeap = device
                .CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC { Type: D3D12_DESCRIPTOR_HEAP_TYPE_RTV, NumDescriptors: 1, ..Default::default() })
                .unwrap();
            let view = views.GetCPUDescriptorHandleForHeapStart();
            device.CreateRenderTargetView(&texture, None, view);
            let allocator: ID3D12CommandAllocator = device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT).unwrap();
            let list: ID3D12GraphicsCommandList = device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None::<&ID3D12PipelineState>).unwrap();
            list.ClearRenderTargetView(view, &rgba, None);
            list.Close().unwrap();
            queue.ExecuteCommandLists(&[Some(list.cast::<ID3D12CommandList>().unwrap())]);
            let fence: ID3D12Fence = device.CreateFence(0, windows::Win32::Graphics::Direct3D12::D3D12_FENCE_FLAG_NONE).unwrap();
            queue.Signal(&fence, 1).unwrap();
            assert!(wait_fence(&fence, 1, 5000));
            texture
        }
    }

    fn until<T>(mut f: impl FnMut() -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(value) = f() {
                return value;
            }
            assert!(Instant::now() < deadline, "timed out");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn pairs_cross_from_d3d12_to_d3d11() {
        let name = ChannelName::new(format!(r"Local\monaka-test-fence-{}", std::process::id())).unwrap();
        let (device12, queue) = gpu12();
        let mut producer = FenceProducer::create(&name, &device12, &queue, 64, 32, DXGI_FORMAT_R8G8B8A8_UNORM).unwrap();
        assert!(FenceProducer::create(&name, &device12, &queue, 64, 32, DXGI_FORMAT_R8G8B8A8_UNORM).is_err(), "a second producer must be refused");

        let described = describe(&name).unwrap();
        assert_eq!((described.width, described.height, described.format), (64, 32, DXGI_FORMAT_R8G8B8A8_UNORM.0 as u32));
        let (device, context, _) = create_device(Some(&find_adapter(described.adapter).unwrap())).unwrap();
        let mut reader = FenceReader::open(&name, &device).unwrap();
        assert!(matches!(FenceReader::open(&name, &device), Err(Error::ReaderTaken)));

        let targets: [ID3D11Texture2D; 2] =
            std::array::from_fn(|_| create_texture(&device, &simple_texture_desc(64, 32, DXGI_FORMAT_R8G8B8A8_UNORM)).unwrap());
        let targets = [&targets[0], &targets[1]];
        assert_eq!(reader.read(&context, targets, 0).unwrap(), None);

        let left = cleared(&device12, &queue, [1.0, 0.0, 0.0, 1.0]);
        let right = cleared(&device12, &queue, [0.0, 1.0, 0.0, 1.0]);
        assert_eq!(producer.next_sequence(), 1);
        assert!(producer.publish((&left, RT), (&right, RT), 7).unwrap());
        let pair = until(|| reader.read(&context, targets, 0).unwrap());
        assert_eq!(pair, Pair { sequence: 1, tick: 7 });
        assert_eq!(read_pixel(&device, &context, targets[0], 3, 3).unwrap(), [255, 0, 0, 255]);
        assert_eq!(read_pixel(&device, &context, targets[1], 3, 3).unwrap(), [0, 255, 0, 255]);

        // The producer keeps finding free slots while the reader keeps up.
        let mut seen = pair.sequence;
        for _ in 0..10 {
            until(|| producer.publish((&right, RT), (&left, RT), 8).unwrap().then_some(()));
            seen = until(|| reader.read(&context, targets, seen).unwrap()).sequence;
        }
        assert_eq!(seen, 11);
        assert_eq!(read_pixel(&device, &context, targets[0], 3, 3).unwrap(), [0, 255, 0, 255]);
    }

    /// A 64x32 depth buffer of `format` cleared to `depth`, left in the depth-write state, GPU work
    /// finished.
    fn cleared_depth(device: &ID3D12Device, queue: &ID3D12CommandQueue, format: DXGI_FORMAT, depth: f32) -> ID3D12Resource {
        unsafe {
            let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() };
            let desc = D3D12_RESOURCE_DESC {
                Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
                Width: 64,
                Height: 32,
                DepthOrArraySize: 1,
                MipLevels: 1,
                Format: format,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Flags: D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL,
                ..Default::default()
            };
            let mut texture: Option<ID3D12Resource> = None;
            device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, DEPTH, None, &mut texture).unwrap();
            let texture = texture.unwrap();
            let views: ID3D12DescriptorHeap = device
                .CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC { Type: D3D12_DESCRIPTOR_HEAP_TYPE_DSV, NumDescriptors: 1, ..Default::default() })
                .unwrap();
            let view = views.GetCPUDescriptorHandleForHeapStart();
            device.CreateDepthStencilView(&texture, None, view);
            let allocator: ID3D12CommandAllocator = device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT).unwrap();
            let list: ID3D12GraphicsCommandList = device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None::<&ID3D12PipelineState>).unwrap();
            list.ClearDepthStencilView(view, D3D12_CLEAR_FLAG_DEPTH, depth, 0, None);
            list.Close().unwrap();
            queue.ExecuteCommandLists(&[Some(list.cast::<ID3D12CommandList>().unwrap())]);
            let fence: ID3D12Fence = device.CreateFence(0, windows::Win32::Graphics::Direct3D12::D3D12_FENCE_FLAG_NONE).unwrap();
            queue.Signal(&fence, 1).unwrap();
            assert!(wait_fence(&fence, 1, 5000));
            texture
        }
    }

    const DEPTH: D3D12_RESOURCE_STATES = D3D12_RESOURCE_STATE_DEPTH_WRITE;

    /// The first texel of a 4-byte float texture on the reader's device.
    fn depth_at_origin(device: &ID3D11Device, context: &ID3D11DeviceContext, texture: &ID3D11Texture2D) -> f32 {
        let (_, _, texel, texels) = crate::d3d::read_texels(device, context, texture).unwrap();
        assert_eq!(texel, 4);
        f32::from_le_bytes(texels[..4].try_into().unwrap())
    }

    #[test]
    fn depth_crosses_beside_the_eyes() {
        let name = ChannelName::new(format!(r"Local\monaka-test-depth-{}", std::process::id())).unwrap();
        let (device12, queue) = gpu12();
        let mut producer = FenceProducer::create(&name, &device12, &queue, 64, 32, DXGI_FORMAT_R8G8B8A8_UNORM).unwrap();
        let described = describe(&name).unwrap();
        let (device, context, _) = create_device(Some(&find_adapter(described.adapter).unwrap())).unwrap();
        let mut reader = FenceReader::open(&name, &device).unwrap();
        let targets: [ID3D11Texture2D; 2] =
            std::array::from_fn(|_| create_texture(&device, &simple_texture_desc(64, 32, DXGI_FORMAT_R8G8B8A8_UNORM)).unwrap());
        let targets = [&targets[0], &targets[1]];
        let image = cleared(&device12, &queue, [1.0, 0.0, 0.0, 1.0]);

        // 32-bit float depth, one eye with stencil: both travel as R32.
        let left = cleared_depth(&device12, &queue, DXGI_FORMAT_D32_FLOAT, 0.25);
        let right = cleared_depth(&device12, &queue, DXGI_FORMAT_D32_FLOAT_S8X24_UINT, 0.75);
        assert!(depth_copyable(&left) && depth_copyable(&right) && !depth_copyable(&image));
        let params = DepthParams::new(0.05, f32::INFINITY, true);
        let depth = Depth {
            eyes: [EyeDepth { texture: &left, state: DEPTH, rect: None }, EyeDepth { texture: &right, state: DEPTH, rect: Some([8, 0, 48, 32]) }],
            params,
        };
        assert!(producer.publish_with_depth((&image, RT), (&image, RT), Some(depth), 3).unwrap());
        let (pair, read) = until(|| reader.read_with_depth(&context, targets, 0).unwrap());
        assert_eq!(pair.sequence, 1);
        let read = read.expect("the pair came with depth");
        assert_eq!(read, ReadDepth { params, rects: [[0, 0, 64, 32], [8, 0, 48, 32]], format: dxgi_format::R32_TYPELESS, width: 64, height: 32 });
        let copies = reader.depth_textures().unwrap();
        assert_eq!(depth_at_origin(&device, &context, copies[0]), 0.25);
        assert_eq!(depth_at_origin(&device, &context, copies[1]), 0.75);

        // The viewer copies it on into a depth swapchain image of the runtime's (D32 float).
        let mut desc = simple_texture_desc(64, 32, windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_D32_FLOAT);
        desc.BindFlags = windows::Win32::Graphics::Direct3D11::D3D11_BIND_DEPTH_STENCIL.0 as u32;
        let swapchain_image = create_texture(&device, &desc).unwrap();
        crate::d3d::copy_texture(&context, &swapchain_image, copies[1]);
        assert_eq!(depth_at_origin(&device, &context, &swapchain_image), 0.75);

        // A pair without depth reads as such; the next with depth has it again.
        until(|| producer.publish((&image, RT), (&image, RT), 4).unwrap().then_some(()));
        let (pair, read) = until(|| reader.read_with_depth(&context, targets, 1).unwrap());
        assert_eq!((pair.sequence, read), (2, None));
        let left = cleared_depth(&device12, &queue, DXGI_FORMAT_D32_FLOAT, 0.5);
        let depth = Depth { eyes: [EyeDepth { texture: &left, state: DEPTH, rect: None }; 2], params };
        until(|| producer.publish_with_depth((&image, RT), (&image, RT), Some(depth), 5).unwrap().then_some(()));
        let (_, read) = until(|| reader.read_with_depth(&context, targets, 2).unwrap());
        assert!(read.is_some());
        assert_eq!(depth_at_origin(&device, &context, reader.depth_textures().unwrap()[1]), 0.5);

        // Depth the channel cannot carry is refused with the pair.
        let bad = Depth { params: DepthParams::new(1.0, 0.5, false), ..depth };
        assert!(producer.publish_with_depth((&image, RT), (&image, RT), Some(bad), 6).is_err());
        let outside = Depth { eyes: [EyeDepth { rect: Some([60, 0, 8, 8]), ..depth.eyes[0] }; 2], ..depth };
        assert!(producer.publish_with_depth((&image, RT), (&image, RT), Some(outside), 6).is_err());
    }

    #[test]
    fn a_held_depth_survives_its_source_being_redrawn() {
        let name = ChannelName::new(format!(r"Local\monaka-test-hold-depth-{}", std::process::id())).unwrap();
        let (device12, queue) = gpu12();
        let mut producer = FenceProducer::create(&name, &device12, &queue, 64, 32, DXGI_FORMAT_R8G8B8A8_UNORM).unwrap();
        let described = describe(&name).unwrap();
        let (device, context, _) = create_device(Some(&find_adapter(described.adapter).unwrap())).unwrap();
        let mut reader = FenceReader::open(&name, &device).unwrap();
        let targets: [ID3D11Texture2D; 2] =
            std::array::from_fn(|_| create_texture(&device, &simple_texture_desc(64, 32, DXGI_FORMAT_R8G8B8A8_UNORM)).unwrap());
        let image = cleared(&device12, &queue, [0.0, 0.0, 1.0, 1.0]);

        assert!(producer.held_depth().is_none());
        let first = cleared_depth(&device12, &queue, DXGI_FORMAT_D32_FLOAT, 0.125);
        assert!(producer.hold_depth(&first, DEPTH).unwrap());
        let second = cleared_depth(&device12, &queue, DXGI_FORMAT_D32_FLOAT, 0.875);
        let held = producer.held_depth().unwrap();
        let depth = Depth {
            eyes: [EyeDepth { texture: &held, state: D3D12_RESOURCE_STATE_COMMON, rect: None }, EyeDepth { texture: &second, state: DEPTH, rect: None }],
            params: DepthParams::new(0.1, 500.0, false),
        };
        assert!(producer.publish_with_depth((&image, RT), (&image, RT), Some(depth), 1).unwrap());
        let (_, read) = until(|| reader.read_with_depth(&context, [&targets[0], &targets[1]], 0).unwrap());
        assert!(read.is_some());
        let copies = reader.depth_textures().unwrap();
        assert_eq!(depth_at_origin(&device, &context, copies[0]), 0.125);
        assert_eq!(depth_at_origin(&device, &context, copies[1]), 0.875);
    }

    #[test]
    fn depth_rects_stay_inside() {
        assert!(rect_inside([0, 0, 64, 32], 64, 32));
        assert!(rect_inside([8, 4, 56, 28], 64, 32));
        assert!(!rect_inside([8, 0, 57, 32], 64, 32), "past the right edge");
        assert!(!rect_inside([0, 0, 0, 32], 64, 32), "empty");
        assert!(!rect_inside([u32::MAX, 0, 2, 2], 64, 32), "overflow");
    }

    #[test]
    fn a_held_eye_survives_its_source_being_redrawn() {
        let name = ChannelName::new(format!(r"Local\monaka-test-hold-{}", std::process::id())).unwrap();
        let (device12, queue) = gpu12();
        let mut producer = FenceProducer::create(&name, &device12, &queue, 64, 32, DXGI_FORMAT_R8G8B8A8_UNORM).unwrap();
        let described = describe(&name).unwrap();
        let (device, context, _) = create_device(Some(&find_adapter(described.adapter).unwrap())).unwrap();
        let mut reader = FenceReader::open(&name, &device).unwrap();
        let targets: [ID3D11Texture2D; 2] =
            std::array::from_fn(|_| create_texture(&device, &simple_texture_desc(64, 32, DXGI_FORMAT_R8G8B8A8_UNORM)).unwrap());
        let targets = [&targets[0], &targets[1]];

        assert!(producer.held().is_none());
        let left = cleared(&device12, &queue, [0.0, 0.0, 1.0, 1.0]);
        assert!(producer.hold(&left, RT).unwrap());
        // The same back buffer is drawn over for the other eye.
        let right = cleared(&device12, &queue, [1.0, 1.0, 0.0, 1.0]);
        let held = producer.held().unwrap();
        assert!(producer.publish((&held, D3D12_RESOURCE_STATE_COMMON), (&right, RT), 9).unwrap());
        until(|| reader.read(&context, targets, 0).unwrap());
        assert_eq!(read_pixel(&device, &context, targets[0], 3, 3).unwrap(), [0, 0, 255, 255]);
        assert_eq!(read_pixel(&device, &context, targets[1], 3, 3).unwrap(), [255, 255, 0, 255]);
    }
}
