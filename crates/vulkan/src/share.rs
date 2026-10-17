//! Each present's frame out of the game's Vulkan device into a D3D12 device of Monaka's own: its
//! colour (the swapchain image) and, when the game names them, its depth ([`on_depth`]) and its
//! UI layer (the game's HUD drawn in an image of its own and kept out of the frame, [`on_layer`]).
//! The D3D12
//! device (on the game's GPU) makes a ring of shared buffers and two shared fences; the game's
//! device imports them. At each present, on the game's queue: the images are copied into the ring's
//! next buffers, and the "produced" fence signalled with the frame's number. On the D3D12 queue:
//! the copy waits for that number, the buffers go into textures, the "consumed" fence is
//! signalled, and the game's frame handler ([`on_frame`]) gets the textures. A buffer is written
//! again only once D3D12 consumed it; nothing waits on the CPU (a busy ring skips the frame).

use crate::device::{self, Device};
use crate::ffi::*;
use monaka_channel::d3d12::compute::{self, DescriptorHeap, Table};
use monaka_channel::d3d12::{self as dx, Recorder};
use monaka_core::sync::Locked;
use monaka_producer::{log, log_first};
use std::ffi::{CStr, c_void};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Mutex, OnceLock};
use windows::Win32::Foundation::{CloseHandle, GENERIC_ALL, HANDLE};
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::Interface;

/// One frame in Monaka's D3D12 device (in COMMON, ready once the queue reaches them): its colour,
/// its format and size; its depth (32-bit float, its own size) when the game named one; its UI
/// layer (the image's bytes as they are, its own size and format) when the game named one at this
/// present; its number.
pub struct Frame<'a> {
    pub device: &'a ID3D12Device,
    pub queue: &'a ID3D12CommandQueue,
    pub color: &'a ID3D12Resource,
    pub format: DXGI_FORMAT,
    pub size: (u32, u32),
    pub depth: Option<&'a ID3D12Resource>,
    pub layer: Option<&'a ID3D12Resource>,
    pub number: u64,
}

/// A depth image of the game's to go out with the frame: the image, the layout it is in at the
/// present (it is put back in it), its Vulkan format and size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Depth {
    pub image: VkImage,
    pub layout: i32,
    pub format: i32,
    pub size: (u32, u32),
}

/// A colour image of the game's to go out with the frame as its UI layer: the image, the layout it
/// is in at the present (it is put back in it), its Vulkan format and size (as the record noted it,
/// `device::color_image`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layer {
    pub image: VkImage,
    pub layout: i32,
    pub format: i32,
    pub size: (u32, u32),
}

