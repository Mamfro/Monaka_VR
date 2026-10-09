//! Stereo pairs from a D3D12 producer to a D3D11 reader: three slots of paired eye textures and
//! two shared fences. Handles travel by `DuplicateHandle` out of the producer process. Neither
//! side ever waits; the slot rules are in [`monaka_core::fence_rules`].
//!
//! Wire-compatible with `GpuFenceProducer` / `GpuFenceReader` in `gpu_fence_channel.h`.

use crate::d3d::{self, adapter_luid, luid};
use crate::d3d12::{self, Recorder, transition};
use crate::shared::{Create, NamedMutex, OwnedHandle, SharedBlock};
use crate::{ChannelName, Error, Result};
use monaka_core::fence_rules::{choose_slot, reader_take};
use monaka_core::protocol::{FENCE_MAGIC, FENCE_SLOTS, FENCE_VERSION, FenceHeader, Luid, NO_SLOT, dimensions_ok, fence_format_ok};
use windows::Win32::Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle, GENERIC_ALL, HANDLE};
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11Device1, ID3D11Device5, ID3D11DeviceContext, ID3D11DeviceContext4, ID3D11Fence, ID3D11Texture2D};
use windows::Win32::Graphics::Direct3D12::{
    D3D12_COMMAND_LIST_TYPE_DIRECT, D3D12_FENCE_FLAG_SHARED, D3D12_HEAP_FLAG_SHARED, D3D12_HEAP_PROPERTIES, D3D12_HEAP_TYPE_DEFAULT,
    D3D12_RESOURCE_DESC, D3D12_RESOURCE_DIMENSION_TEXTURE2D, D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET, D3D12_RESOURCE_FLAG_NONE,
    D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATES, ID3D12CommandQueue,
    ID3D12Device, ID3D12Fence, ID3D12Resource,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_SAMPLE_DESC};
use windows::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId, OpenProcess, PROCESS_DUP_HANDLE};
use windows::core::Interface;

pub use crate::keyed::Pair;

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
    textures: [[ID3D11Texture2D; 2]; FENCE_SLOTS],
    produced: ID3D11Fence,
    consumed: ID3D11Fence,
    description: Description,
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
        let duplicate = |value: u64| -> Result<OwnedHandle> {
            let mut local = HANDLE::default();
            // SAFETY: duplicates a handle value published by the producer into this process.
            unsafe {
                DuplicateHandle(
                    producer.raw(),
                    HANDLE(value as usize as *mut _),
                    GetCurrentProcess(),
                    &mut local,
                    0,
                    false,
                    DUPLICATE_SAME_ACCESS,
                )?
            };
            OwnedHandle::new(local).ok_or(Error::Incompatible("published handle"))
        };
        let open_texture = |value: u64| -> Result<ID3D11Texture2D> {
            let local = duplicate(value)?;
            // SAFETY: opens a shared D3D12 texture on our D3D11 device; D3D keeps its own reference.
            Ok(unsafe { device1.OpenSharedResource1(local.raw()) }?)
        };
        let mut slots = Vec::with_capacity(FENCE_SLOTS);
        for slot in record.textures {
            slots.push([open_texture(slot[0])?, open_texture(slot[1])?]);
        }
        let textures: [[ID3D11Texture2D; 2]; FENCE_SLOTS] = slots.try_into().map_err(|_| Error::Incompatible("slots"))?;
        let open_fence = |value: u64| -> Result<ID3D11Fence> {
            let local = duplicate(value)?;
            let mut fence = None;
            // SAFETY: opens a shared fence on our device into a local out-pointer.
            unsafe { device5.OpenSharedFence(local.raw(), &mut fence) }?;
            fence.ok_or(Error::Incompatible("shared fence"))
        };
        let produced = open_fence(record.produced)?;
        let consumed = open_fence(record.consumed)?;
        drop(record);
        Ok(Self { header, _lease: lease, device: device.clone(), textures, produced, consumed, description, busy: 0, idle: 0 })
    }

    pub fn description(&self) -> Description {
        self.description
    }

    /// Copies the newest pair into `eyes` once its GPU copy has finished and it is newer than
    /// `seen`, then signals `consumed` so the producer may reuse the slot. `Ok(None)`: nothing new.
    pub fn read(&mut self, context: &ID3D11DeviceContext, eyes: [&ID3D11Texture2D; 2], seen: u64) -> Result<Option<Pair>> {
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
            (slot as usize, Pair { sequence, tick: record.slot_ticks[slot as usize] })
        };
        // SAFETY: copies a completed slot into our textures, then signals when the copy has run.
        unsafe {
            for (target, source) in eyes.into_iter().zip(&self.textures[slot]) {
                context.CopyResource(target, source);
            }
            context4.Signal(&self.consumed, pair.sequence)?;
            context.Flush();
        }
        Ok(Some(pair))
    }
}

/// GPU objects of a producer; leaked on purpose if the GPU never finishes with them.
struct ProducerGpu {
    queue: ID3D12CommandQueue,
    textures: [[ID3D12Resource; 2]; FENCE_SLOTS],
    consumed: ID3D12Fence,
    /// Completes on the shared `produced` fence, signalled with each pair's sequence.
    recorder: Recorder,
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
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
    sequence: u64,
}

