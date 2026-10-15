//! Small Direct3D 12 helpers shared by the fence transport and the D3D12 producers: barriers and
//! copy locations that borrow resources without touching their reference counts, private
//! textures, fence waits, a ring of command lists that never waits for the GPU, and read-back of
//! a texture to the CPU for probes ([`read_texture`]). Compute passes (shaders, root signatures,
//! pipelines, descriptor heaps) are in [`compute`].

pub mod compute;

use std::mem::ManuallyDrop;
use windows::Win32::Foundation::WAIT_OBJECT_0;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_SAMPLE_DESC};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::core::{Interface, Result};

/// `resource` as a barrier/copy field, borrowed: no AddRef, and ManuallyDrop skips the Release.
fn borrowed(resource: &ID3D12Resource) -> ManuallyDrop<Option<ID3D12Resource>> {
    // SAFETY: copies the interface pointer without AddRef; the caller never drops it.
    ManuallyDrop::new(Some(unsafe { std::mem::transmute_copy(resource) }))
}

pub fn transition(resource: &ID3D12Resource, before: D3D12_RESOURCE_STATES, after: D3D12_RESOURCE_STATES) -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                pResource: borrowed(resource),
                Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                StateBefore: before,
                StateAfter: after,
            }),
        },
    }
}

/// Up to `N` barriers gathered on the stack (a publish's, without allocating).
pub struct Barriers<const N: usize> {
    list: [D3D12_RESOURCE_BARRIER; N],
    len: usize,
}

impl<const N: usize> Barriers<N> {
    pub fn new() -> Self {
        Self { list: std::array::from_fn(|_| D3D12_RESOURCE_BARRIER::default()), len: 0 }
    }

    /// Adds `barrier`; more than `N` is a caller's bug.
    pub fn push(&mut self, barrier: D3D12_RESOURCE_BARRIER) {
        assert!(self.len < N, "more than {N} barriers");
        self.list[self.len] = barrier;
        self.len += 1;
    }

    pub fn as_slice(&self) -> &[D3D12_RESOURCE_BARRIER] {
        &self.list[..self.len]
    }
}

impl<const N: usize> Default for Barriers<N> {
    fn default() -> Self {
        Self::new()
    }
}

/// Records a transition unless the states are equal.
pub fn barrier(list: &ID3D12GraphicsCommandList, resource: &ID3D12Resource, before: D3D12_RESOURCE_STATES, after: D3D12_RESOURCE_STATES) {
    if before != after {
        // SAFETY: records a barrier on a list being recorded.
        unsafe { list.ResourceBarrier(&[transition(resource, before, after)]) };
    }
}

pub fn uav_barrier(list: &ID3D12GraphicsCommandList, resource: &ID3D12Resource) {
    let barrier = D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_UAV,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 { UAV: ManuallyDrop::new(D3D12_RESOURCE_UAV_BARRIER { pResource: borrowed(resource) }) },
    };
    // SAFETY: records a barrier on a list being recorded.
    unsafe { list.ResourceBarrier(&[barrier]) };
}

pub fn subresource(resource: &ID3D12Resource, index: u32) -> D3D12_TEXTURE_COPY_LOCATION {
    D3D12_TEXTURE_COPY_LOCATION {
        pResource: borrowed(resource),
        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: index },
    }
}

/// A subresource's layout `footprint` inside `buffer`, as a copy location borrowed like
/// [`subresource`]: the other end of a texture-to-buffer copy.
pub fn placed(buffer: &ID3D12Resource, footprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT) -> D3D12_TEXTURE_COPY_LOCATION {
    D3D12_TEXTURE_COPY_LOCATION {
        pResource: borrowed(buffer),
        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { PlacedFootprint: footprint },
    }
}