/// What a game does with each frame (publishes it, warps it), on the game's present thread.
pub type FrameFn = fn(&Frame<'_>);
/// The game's depth image at a present, if it has one now.
pub type DepthFn = fn() -> Option<Depth>;
/// The game's UI layer at a present, if its HUD went into one this frame.
pub type LayerFn = fn() -> Option<Layer>;

static ON_FRAME: OnceLock<FrameFn> = OnceLock::new();
static ON_DEPTH: OnceLock<DepthFn> = OnceLock::new();
static ON_LAYER: OnceLock<LayerFn> = OnceLock::new();
static SHARING: Mutex<Option<Sharing>> = Mutex::new(None);
static BROKEN: Mutex<Option<String>> = Mutex::new(None);
static PRESENTS: AtomicU64 = AtomicU64::new(0);
static SHARED: AtomicU64 = AtomicU64::new(0);
static WITH_DEPTH: AtomicU64 = AtomicU64::new(0);
static WITH_LAYER: AtomicU64 = AtomicU64::new(0);
static BUSY: AtomicU64 = AtomicU64::new(0);

/// The game's frame handler.
pub fn on_frame(handler: FrameFn) {
    let _ = ON_FRAME.set(handler);
}

/// Where the game's depth image is at each present.
pub fn on_depth(depth: DepthFn) {
    let _ = ON_DEPTH.set(depth);
}

/// Where the game's UI layer is at each present.
pub fn on_layer(layer: LayerFn) {
    let _ = ON_LAYER.set(layer);
}

pub fn summary() -> String {
    let broken = BROKEN.locked().clone().map_or(String::new(), |why| format!("; stopped: {why}"));
    format!(
        "{} presents, {} frames shared ({} with depth, {} with a UI layer), {} skipped (ring busy); {}{broken}",
        PRESENTS.load(Relaxed),
        SHARED.load(Relaxed),
        WITH_DEPTH.load(Relaxed),
        WITH_LAYER.load(Relaxed),
        BUSY.load(Relaxed),
        device::state()
    )
}

/// Lets go of the ring once both GPUs are done with it (left in place if they never are).
pub fn close() {
    if let Some(sharing) = SHARING.locked().take() {
        if sharing.recorder.idle(2000) {
            drop(sharing);
        } else {
            std::mem::forget(sharing);
        }
    }
}

/// Slots in the ring: a frame being written, one being read, one spare.
const SLOTS: usize = 3;

/// The device functions the sharing calls.
struct Fns {
    create_buffer: CreateBufferFn,
    buffer_requirements: GetBufferMemoryRequirementsFn,
    handle_properties: GetMemoryWin32HandlePropertiesFn,
    allocate_memory: AllocateMemoryFn,
    bind_buffer: BindBufferMemoryFn,
    create_pool: CreateCommandPoolFn,
    allocate_commands: AllocateCommandBuffersFn,
    begin: BeginCommandBufferFn,
    end: EndCommandBufferFn,
    reset: ResetCommandBufferFn,
    barrier: CmdPipelineBarrierFn,
    copy: CmdCopyImageToBufferFn,
    submit: QueueSubmitFn,
    create_semaphore: CreateSemaphoreFn,
    import_semaphore: ImportSemaphoreWin32HandleFn,
    create_fence: CreateFenceFn,
    fence_status: GetFenceStatusFn,
    reset_fences: ResetFencesFn,
}

impl Fns {
    fn load() -> Result<Self, String> {
        fn get<F>(name: &CStr) -> Result<F, String> {
            let address = device::function(name).ok_or_else(|| format!("{} not found", name.to_string_lossy()))?;
            // SAFETY: the device's function of this name has type F (the API's).
            Ok(unsafe { std::mem::transmute_copy(&address) })
        }
        Ok(Self {
            create_buffer: get(c"vkCreateBuffer")?,
            buffer_requirements: get(c"vkGetBufferMemoryRequirements")?,
            handle_properties: get(c"vkGetMemoryWin32HandlePropertiesKHR")?,
            allocate_memory: get(c"vkAllocateMemory")?,
            bind_buffer: get(c"vkBindBufferMemory")?,
            create_pool: get(c"vkCreateCommandPool")?,
            allocate_commands: get(c"vkAllocateCommandBuffers")?,
            begin: get(c"vkBeginCommandBuffer")?,
            end: get(c"vkEndCommandBuffer")?,
            reset: get(c"vkResetCommandBuffer")?,
            barrier: get(c"vkCmdPipelineBarrier")?,
            copy: get(c"vkCmdCopyImageToBuffer")?,
            submit: get(c"vkQueueSubmit")?,
            create_semaphore: get(c"vkCreateSemaphore")?,
            import_semaphore: get(c"vkImportSemaphoreWin32HandleKHR")?,
            create_fence: get(c"vkCreateFence")?,
            fence_status: get(c"vkGetFenceStatus")?,
            reset_fences: get(c"vkResetFences")?,
        })
    }
}

/// An image's way out: a shared buffer (both sides) and the texture it is read into.
struct Lane {
    buffer: VkBuffer,
    shared: ID3D12Resource,
    texture: ID3D12Resource,
}

/// One slot: its command buffer and the fence its submission signals, its lanes (colour, and depth
/// and the UI layer when the ring carries them), and the frame number it last carried.
struct Slot {
    commands: VkCommandBuffer,
    done: VkFence,
    color: Lane,
    depth: Option<Lane>,
    layer: Option<Lane>,
    number: u64,
}

/// What a lane carries: the image's size, its texel size in the buffer, the buffer's row pitch,
/// and the D3D12 format the texture reads it as.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Shape {
    size: (u32, u32),
    texel: u32,
    pitch: u32,
    format: DXGI_FORMAT,
}

impl Shape {
    fn new(size: (u32, u32), texel: u32, format: DXGI_FORMAT) -> Self {
        Self { size, texel, pitch: (size.0 * texel).next_multiple_of(D3D12_TEXTURE_DATA_PITCH_ALIGNMENT), format }
    }

    fn bytes(&self) -> u64 {
        u64::from(self.pitch) * u64::from(self.size.1)
    }
}

struct Sharing {
    fns: Fns,
    device: VkDevice,
    family: u32,
    slots: Vec<Slot>,
    produced: (ID3D12Fence, VkSemaphore),
    consumed: (ID3D12Fence, VkSemaphore),
    number: u64,
    d3d: ID3D12Device,
    queue: ID3D12CommandQueue,
    recorder: Recorder,
    /// The swapchain image's Vulkan format and lane; the depth's Vulkan format and lane, and the
    /// pass making 24-bit depth a float; the UI layer's Vulkan format and lane.
    vk_format: i32,
    color: Shape,
    depth: Option<(i32, Shape)>,
    unpack: Option<Unpack24>,
    layer: Option<(i32, Shape)>,
}

// SAFETY: plain handles and free-threaded D3D12 objects, used under the lock.
unsafe impl Send for Sharing {}

/// The D3D12 format a colour image (the swapchain's, a UI layer) of Vulkan `format` is shared as
/// (its bytes as they are), and its bytes per texel.
fn color_format(format: i32) -> Option<(DXGI_FORMAT, u32)> {
    match format {
        FORMAT_B8G8R8A8_UNORM | FORMAT_B8G8R8A8_SRGB => Some((DXGI_FORMAT_B8G8R8A8_UNORM, 4)),
        FORMAT_R8G8B8A8_UNORM | FORMAT_R8G8B8A8_SRGB => Some((DXGI_FORMAT_R8G8B8A8_UNORM, 4)),
        FORMAT_A2B10G10R10_UNORM_PACK32 => Some((DXGI_FORMAT_R10G10B10A2_UNORM, 4)),
        FORMAT_R16G16B16A16_SFLOAT => Some((DXGI_FORMAT_R16G16B16A16_FLOAT, 8)),
        _ => None,
    }
}