const RECORDINGS: usize = 4;

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

            let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() };
            let desc = D3D12_RESOURCE_DESC {
                Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
                Width: width as u64,
                Height: height,
                DepthOrArraySize: 1,
                MipLevels: 1,
                Format: format,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Flags: D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
                ..Default::default()
            };
            let mut handles = Vec::new();
            let mut share = |object: &windows::Win32::Graphics::Direct3D12::ID3D12DeviceChild| -> Result<u64> {
                let handle = device.CreateSharedHandle(object, None, GENERIC_ALL.0, None)?;
                let value = handle.0 as usize as u64;
                handles.push(OwnedHandle::new(handle).ok_or(Error::Incompatible("shared handle"))?);
                Ok(value)
            };
            let mut published = [[0u64; 2]; FENCE_SLOTS];
            let mut slots = Vec::with_capacity(FENCE_SLOTS);
            for slot in published.iter_mut() {
                let mut pair = Vec::with_capacity(2);
                for value in slot.iter_mut() {
                    let mut texture: Option<ID3D12Resource> = None;
                    device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_SHARED, &desc, D3D12_RESOURCE_STATE_COMMON, None, &mut texture)?;
                    let texture = texture.expect("texture created");
                    *value = share(&texture.cast()?)?;
                    pair.push(texture);
                }
                slots.push([pair.remove(0), pair.remove(0)]);
            }
            let produced: ID3D12Fence = device.CreateFence(0, D3D12_FENCE_FLAG_SHARED)?;
            let consumed: ID3D12Fence = device.CreateFence(0, D3D12_FENCE_FLAG_SHARED)?;
            let produced_handle = share(&produced.cast()?)?;
            let consumed_handle = share(&consumed.cast()?)?;
            let recorder = Recorder::on(device, RECORDINGS, produced)?;

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
            let textures = slots.try_into().map_err(|_| Error::Incompatible("slots"))?;
            let gpu = ProducerGpu { queue: queue.clone(), textures, consumed, recorder, _handles: handles };
            Ok(Self { header, gpu: Some(gpu), hold: None, width, height, format, sequence: 0 })
        }
    }

    /// The sequence the next successful publish will carry, so its pose record can go out first.
    pub fn next_sequence(&self) -> u64 {
        self.sequence + 1
    }

    /// Copies the two eye images (in the given states) into a free slot on the queue and publishes
    /// them. `Ok(false)`: skipped because no slot or recording was free; it never waits.
    pub fn publish(&mut self, left: (&ID3D12Resource, D3D12_RESOURCE_STATES), right: (&ID3D12Resource, D3D12_RESOURCE_STATES), tick: u64) -> Result<bool> {
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
        // SAFETY: fence reads, command recording on our own lists, and queue submission.
        unsafe {
            if !gpu.recorder.ready()? {
                return Ok(false);
            }
            let sequence = self.sequence + 1;
            // The slot is marked with this pair before its copy is submitted, in the same lock as it
            // is chosen: the reader never takes it while it is being rewritten.
            let (slot, before) = {
                let Some(mut record) = self.header.lock(0).tolerant() else { return Ok(false) };
                let Some(slot) = choose_slot(&record, gpu.consumed.GetCompletedValue()) else { return Ok(false) };
                let slot = slot as usize;
                let before = (record.slot_sequences[slot], record.slot_ticks[slot]);
                (record.slot_sequences[slot], record.slot_ticks[slot]) = (sequence, tick);
                (slot, before)
            };
            let list = match gpu.recorder.begin() {
                Ok(Some(list)) => list,
                failed => {
                    // Not copied after all: the slot holds what it held.
                    if let Some(mut record) = self.header.lock(0).tolerant() {
                        (record.slot_sequences[slot], record.slot_ticks[slot]) = before;
                    }
                    return Ok(failed.map(|_| false)?);
                }
            };
            let list = &list;
            let same = left.0 == right.0;
            let targets = &gpu.textures[slot];
            let mut before = Vec::with_capacity(4);
            let mut after = Vec::with_capacity(4);
            for (eye, (source, state)) in sources.iter().enumerate() {
                if !(same && eye == 1) {
                    before.push(transition(source, *state, D3D12_RESOURCE_STATE_COPY_SOURCE));
                    after.push(transition(source, D3D12_RESOURCE_STATE_COPY_SOURCE, *state));
                }
                before.push(transition(&targets[eye], D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_COPY_DEST));
                after.push(transition(&targets[eye], D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COMMON));
            }
            list.ResourceBarrier(&before);
            for (eye, (source, _)) in sources.iter().enumerate() {
                list.CopyResource(&targets[eye], *source);
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
}

impl Drop for FenceProducer {
    /// Frees the GPU objects once the GPU is done with them. If it is not done within five seconds
    /// they are leaked on purpose rather than freed under a running copy.
    fn drop(&mut self) {
        if let Some(hold) = self.hold.take()
            && !hold.recorder.idle(5000)
        {
            std::mem::forget(hold);
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
        D3D12_COMMAND_QUEUE_DESC, D3D12_DESCRIPTOR_HEAP_DESC, D3D12_DESCRIPTOR_HEAP_TYPE_RTV, D3D12_HEAP_FLAG_NONE,
        D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12CreateDevice, ID3D12CommandAllocator, ID3D12CommandList, ID3D12DescriptorHeap,
        ID3D12GraphicsCommandList, ID3D12PipelineState,
    };
    use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM;

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