/// A CPU-readable buffer of `bytes` in COPY_DEST (the state a readback heap's resources keep).
pub fn readback_buffer(device: &ID3D12Device, bytes: u64) -> Result<ID3D12Resource> {
    let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_READBACK, ..Default::default() };
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: bytes,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        ..Default::default()
    };
    let mut out: Option<ID3D12Resource> = None;
    // SAFETY: creation with a valid descriptor into a local out-pointer.
    unsafe { device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_COPY_DEST, None, &mut out)? };
    out.ok_or_else(|| windows::core::Error::from_hresult(windows::Win32::Foundation::E_OUTOFMEMORY))
}

/// `rows` rows of `footprint` in a readback buffer, the first `row_bytes` of each, tightly packed.
/// Call it once the copy into the buffer has finished on the GPU.
pub fn read_rows(buffer: &ID3D12Resource, footprint: &D3D12_PLACED_SUBRESOURCE_FOOTPRINT, rows: u32, row_bytes: usize) -> Result<Vec<u8>> {
    let (offset, pitch) = (footprint.Offset as usize, footprint.Footprint.RowPitch as usize);
    if row_bytes > pitch {
        return Err(windows::core::Error::from_hresult(windows::Win32::Foundation::E_INVALIDARG));
    }
    let end = if rows == 0 { offset } else { offset + (rows as usize - 1) * pitch + row_bytes };
    // SAFETY: `GetDesc` is a plain read.
    if (unsafe { buffer.GetDesc() }.Width as usize) < end {
        return Err(windows::core::Error::from_hresult(windows::Win32::Foundation::E_INVALIDARG));
    }
    let mut texels = Vec::with_capacity(rows as usize * row_bytes);
    let mut mapped = std::ptr::null_mut();
    // SAFETY: maps the read range of a readback buffer that holds it (checked above), copies each
    // row out within the mapping, and unmaps without writing.
    unsafe {
        buffer.Map(0, Some(&D3D12_RANGE { Begin: offset, End: end }), Some(&mut mapped))?;
        for y in 0..rows as usize {
            texels.extend_from_slice(std::slice::from_raw_parts(mapped.cast::<u8>().add(offset + y * pitch), row_bytes));
        }
        buffer.Unmap(0, Some(&D3D12_RANGE { Begin: 0, End: 0 }));
    }
    Ok(texels)
}

/// Reads subresource 0 of `texture` (plane 0 of a depth-stencil: its depth) back to the CPU: width,
/// height, bytes per row and the rows tightly packed. `texture` is in `state` and left there. The
/// copy runs on a recorder of its own on `queue` and this thread waits for it (up to 2 s), so it is
/// for probes, never for a frame's path.
pub fn read_texture(queue: &ID3D12CommandQueue, texture: &ID3D12Resource, state: D3D12_RESOURCE_STATES) -> Result<(u32, u32, usize, Vec<u8>)> {
    let device = device_of(queue)?;
    // SAFETY: a plain descriptor read.
    let desc = unsafe { texture.GetDesc() };
    let (mut footprint, mut rows, mut row_bytes, mut total) = (D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default(), 0u32, 0u64, 0u64);
    // SAFETY: layout query for one subresource into locals.
    unsafe { device.GetCopyableFootprints(&desc, 0, 1, 0, Some(&mut footprint), Some(&mut rows), Some(&mut row_bytes), Some(&mut total)) };
    if total == u64::MAX {
        return Err(windows::core::Error::from_hresult(windows::Win32::Foundation::E_INVALIDARG));
    }
    let buffer = readback_buffer(&device, total)?;
    let mut recorder = Recorder::new(&device, 1)?;
    let list = recorder.begin()?.expect("a fresh recorder is free");
    barrier(&list, texture, state, D3D12_RESOURCE_STATE_COPY_SOURCE);
    // SAFETY: records a copy between live resources on our own open list; `texture` and `buffer`
    // outlive its execution, which is waited for below.
    unsafe { list.CopyTextureRegion(&placed(&buffer, footprint), 0, 0, 0, &subresource(texture, 0), None) };
    barrier(&list, texture, D3D12_RESOURCE_STATE_COPY_SOURCE, state);
    recorder.submit(queue)?;
    if !recorder.idle(2000) {
        return Err(windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL));
    }
    let texels = read_rows(&buffer, &footprint, rows, row_bytes as usize)?;
    Ok((desc.Width as u32, desc.Height, row_bytes as usize, texels))
}