/// How a depth image's depth plane comes out of its copy (4 bytes a texel either way): a 32-bit
/// float as it is, or 24-bit unsigned normalised in the low bits (made a float on the D3D12 side,
/// [`Unpack24`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DepthKind {
    Float,
    Unorm24,
}

/// How a depth image of Vulkan `format` can go out, and whether it has a stencil plane (its
/// barriers then name both).
fn depth_format(format: i32) -> Option<(DepthKind, bool)> {
    match format {
        FORMAT_D32_SFLOAT => Some((DepthKind::Float, false)),
        FORMAT_D32_SFLOAT_S8_UINT => Some((DepthKind::Float, true)),
        FORMAT_D24_UNORM_S8_UINT => Some((DepthKind::Unorm24, true)),
        _ => None,
    }
}

/// 24-bit depth (in the low bits of each 4-byte texel of a shared buffer) into a 32-bit float
/// texture, on the D3D12 side: a compute pass, one buffer view and one texture view per slot.
struct Unpack24 {
    root: ID3D12RootSignature,
    pipeline: ID3D12PipelineState,
    views: DescriptorHeap,
}

const UNPACK24: &str = "cbuffer C : register(b0) { uint Pitch; uint Width; uint Height; };\n\
    ByteAddressBuffer Depth : register(t0); RWTexture2D<float> Out : register(u0);\n\
    [numthreads(8, 8, 1)] void Main(uint3 id : SV_DispatchThreadID) {\n\
        if (id.x >= Width || id.y >= Height) return;\n\
        Out[id.xy] = (Depth.Load((id.y * Pitch + id.x) * 4) & 0xFFFFFF) / 16777215.0;\n\
    }";

impl Unpack24 {
    fn new(device: &ID3D12Device, slots: &[Slot], shape: &Shape) -> Result<Self, String> {
        let root = compute::root_signature(device, 3, &[Table { srvs: 1, uavs: 1 }])?;
        let pipeline = compute::shader_pipeline(device, &root, UNPACK24, "unpack24.hlsl", windows::core::s!("Main"))?;
        let views = DescriptorHeap::new(device, 2 * slots.len() as u32)?;
        for (i, slot) in slots.iter().enumerate() {
            let Some(lane) = &slot.depth else { continue };
            let raw = D3D12_SHADER_RESOURCE_VIEW_DESC {
                Format: DXGI_FORMAT_R32_TYPELESS,
                ViewDimension: D3D12_SRV_DIMENSION_BUFFER,
                Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 { Buffer: D3D12_BUFFER_SRV { FirstElement: 0, NumElements: (shape.bytes() / 4) as u32, StructureByteStride: 0, Flags: D3D12_BUFFER_SRV_FLAG_RAW } },
            };
            // SAFETY: views of our own resources into our own heap.
            unsafe {
                device.CreateShaderResourceView(&lane.shared, Some(&raw), views.cpu(2 * i as u32));
                device.CreateUnorderedAccessView(&lane.texture, None, Some(&compute::uav_desc(DXGI_FORMAT_R32_FLOAT)), views.cpu(2 * i as u32 + 1));
            }
        }
        Ok(Self { root, pipeline, views })
    }

    /// Records slot `index`'s unpacking (its texture from COMMON, back to COMMON).
    ///
    /// # Safety
    /// `list` is open on the device the pass was made on; `texture` is slot `index`'s depth texture.
    unsafe fn record(&self, list: &ID3D12GraphicsCommandList, index: usize, texture: &ID3D12Resource, shape: &Shape) {
        let constants = [shape.pitch / 4, shape.size.0, shape.size.1];
        dx::barrier(list, texture, D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
        // SAFETY: the caller's guarantee; our own pass, views and texture.
        unsafe {
            list.SetDescriptorHeaps(&[Some(self.views.heap().clone())]);
            list.SetComputeRootSignature(&self.root);
            list.SetPipelineState(&self.pipeline);
            list.SetComputeRoot32BitConstants(0, 3, constants.as_ptr().cast(), 0);
            list.SetComputeRootDescriptorTable(1, self.views.gpu(2 * index as u32));
            list.Dispatch(shape.size.0.div_ceil(8), shape.size.1.div_ceil(8), 1);
        }
        dx::barrier(list, texture, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COMMON);
    }
}

/// This copy's present handler (`crate::device::PresentHandler`): [`present`].
///
/// # Safety
/// Called from the game's present with its queue, swapchain and image index.
pub unsafe extern "system" fn present_handler(queue: VkQueue, swapchain: VkSwapchainKHR, index: u32) {
    let _flight = monaka_hook::InFlight::enter();
    present(queue, swapchain, index);
}

/// At the game's present of `swapchain`'s image `index` on `queue` (before it presents): the image
/// (and the game's depth) copied out and handed over.
pub fn present(queue: VkQueue, swapchain: VkSwapchainKHR, index: u32) {
    PRESENTS.fetch_add(1, Relaxed);
    if BROKEN.locked().is_some() {
        return;
    }
    let (Some(device), Some(chain), Some(family)) = (device::device(), device::swapchain(swapchain), device::family_of(queue)) else {
        log_first(3, format_args!("a present before the sharing can run: {}", device::state()));
        return;
    };
    let Some(&image) = chain.images[..chain.count as usize].get(index as usize) else { return };
    let depth = ON_DEPTH.get().and_then(|depth| depth()).filter(|d| {
        let usable = depth_format(d.format).is_some();
        if !usable {
            log_first(1, format_args!("the game's depth (format {}) cannot go out (32-bit float or 24-bit depth only)", d.format));
        }
        usable
    });
    let layer = ON_LAYER.get().and_then(|layer| layer()).filter(|l| {
        let usable = color_format(l.format).is_some();
        if !usable {
            log_first(1, format_args!("the game's UI layer (format {}) cannot go out", l.format));
        }
        usable
    });
    let mut guard = SHARING.locked();
    let size = (chain.width, chain.height);
    let depth_shape = depth.map(|d| (d.format, d.size));
    let layer_shape = layer.map(|l| (l.format, l.size));
    // A frame without the depth (or layer) found goes out on the ring as it is; a new shape (or
    // depth or a layer found for a ring made without) makes a new ring.
    let fits = guard.as_ref().is_some_and(|s| {
        s.color.size == size
            && s.vk_format == chain.format
            && s.family == family
            && (depth_shape.is_none() || s.depth.map(|(f, shape)| (f, shape.size)) == depth_shape)
            && (layer_shape.is_none() || s.layer.map(|(f, shape)| (f, shape.size)) == layer_shape)
    });
    if !fits {
        // Keep what the old ring carried that this frame did not name.
        let (depth_shape, layer_shape) = match guard.as_ref() {
            Some(old) => (depth_shape.or(old.depth.map(|(f, s)| (f, s.size))), layer_shape.or(old.layer.map(|(f, s)| (f, s.size)))),
            None => (depth_shape, layer_shape),
        };
        if let Some(old) = guard.take() {
            // A new shape: the old ring stays (the GPUs may still use it).
            std::mem::forget(old);
        }
        match Sharing::new(&device, family, size, chain.format, depth_shape, layer_shape) {
            Ok(made) => {
                let depth = depth_shape.map_or("no depth".to_owned(), |(f, s)| format!("depth {}x{} format {f}", s.0, s.1));
                let layer = layer_shape.map_or("no UI layer".to_owned(), |(f, s)| format!("UI layer {}x{} format {f}", s.0, s.1));
                log!("frames shared: {}x{} format {} ({:?}), {depth}, {layer}, {} slots", size.0, size.1, chain.format, made.color.format, SLOTS);
                *guard = Some(made);
            }
            Err(why) => {
                log!("frames cannot be shared: {why}");
                *BROKEN.locked() = Some(why);
                return;
            }
        }
    }
    let Some(sharing) = guard.as_mut() else { return };
    match sharing.share(queue, image, depth, layer) {
        Ok(Some((slot, with_layer))) => {
            SHARED.fetch_add(1, Relaxed);
            let slot = &sharing.slots[slot];
            if slot.depth.is_some() {
                WITH_DEPTH.fetch_add(1, Relaxed);
            }
            if with_layer {
                WITH_LAYER.fetch_add(1, Relaxed);
            }
            if let Some(handler) = ON_FRAME.get() {
                handler(&Frame {
                    device: &sharing.d3d,
                    queue: &sharing.queue,
                    color: &slot.color.texture,
                    format: sharing.color.format,
                    size: sharing.color.size,
                    depth: slot.depth.as_ref().map(|lane| &lane.texture),
                    layer: slot.layer.as_ref().filter(|_| with_layer).map(|lane| &lane.texture),
                    number: slot.number,
                });
            }
        }
        Ok(None) => {
            BUSY.fetch_add(1, Relaxed);
        }
        Err(why) => {
            log!("frame sharing stopped: {why}");
            *BROKEN.locked() = Some(why);
        }
    }
}

fn check(result: VkResult, what: &str) -> Result<(), String> {
    if result == VK_SUCCESS { Ok(()) } else { Err(format!("{what} failed ({result})")) }
}

/// A barrier of `image`'s first mip and layer (`aspect`) from `old` to `new`.
fn barrier(image: VkImage, aspect: u32, old: i32, new: i32, src: u32, dst: u32) -> VkImageMemoryBarrier {
    VkImageMemoryBarrier {
        sType: STRUCTURE_IMAGE_MEMORY_BARRIER,
        pNext: std::ptr::null(),
        srcAccessMask: src,
        dstAccessMask: dst,
        oldLayout: old,
        newLayout: new,
        srcQueueFamilyIndex: QUEUE_FAMILY_IGNORED,
        dstQueueFamilyIndex: QUEUE_FAMILY_IGNORED,
        image,
        subresourceRange: VkImageSubresourceRange { aspectMask: aspect, baseMipLevel: 0, levelCount: 1, baseArrayLayer: 0, layerCount: 1 },
    }
}

/// A copy of an image's first mip (`aspect`) of `size` into a buffer of `shape`.
fn region(aspect: u32, shape: &Shape) -> VkBufferImageCopy {
    VkBufferImageCopy {
        bufferOffset: 0,
        bufferRowLength: shape.pitch / shape.texel,
        bufferImageHeight: shape.size.1,
        imageSubresource: VkImageSubresourceLayers { aspectMask: aspect, mipLevel: 0, baseArrayLayer: 0, layerCount: 1 },
        imageOffset: [0; 3],
        imageExtent: [shape.size.0, shape.size.1, 1],
    }
}

impl Sharing {
    fn new(device: &Device, family: u32, size: (u32, u32), vk_format: i32, depth: Option<(i32, (u32, u32))>, layer: Option<(i32, (u32, u32))>) -> Result<Self, String> {
        let fns = Fns::load()?;
        let (format, texel) = color_format(vk_format).ok_or_else(|| format!("swapchain format {vk_format} not handled"))?;
        let color = Shape::new(size, texel, format);
        let depth = depth.map(|(f, size)| (f, Shape::new(size, 4, DXGI_FORMAT_R32_FLOAT)));
        let layer = layer.and_then(|(f, size)| color_format(f).map(|(format, texel)| (f, Shape::new(size, texel, format))));
        let depth_kind = depth.and_then(|(f, _)| depth_format(f)).map(|(kind, _)| kind);
        let (d3d, queue) = d3d12(device.luid)?;
        let recorder = Recorder::new(&d3d, SLOTS + 1).map_err(|e| e.to_string())?;
        let vk = device.device;
        let produced = shared_fence(&fns, vk, &d3d)?;
        let consumed = shared_fence(&fns, vk, &d3d)?;
        let pool = command_pool(&fns, vk, family)?;
        let mut slots = Vec::with_capacity(SLOTS);
        for _ in 0..SLOTS {
            let (commands, done) = command_buffer(&fns, vk, pool)?;
            let color_lane = lane(&fns, device, &d3d, &color, D3D12_RESOURCE_FLAG_NONE)?;
            // 24-bit depth is written by the unpacking pass, so its texture takes unordered access.
            let flags = if depth_kind == Some(DepthKind::Unorm24) { D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS } else { D3D12_RESOURCE_FLAG_NONE };
            let depth_lane = depth.map(|(_, shape)| lane(&fns, device, &d3d, &shape, flags)).transpose()?;
            let layer_lane = layer.map(|(_, shape)| lane(&fns, device, &d3d, &shape, D3D12_RESOURCE_FLAG_NONE)).transpose()?;
            slots.push(Slot { commands, done, color: color_lane, depth: depth_lane, layer: layer_lane, number: 0 });
        }
        let unpack = match (depth_kind, depth) {
            (Some(DepthKind::Unorm24), Some((_, shape))) => Some(Unpack24::new(&d3d, &slots, &shape)?),
            _ => None,
        };
        Ok(Self { fns, device: vk, family, slots, produced, consumed, number: 0, d3d, queue, recorder, vk_format, color, depth, unpack, layer })
    }