/// Copies plane 0 (a depth-stencil's depth) of `source` into `target`, with the transitions.
pub fn copy_plane0(list: &ID3D12GraphicsCommandList, target: &ID3D12Resource, source: &ID3D12Resource, state: D3D12_RESOURCE_STATES) {
    // SAFETY: records on a list being recorded; both resources are live for the list's execution.
    unsafe {
        list.ResourceBarrier(&[
            transition(source, state, D3D12_RESOURCE_STATE_COPY_SOURCE),
            transition(target, D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_COPY_DEST),
        ]);
        list.CopyTextureRegion(&subresource(target, 0), 0, 0, 0, &subresource(source, 0), None);
        list.ResourceBarrier(&[
            transition(source, D3D12_RESOURCE_STATE_COPY_SOURCE, state),
            transition(target, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COMMON),
        ]);
    }
}

/// Copies all of `source` (in `state`) into `target` (in COMMON); both end where they started.
pub fn copy_whole(list: &ID3D12GraphicsCommandList, target: &ID3D12Resource, source: &ID3D12Resource, state: D3D12_RESOURCE_STATES) {
    barrier(list, source, state, D3D12_RESOURCE_STATE_COPY_SOURCE);
    barrier(list, target, D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_COPY_DEST);
    // SAFETY: records on a list being recorded.
    unsafe { list.CopyResource(target, source) };
    barrier(list, source, D3D12_RESOURCE_STATE_COPY_SOURCE, state);
    barrier(list, target, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COMMON);
}

/// A private 2D texture in the common state.
pub fn texture(device: &ID3D12Device, width: u32, height: u32, format: DXGI_FORMAT, flags: D3D12_RESOURCE_FLAGS) -> Result<ID3D12Resource> {
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
    let mut out: Option<ID3D12Resource> = None;
    // SAFETY: creation with a valid descriptor into a local out-pointer.
    unsafe { device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_COMMON, None, &mut out)? };
    out.ok_or_else(|| windows::core::Error::from_hresult(windows::Win32::Foundation::E_OUTOFMEMORY))
}

/// The texture in `slot` when it is `width` x `height` in `format`; otherwise a new one made there
/// (with `flags`). `None` when it cannot be made.
pub fn fitting(slot: &mut Option<ID3D12Resource>, device: &ID3D12Device, width: u64, height: u32, format: DXGI_FORMAT, flags: D3D12_RESOURCE_FLAGS) -> Option<ID3D12Resource> {
    let fits = slot.as_ref().is_some_and(|t| {
        // SAFETY: reads a descriptor.
        let d = unsafe { t.GetDesc() };
        d.Width == width && d.Height == height && d.Format == format
    });
    if !fits {
        *slot = texture(device, width as u32, height, format, flags).ok();
    }
    slot.clone()
}

pub fn device_of(object: &impl Interface) -> Result<ID3D12Device> {
    let child: ID3D12DeviceChild = object.cast()?;
    let mut device: Option<ID3D12Device> = None;
    // SAFETY: queries the owning device into a local out-pointer.
    unsafe { child.GetDevice(&mut device)? };
    device.ok_or_else(|| windows::core::Error::from_hresult(windows::Win32::Foundation::E_NOINTERFACE))
}

/// Waits up to `timeout_ms` for `fence` to reach `value`.
pub fn wait_fence(fence: &ID3D12Fence, value: u64, timeout_ms: u32) -> bool {
    // SAFETY: fence and event calls on live objects; the event is closed below.
    unsafe {
        if value == 0 || fence.GetCompletedValue() >= value {
            return true;
        }
        let Ok(event) = CreateEventW(None, false, false, None) else { return false };
        let ok = fence.SetEventOnCompletion(value, event).is_ok() && WaitForSingleObject(event, timeout_ms) == WAIT_OBJECT_0;
        let _ = windows::Win32::Foundation::CloseHandle(event);
        ok
    }
}