    /// `image` (the swapchain's, in PRESENT_SRC), `depth` and the UI `layer` copied into the next
    /// slot on the game's `queue`, then read into the slot's textures on ours. The slot and whether
    /// it carries this frame's layer, or none while the ring is busy.
    fn share(&mut self, queue: VkQueue, image: VkImage, depth: Option<Depth>, layer: Option<Layer>) -> Result<Option<(usize, bool)>, String> {
        let next = self.number + 1;
        let index = (next % SLOTS as u64) as usize;
        let fns = &self.fns;
        let slot = &self.slots[index];
        // SAFETY: a status read of our own fence.
        if unsafe { (fns.fence_status)(self.device, slot.done) } != VK_SUCCESS {
            return Ok(None);
        }
        let commands = slot.commands;
        let begin = VkCommandBufferBeginInfo { sType: STRUCTURE_COMMAND_BUFFER_BEGIN_INFO, pNext: std::ptr::null(), flags: COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT, pInheritanceInfo: std::ptr::null() };
        let mut before = vec![barrier(image, IMAGE_ASPECT_COLOR, IMAGE_LAYOUT_PRESENT_SRC_KHR, IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL, ACCESS_MEMORY_WRITE, ACCESS_TRANSFER_READ)];
        let mut after = vec![barrier(image, IMAGE_ASPECT_COLOR, IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL, IMAGE_LAYOUT_PRESENT_SRC_KHR, ACCESS_TRANSFER_READ, 0)];
        let depth_copy = match (depth, &slot.depth, self.depth) {
            (Some(depth), Some(lane), Some((_, shape))) => {
                let aspect = if depth_format(depth.format).is_some_and(|(_, stencil)| stencil) { IMAGE_ASPECT_DEPTH | IMAGE_ASPECT_STENCIL } else { IMAGE_ASPECT_DEPTH };
                before.push(barrier(depth.image, aspect, depth.layout, IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL, ACCESS_MEMORY_WRITE, ACCESS_TRANSFER_READ));
                after.push(barrier(depth.image, aspect, IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL, depth.layout, ACCESS_TRANSFER_READ, ACCESS_MEMORY_READ | ACCESS_MEMORY_WRITE));
                Some((depth.image, lane.buffer, region(IMAGE_ASPECT_DEPTH, &shape)))
            }
            _ => None,
        };
        let layer_copy = match (layer, &slot.layer, self.layer) {
            (Some(layer), Some(lane), Some((_, shape))) => {
                before.push(barrier(layer.image, IMAGE_ASPECT_COLOR, layer.layout, IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL, ACCESS_MEMORY_WRITE, ACCESS_TRANSFER_READ));
                after.push(barrier(layer.image, IMAGE_ASPECT_COLOR, IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL, layer.layout, ACCESS_TRANSFER_READ, ACCESS_MEMORY_READ | ACCESS_MEMORY_WRITE));
                Some((layer.image, lane.buffer, region(IMAGE_ASPECT_COLOR, &shape)))
            }
            _ => None,
        };
        let color_region = region(IMAGE_ASPECT_COLOR, &self.color);
        // SAFETY: our own command buffer (its last submission finished: its fence is signalled),
        // recording copies of the game's images, on the game's present thread.
        unsafe {
            check((fns.reset_fences)(self.device, 1, &slot.done), "vkResetFences")?;
            check((fns.reset)(commands, 0), "vkResetCommandBuffer")?;
            check((fns.begin)(commands, &begin), "vkBeginCommandBuffer")?;
            (fns.barrier)(commands, PIPELINE_STAGE_ALL_COMMANDS, PIPELINE_STAGE_TRANSFER, 0, 0, std::ptr::null(), 0, std::ptr::null(), before.len() as u32, before.as_ptr());
            (fns.copy)(commands, image, IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL, slot.color.buffer, 1, &color_region);
            for (source, buffer, region) in depth_copy.iter().chain(&layer_copy) {
                (fns.copy)(commands, *source, IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL, *buffer, 1, region);
            }
            (fns.barrier)(commands, PIPELINE_STAGE_TRANSFER, PIPELINE_STAGE_ALL_COMMANDS, 0, 0, std::ptr::null(), 0, std::ptr::null(), after.len() as u32, after.as_ptr());
            check((fns.end)(commands), "vkEndCommandBuffer")?;
        }
        // Wait until D3D12 read what this slot carried before; then say this frame is in it.
        let wait_values = [slot.number];
        let signal_values = [next];
        let values = VkD3D12FenceSubmitInfoKHR {
            sType: STRUCTURE_D3D12_FENCE_SUBMIT_INFO_KHR,
            pNext: std::ptr::null(),
            waitSemaphoreValuesCount: 1,
            pWaitSemaphoreValues: wait_values.as_ptr(),
            signalSemaphoreValuesCount: 1,
            pSignalSemaphoreValues: signal_values.as_ptr(),
        };
        let wait_stage = [PIPELINE_STAGE_TRANSFER];
        let submit = VkSubmitInfo {
            sType: STRUCTURE_SUBMIT_INFO,
            pNext: &values as *const _ as *const c_void,
            waitSemaphoreCount: 1,
            pWaitSemaphores: &self.consumed.1,
            pWaitDstStageMask: wait_stage.as_ptr(),
            commandBufferCount: 1,
            pCommandBuffers: &commands,
            signalSemaphoreCount: 1,
            pSignalSemaphores: &self.produced.1,
        };
        // SAFETY: the game's queue, on its present thread (where the game uses it).
        check(unsafe { (fns.submit)(queue, 1, &submit, slot.done) }, "vkQueueSubmit")?;
        self.number = next;
        self.slots[index].number = next;
        self.read(index, depth_copy.is_some(), layer_copy.is_some())?;
        Ok(Some((index, layer_copy.is_some())))
    }