struct Recording {
    allocator: ID3D12CommandAllocator,
    list: ID3D12GraphicsCommandList,
    done: u64,
}

/// A ring of direct command lists and the fence their completion is read from; a list is reused
/// only once the GPU is done with it, and nothing ever waits.
pub struct Recorder {
    fence: ID3D12Fence,
    value: u64,
    next: usize,
    recordings: Vec<Recording>,
}

impl Recorder {
    /// A ring of `count` lists with a fence of its own ([`Recorder::submit`] numbers the work).
    pub fn new(device: &ID3D12Device, count: usize) -> Result<Self> {
        // SAFETY: creation call on a live device.
        let fence: ID3D12Fence = unsafe { device.CreateFence(0, D3D12_FENCE_FLAG_NONE) }?;
        Self::on(device, count, fence)
    }

    /// A ring of `count` lists completing on `fence` with values the caller chooses
    /// ([`Recorder::submit_as`]): a fence shared with another process, signalled with a sequence.
    pub fn on(device: &ID3D12Device, count: usize, fence: ID3D12Fence) -> Result<Self> {
        // SAFETY: creation calls on a live device.
        unsafe {
            let mut recordings = Vec::with_capacity(count);
            for _ in 0..count {
                let allocator: ID3D12CommandAllocator = device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)?;
                let list: ID3D12GraphicsCommandList = device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None::<&ID3D12PipelineState>)?;
                list.Close()?;
                recordings.push(Recording { allocator, list, done: 0 });
            }
            Ok(Self { fence, value: 0, next: 0, recordings })
        }
    }

    /// Whether the next list is free (the GPU finished it); an error once the device is gone.
    pub fn ready(&self) -> Result<bool> {
        // SAFETY: a fence read.
        let completed = unsafe { self.fence.GetCompletedValue() };
        if completed == u64::MAX {
            return Err(windows::core::Error::from_hresult(windows::Win32::Graphics::Dxgi::DXGI_ERROR_DEVICE_REMOVED));
        }
        Ok(self.recordings[self.next].done <= completed)
    }

    /// The next list, reset and open; `Ok(None)` while the GPU still uses it (never waits).
    pub fn begin(&mut self) -> Result<Option<ID3D12GraphicsCommandList>> {
        if !self.ready()? {
            return Ok(None);
        }
        let recording = &self.recordings[self.next];
        // SAFETY: resets our own allocator and closed list, which the GPU is done with.
        unsafe {
            recording.allocator.Reset()?;
            recording.list.Reset(&recording.allocator, None::<&ID3D12PipelineState>)?;
        }
        Ok(Some(recording.list.clone()))
    }

    /// Closes the list `begin` returned, runs it on `queue` and signals the ring's fence with the
    /// next value of its own.
    pub fn submit(&mut self, queue: &ID3D12CommandQueue) -> Result<u64> {
        self.submit_as(queue, self.value + 1)
    }

    /// [`Recorder::submit`], signalling `value` (larger than any before) on the fence.
    pub fn submit_as(&mut self, queue: &ID3D12CommandQueue, value: u64) -> Result<u64> {
        let recording = &mut self.recordings[self.next];
        // SAFETY: closes and submits our own list.
        unsafe {
            recording.list.Close()?;
            queue.ExecuteCommandLists(&[Some(recording.list.cast::<ID3D12CommandList>()?)]);
            queue.Signal(&self.fence, value)?;
        }
        self.value = value;
        recording.done = value;
        self.next = (self.next + 1) % self.recordings.len();
        Ok(value)
    }

    /// Waits (at most `timeout_ms`) for everything submitted; false if the GPU did not finish.
    pub fn idle(&self, timeout_ms: u32) -> bool {
        wait_fence(&self.fence, self.value, timeout_ms)
    }
}

#[cfg(test)]
mod tests;