    /// Slot `index`'s buffers into its textures on our queue, once the game's copy is done; the
    /// consumed fence then signalled. The depth lane only when `with_depth`, the layer's only when
    /// `with_layer`.
    fn read(&mut self, index: usize, with_depth: bool, with_layer: bool) -> Result<(), String> {
        let slot = &self.slots[index];
        let color = (&slot.color, self.color);
        let depth = slot.depth.as_ref().filter(|_| with_depth).zip(self.depth.map(|(_, s)| s));
        let layer = slot.layer.as_ref().filter(|_| with_layer).zip(self.layer.map(|(_, s)| s));
        // SAFETY: GPU waits and copies on our own queue and resources.
        unsafe {
            self.queue.Wait(&self.produced.0, slot.number).map_err(|e| e.to_string())?;
            let list = loop {
                if let Some(list) = self.recorder.begin().map_err(|e| e.to_string())? {
                    break list;
                }
                // Our own ring of lists (one more than the slots) frees within a frame.
                std::thread::yield_now();
            };
            copy_in(&list, color.0, &color.1);
            match (depth, &self.unpack) {
                (Some((lane, shape)), Some(unpack)) => unpack.record(&list, index, &lane.texture, &shape),
                (Some((lane, shape)), None) => copy_in(&list, lane, &shape),
                (None, _) => {}
            }
            if let Some((lane, shape)) = layer {
                copy_in(&list, lane, &shape);
            }
            self.recorder.submit(&self.queue).map_err(|e| e.to_string())?;
            self.queue.Signal(&self.consumed.0, slot.number).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

/// Records `lane`'s buffer copied into its texture (from COMMON, back to COMMON).
///
/// # Safety
/// `list` is open on the lane's device.
unsafe fn copy_in(list: &ID3D12GraphicsCommandList, lane: &Lane, shape: &Shape) {
    let footprint = D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
        Offset: 0,
        Footprint: D3D12_SUBRESOURCE_FOOTPRINT { Format: shape.format, Width: shape.size.0, Height: shape.size.1, Depth: 1, RowPitch: shape.pitch },
    };
    // SAFETY: the caller's guarantee; a copy between our own resources of matching shape.
    unsafe { list.CopyTextureRegion(&dx::subresource(&lane.texture, 0), 0, 0, 0, &dx::placed(&lane.shared, footprint), None) };
    dx::barrier(list, &lane.texture, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COMMON);
}

/// A lane of `shape`: a shared buffer, the game's buffer on it, and the texture (made with
/// `flags`) it is read into.
fn lane(fns: &Fns, device: &Device, d3d: &ID3D12Device, shape: &Shape, flags: D3D12_RESOURCE_FLAGS) -> Result<Lane, String> {
    let (shared, buffer) = shared_buffer(fns, device, d3d, shape.bytes())?;
    let texture = dx::texture(d3d, shape.size.0, shape.size.1, shape.format, flags).map_err(|e| e.to_string())?;
    Ok(Lane { buffer, shared, texture })
}

/// This copy's D3D12 device and queue, made once: every ring this copy makes is on them (what the
/// frames go to, a pair publisher, was made on them).
static D3D12: Mutex<Option<(ID3D12Device, ID3D12CommandQueue)>> = Mutex::new(None);

fn d3d12(luid: Option<[u8; 8]>) -> Result<(ID3D12Device, ID3D12CommandQueue), String> {
    let mut made = D3D12.locked();
    if let Some(both) = made.as_ref() {
        return Ok(both.clone());
    }
    let device = d3d12_device(luid)?;
    // SAFETY: creation calls on our own device.
    let queue: ID3D12CommandQueue = unsafe { device.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC { Type: D3D12_COMMAND_LIST_TYPE_DIRECT, ..Default::default() }) }.map_err(|e| e.to_string())?;
    *made = Some((device.clone(), queue.clone()));
    Ok((device, queue))
}

/// A D3D12 device on the game's GPU (by its LUID; the default adapter without one).
fn d3d12_device(luid: Option<[u8; 8]>) -> Result<ID3D12Device, String> {
    let adapter = match luid {
        Some(bytes) => {
            let low = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
            let high = i32::from_le_bytes(bytes[4..8].try_into().unwrap());
            let adapter = monaka_channel::d3d::find_adapter(monaka_contract::protocol::Luid { low, high }).map_err(|e| format!("the game's GPU: {e}"))?;
            Some(adapter.cast::<windows::core::IUnknown>().map_err(|e| e.to_string())?)
        }
        None => None,
    };
    let mut device: Option<ID3D12Device> = None;
    // SAFETY: device creation on the found adapter.
    unsafe { D3D12CreateDevice(adapter.as_ref(), D3D_FEATURE_LEVEL_11_0, &mut device) }.map_err(|e| format!("D3D12 device: {e}"))?;
    device.ok_or_else(|| "no D3D12 device".to_owned())
}

/// A shared D3D12 fence, and the game's semaphore on it.
fn shared_fence(fns: &Fns, vk: VkDevice, d3d: &ID3D12Device) -> Result<(ID3D12Fence, VkSemaphore), String> {
    // SAFETY: creation calls on our own device, an import on the game's.
    unsafe {
        let fence: ID3D12Fence = d3d.CreateFence(0, D3D12_FENCE_FLAG_SHARED).map_err(|e| e.to_string())?;
        let handle = d3d.CreateSharedHandle(&fence, None, GENERIC_ALL.0, None).map_err(|e| e.to_string())?;
        let info = VkSemaphoreCreateInfo { sType: STRUCTURE_SEMAPHORE_CREATE_INFO, pNext: std::ptr::null(), flags: 0 };
        let mut semaphore = 0;
        let made = check((fns.create_semaphore)(vk, &info, std::ptr::null(), &mut semaphore), "vkCreateSemaphore");
        let imported = made.and_then(|()| {
            let import = VkImportSemaphoreWin32HandleInfoKHR {
                sType: STRUCTURE_IMPORT_SEMAPHORE_WIN32_HANDLE_INFO_KHR,
                pNext: std::ptr::null(),
                semaphore,
                flags: 0,
                handleType: EXTERNAL_SEMAPHORE_HANDLE_TYPE_D3D12_FENCE,
                handle: handle.0,
                name: std::ptr::null(),
            };
            check((fns.import_semaphore)(vk, &import), "vkImportSemaphoreWin32HandleKHR")
        });
        let _ = CloseHandle(handle);
        imported?;
        Ok((fence, semaphore))
    }
}

/// A shared D3D12 buffer of `bytes`, and the game's buffer on its memory.
fn shared_buffer(fns: &Fns, device: &Device, d3d: &ID3D12Device, bytes: u64) -> Result<(ID3D12Resource, VkBuffer), String> {
    let vk = device.device;
    // SAFETY: creation calls on our own device; creation, import and binding on the game's.
    unsafe {
        let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() };
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
        let mut shared: Option<ID3D12Resource> = None;
        d3d.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_SHARED, &desc, D3D12_RESOURCE_STATE_COMMON, None, &mut shared).map_err(|e| format!("shared buffer: {e}"))?;
        let shared = shared.ok_or("no shared buffer")?;
        let handle: HANDLE = d3d.CreateSharedHandle(&shared, None, GENERIC_ALL.0, None).map_err(|e| e.to_string())?;
        let result = import_buffer(fns, device, vk, handle, bytes);
        let _ = CloseHandle(handle);
        Ok((shared, result?))
    }
}

/// The game's buffer of `bytes` on the D3D12 resource behind `handle`.
///
/// # Safety
/// `handle` is a live shared handle of a D3D12 buffer of `bytes`.
unsafe fn import_buffer(fns: &Fns, device: &Device, vk: VkDevice, handle: HANDLE, bytes: u64) -> Result<VkBuffer, String> {
    let external = VkExternalMemoryBufferCreateInfo { sType: STRUCTURE_EXTERNAL_MEMORY_BUFFER_CREATE_INFO, pNext: std::ptr::null(), handleTypes: EXTERNAL_MEMORY_HANDLE_TYPE_D3D12_RESOURCE };
    let info = VkBufferCreateInfo {
        sType: STRUCTURE_BUFFER_CREATE_INFO,
        pNext: &external as *const _ as *const c_void,
        flags: 0,
        size: bytes,
        usage: BUFFER_USAGE_TRANSFER_DST,
        sharingMode: 0,
        queueFamilyIndexCount: 0,
        pQueueFamilyIndices: std::ptr::null(),
    };
    let mut buffer = 0;
    let mut requirements = VkMemoryRequirements::default();
    let mut properties = VkMemoryWin32HandlePropertiesKHR { sType: STRUCTURE_MEMORY_WIN32_HANDLE_PROPERTIES_KHR, pNext: std::ptr::null_mut(), memoryTypeBits: 0 };
    // SAFETY: creation and queries on the game's device with valid structures.
    unsafe {
        check((fns.create_buffer)(vk, &info, std::ptr::null(), &mut buffer), "vkCreateBuffer")?;
        (fns.buffer_requirements)(vk, buffer, &mut requirements);
        check((fns.handle_properties)(vk, EXTERNAL_MEMORY_HANDLE_TYPE_D3D12_RESOURCE, handle.0, &mut properties), "vkGetMemoryWin32HandlePropertiesKHR")?;
    }
    let bits = requirements.memoryTypeBits & properties.memoryTypeBits;
    let memory = device.memory;
    let kinds = (0..memory.memoryTypeCount as usize).filter(|&i| bits & (1 << i) != 0);
    let kind = kinds.clone().find(|&i| memory.memoryTypes[i].propertyFlags & MEMORY_PROPERTY_DEVICE_LOCAL != 0).or_else(|| kinds.clone().next()).ok_or("no memory type for the shared buffer")?;
    let dedicated = VkMemoryDedicatedAllocateInfo { sType: STRUCTURE_MEMORY_DEDICATED_ALLOCATE_INFO, pNext: std::ptr::null(), image: 0, buffer };
    let import = VkImportMemoryWin32HandleInfoKHR {
        sType: STRUCTURE_IMPORT_MEMORY_WIN32_HANDLE_INFO_KHR,
        pNext: &dedicated as *const _ as *const c_void,
        handleType: EXTERNAL_MEMORY_HANDLE_TYPE_D3D12_RESOURCE,
        handle: handle.0,
        name: std::ptr::null(),
    };
    let allocate = VkMemoryAllocateInfo { sType: STRUCTURE_MEMORY_ALLOCATE_INFO, pNext: &import as *const _ as *const c_void, allocationSize: requirements.size, memoryTypeIndex: kind as u32 };
    let mut memory = 0;
    // SAFETY: the import and binding on the game's device.
    unsafe {
        check((fns.allocate_memory)(vk, &allocate, std::ptr::null(), &mut memory), "vkAllocateMemory (import)")?;
        check((fns.bind_buffer)(vk, buffer, memory, 0), "vkBindBufferMemory")?;
    }
    Ok(buffer)
}

fn command_pool(fns: &Fns, vk: VkDevice, family: u32) -> Result<VkCommandPool, String> {
    let info = VkCommandPoolCreateInfo { sType: STRUCTURE_COMMAND_POOL_CREATE_INFO, pNext: std::ptr::null(), flags: COMMAND_POOL_CREATE_RESET_COMMAND_BUFFER, queueFamilyIndex: family };
    let mut pool = 0;
    // SAFETY: creation on the game's device.
    check(unsafe { (fns.create_pool)(vk, &info, std::ptr::null(), &mut pool) }, "vkCreateCommandPool")?;
    Ok(pool)
}

/// A command buffer of `pool`, given the device's dispatch (as the loader gives the ones made
/// through it: layers find their state by it), and a fence made signalled.
fn command_buffer(fns: &Fns, vk: VkDevice, pool: VkCommandPool) -> Result<(VkCommandBuffer, VkFence), String> {
    let info = VkCommandBufferAllocateInfo { sType: STRUCTURE_COMMAND_BUFFER_ALLOCATE_INFO, pNext: std::ptr::null(), commandPool: pool, level: 0, commandBufferCount: 1 };
    let mut commands = 0;
    let fence_info = VkFenceCreateInfo { sType: STRUCTURE_FENCE_CREATE_INFO, pNext: std::ptr::null(), flags: FENCE_CREATE_SIGNALED };
    let mut fence = 0;
    // SAFETY: creation on the game's device; the dispatch pointer is the first word of both.
    unsafe {
        check((fns.allocate_commands)(vk, &info, &mut commands), "vkAllocateCommandBuffers")?;
        *(commands as *mut usize) = *(vk as *const usize);
        check((fns.create_fence)(vk, &fence_info, std::ptr::null(), &mut fence), "vkCreateFence")?;
    }
    Ok((commands, fence))
}
