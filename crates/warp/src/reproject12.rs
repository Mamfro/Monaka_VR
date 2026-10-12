//! Rendered frames redrawn from another camera on D3D12 (`reproject12.hlsl`), as [`crate::Reprojector`]
//! does on D3D11: for alternate-eye rendering with depth, the other eye of this moment is this
//! frame reprojected (the primary), and what this frame could not see is filled from that eye's own
//! last real frame reprojected to now (the fill). The colour is read at its own size and the depth
//! at the render size (UV scaled), so a frame upscaled after its depth was made reprojects at the
//! display size. Besides the image, the target's depth is written at a size of the caller's choice
//! (the render size, beside the real eye's), in the target projection's own depth convention.
//!
//! Recorded on a command list of the caller's; the caller owns the outputs, and every texture it
//! passes is in COMMON and left there.

use crate::{Error, Result, View};
use monaka_channel::d3d12::compute::{self, DescriptorHeap, Table, srv_desc, uav_desc};
use monaka_channel::d3d12::{self, barrier, uav_barrier};
use monaka_core::camera::{Mat34, Mat44};
use monaka_core::depth::reprojection;
use std::ffi::c_void;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::{Interface, s};

pub const REPROJECT12_SOURCE: &str = include_str!("reproject12.hlsl");

/// The shader's root constants (`cbuffer Reproject`).
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Constants {
    m: [f32; 16],
    width: u32,
    height: u32,
    source_width: u32,
    source_height: u32,
    depth_width: u32,
    depth_height: u32,
    out_depth_width: u32,
    out_depth_height: u32,
    near_a: f32,
    near_b: f32,
    pad: [u32; 2],
}

const CONSTANTS: u32 = (size_of::<Constants>() / 4) as u32;
const _: () = assert!(size_of::<Constants>() == 28 * 4);

/// One frame to redraw: its colour (any size) and depth (plane 0 as R32: `R32_TYPELESS` or
/// `R32_FLOAT`, at the render size), both in COMMON, with the canonical camera and projection it
/// was rendered with.
#[derive(Clone, Copy)]
pub struct Source<'a> {
    pub color: &'a ID3D12Resource,
    pub depth: &'a ID3D12Resource,
    pub projection: Mat44,
    pub camera: Mat34,
}

/// Where a redraw goes: `color` (the view's size, a format with unordered access, see
/// [`uav_format`]) and, optionally, `depth` (R32, any size, with unordered access). Both in COMMON.
#[derive(Clone, Copy)]
pub struct Targets<'a> {
    pub color: &'a ID3D12Resource,
    pub depth: Option<&'a ID3D12Resource>,
}

/// One layer's depth-test keys and painted image.
struct Layer {
    keys: ID3D12Resource,
    image: ID3D12Resource,
}

/// Views per pass: six shader resources (t0-t5), then four unordered access views (u0-u3).
const VIEWS: u32 = 10;
/// Passes per redraw with views of their own: the primary's splat, the fill's, the resolves.
const PASSES: u32 = 3;
/// Redraws whose views the heap holds at once (the lists in flight read them).
const SETS: u32 = 8;

pub struct Reprojector12 {
    device: ID3D12Device,
    root: ID3D12RootSignature,
    clear: ID3D12PipelineState,
    splat: ID3D12PipelineState,
    paint: ID3D12PipelineState,
    resolve: ID3D12PipelineState,
    resolve_depth: ID3D12PipelineState,
    heap: DescriptorHeap,
    next: u32,
    layers: Option<[Layer; 2]>,
    size: (u32, u32),
    format: DXGI_FORMAT,
}

// SAFETY: D3D12 objects are free-threaded; the reprojector is used under its owner's lock.
unsafe impl Send for Reprojector12 {}

/// The format a colour texture of `format` is written in by a shader (unordered access): the
/// UNORM or float member of its family, which copies to and from it unchanged. `None` for a
/// family the passes do not write.
pub fn uav_format(format: DXGI_FORMAT) -> Option<DXGI_FORMAT> {
    match format {
        DXGI_FORMAT_R8G8B8A8_TYPELESS | DXGI_FORMAT_R8G8B8A8_UNORM | DXGI_FORMAT_R8G8B8A8_UNORM_SRGB => Some(DXGI_FORMAT_R8G8B8A8_UNORM),
        DXGI_FORMAT_B8G8R8A8_TYPELESS | DXGI_FORMAT_B8G8R8A8_UNORM | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB => Some(DXGI_FORMAT_B8G8R8A8_UNORM),
        DXGI_FORMAT_R10G10B10A2_TYPELESS | DXGI_FORMAT_R10G10B10A2_UNORM => Some(DXGI_FORMAT_R10G10B10A2_UNORM),
        DXGI_FORMAT_R16G16B16A16_TYPELESS | DXGI_FORMAT_R16G16B16A16_FLOAT => Some(DXGI_FORMAT_R16G16B16A16_FLOAT),
        _ => None,
    }
}

/// Whether the device's shaders can write `format` (typed unordered access stores).
pub fn writable(device: &ID3D12Device, format: DXGI_FORMAT) -> bool {
    let mut support = D3D12_FEATURE_DATA_FORMAT_SUPPORT { Format: format, ..Default::default() };
    // SAFETY: a feature query into a local structure of the size given.
    let asked = unsafe { device.CheckFeatureSupport(D3D12_FEATURE_FORMAT_SUPPORT, &mut support as *mut _ as *mut c_void, size_of::<D3D12_FEATURE_DATA_FORMAT_SUPPORT>() as u32) };
    asked.is_ok() && (support.Support2.0 & D3D12_FORMAT_SUPPORT2_UAV_TYPED_STORE.0) != 0
}

impl Reprojector12 {
    pub fn new(device: &ID3D12Device) -> Result<Self> {
        let root = compute::root_signature(device, CONSTANTS, &[Table { srvs: 6, uavs: 4 }]).map_err(Error::Shader)?;
        let pipeline = |entry| compute::shader_pipeline(device, &root, REPROJECT12_SOURCE, "reproject12.hlsl", entry).map_err(Error::Shader);
        Ok(Self {
            clear: pipeline(s!("Clear"))?,
            splat: pipeline(s!("Splat"))?,
            paint: pipeline(s!("Paint"))?,
            resolve: pipeline(s!("Resolve"))?,
            resolve_depth: pipeline(s!("ResolveDepth"))?,
            heap: DescriptorHeap::new(device, VIEWS * PASSES * SETS).map_err(Error::Shader)?,
            device: device.clone(),
            root,
            next: 0,
            layers: None,
            size: (0, 0),
            format: DXGI_FORMAT(0),
        })
    }

    /// The layers for `size` in `format`. Made again when either changes: the caller makes sure
    /// the GPU is done with the lists that used the old ones.
    fn layers(&mut self, size: (u32, u32), format: DXGI_FORMAT) -> Result<()> {
        if self.layers.is_some() && self.size == size && self.format == format {
            return Ok(());
        }
        self.layers = None;
        let uav = D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS;
        let layer = || -> Result<Layer> {
            Ok(Layer { keys: d3d12::texture(&self.device, size.0, size.1, DXGI_FORMAT_R32_UINT, uav)?, image: d3d12::texture(&self.device, size.0, size.1, format, uav)? })
        };
        self.layers = Some([layer()?, layer()?]);
        (self.size, self.format) = (size, format);
        Ok(())
    }

    /// Records on `list`: `view` drawn from `primary` into `out`, its gaps filled from `fill`,
    /// then from a neighbour. Every texture is in COMMON before and after.
    pub fn record(&mut self, list: &ID3D12GraphicsCommandList, view: &View, primary: &Source, fill: Option<&Source>, out: &Targets) -> Result<()> {
        // SAFETY: reads resource descriptors.
        let (out_desc, out_depth) = unsafe { (out.color.GetDesc(), out.depth.map(|d| d.GetDesc())) };
        let format = uav_format(out_desc.Format).filter(|&f| f == out_desc.Format).ok_or(Error::Unsupported("output format"))?;
        if (out_desc.Width as u32, out_desc.Height) != (view.width, view.height) {
            return Err(Error::Unsupported("output size"));
        }
        if !(view.mapping.a.is_finite() && view.mapping.a != 0.0 && view.mapping.b.is_finite()) {
            return Err(Error::Unsupported("depth convention"));
        }
        self.layers((view.width, view.height), format)?;
        let base = (self.next % SETS) * VIEWS * PASSES;
        self.next = self.next.wrapping_add(1);
        let layers = self.layers.as_ref().expect("made above");
        let mut constants = Constants { width: view.width, height: view.height, near_a: view.mapping.a, near_b: view.mapping.b, ..Default::default() };
        if let Some(depth) = &out_depth {
            (constants.out_depth_width, constants.out_depth_height) = (depth.Width as u32, depth.Height);
        }
        // The two splats, each with its source's views and its layer's.
        let sources = [Some(primary), fill];
        let mut splats: [Option<Constants>; 2] = [None, None];
        for (k, (source, layer)) in sources.iter().zip(layers).enumerate() {
            let set = base + VIEWS * k as u32;
            let source = (*source).and_then(|s| Some((s, self.source_constants(view, s, &constants)?)));
            // SAFETY: views of live textures (or null views) into our own heap.
            unsafe {
                match source {
                    Some((s, _)) => {
                        let (color, depth) = (s.color.GetDesc(), s.depth.GetDesc());
                        self.device.CreateShaderResourceView(s.color, Some(&srv_desc(read_format(color.Format))), self.heap.cpu(set));
                        let depth_format = depth_read_format(depth.Format).ok_or(Error::Unsupported("depth format"))?;
                        self.device.CreateShaderResourceView(s.depth, Some(&srv_desc(depth_format)), self.heap.cpu(set + 1));
                    }
                    None => self.null_sources(set, format),
                }
                self.null_resolve_inputs(set, format);
                self.device.CreateUnorderedAccessView(&layer.keys, None, Some(&uav_desc(DXGI_FORMAT_R32_UINT)), self.heap.cpu(set + 6));
                self.device.CreateUnorderedAccessView(&layer.image, None, Some(&uav_desc(format)), self.heap.cpu(set + 7));
                self.null_outputs(set, format);
            }
            splats[k] = source.map(|(_, c)| c);
        }
        // The resolves: both layers read, the outputs written.
        let set = base + VIEWS * 2;
        // SAFETY: views of live textures (or null views) into our own heap.
        unsafe {
            self.null_sources(set, format);
            self.device.CreateShaderResourceView(&layers[0].image, Some(&srv_desc(format)), self.heap.cpu(set + 2));
            self.device.CreateShaderResourceView(&layers[1].image, Some(&srv_desc(format)), self.heap.cpu(set + 3));
            self.device.CreateShaderResourceView(&layers[0].keys, Some(&srv_desc(DXGI_FORMAT_R32_UINT)), self.heap.cpu(set + 4));
            self.device.CreateShaderResourceView(&layers[1].keys, Some(&srv_desc(DXGI_FORMAT_R32_UINT)), self.heap.cpu(set + 5));
            self.device.CreateUnorderedAccessView(None, None, Some(&uav_desc(DXGI_FORMAT_R32_UINT)), self.heap.cpu(set + 6));
            self.device.CreateUnorderedAccessView(None, None, Some(&uav_desc(format)), self.heap.cpu(set + 7));
            self.device.CreateUnorderedAccessView(out.color, None, Some(&uav_desc(format)), self.heap.cpu(set + 8));
            match out.depth {
                Some(depth) => self.device.CreateUnorderedAccessView(depth, None, Some(&uav_desc(DXGI_FORMAT_R32_FLOAT)), self.heap.cpu(set + 9)),
                None => self.device.CreateUnorderedAccessView(None, None, Some(&uav_desc(DXGI_FORMAT_R32_FLOAT)), self.heap.cpu(set + 9)),
            }
        }
        const SRV: D3D12_RESOURCE_STATES = D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE;
        const UAV: D3D12_RESOURCE_STATES = D3D12_RESOURCE_STATE_UNORDERED_ACCESS;
        const COMMON: D3D12_RESOURCE_STATES = D3D12_RESOURCE_STATE_COMMON;
        // Each texture read once (a fill may share the primary's depth).
        let mut read: Vec<&ID3D12Resource> = Vec::new();
        for texture in sources.iter().flatten().flat_map(|s| [s.color, s.depth]) {
            if !read.iter().any(|r| r.as_raw() == texture.as_raw()) {
                read.push(texture);
            }
        }
        let groups = |w: u32, h: u32| (w.div_ceil(8), h.div_ceil(8));
        let set_constants = |c: &Constants| {
            // SAFETY: root constants of the bound root signature, from a whole `Constants`.
            unsafe { list.SetComputeRoot32BitConstants(0, CONSTANTS, c as *const Constants as *const c_void, 0) };
        };
        // SAFETY: records compute work on the caller's open list with our own heap, root signature
        // and pipelines; every resource is live until the list has executed (the caller's fence).
        unsafe {
            read.iter().for_each(|r| barrier(list, r, COMMON, SRV));
            list.SetDescriptorHeaps(&[Some(self.heap.heap().clone())]);
            list.SetComputeRootSignature(&self.root);
            for (k, layer) in layers.iter().enumerate() {
                barrier(list, &layer.keys, COMMON, UAV);
                barrier(list, &layer.image, COMMON, UAV);
                list.SetComputeRootDescriptorTable(1, self.heap.gpu(base + VIEWS * k as u32));
                set_constants(&constants);
                let (gx, gy) = groups(view.width, view.height);
                list.SetPipelineState(&self.clear);
                list.Dispatch(gx, gy, 1);
                uav_barrier(list, &layer.keys);
                uav_barrier(list, &layer.image);
                if let Some(c) = &splats[k] {
                    set_constants(c);
                    let (sx, sy) = groups(c.source_width, c.source_height);
                    list.SetPipelineState(&self.splat);
                    list.Dispatch(sx, sy, 1);
                    uav_barrier(list, &layer.keys);
                    list.SetPipelineState(&self.paint);
                    list.Dispatch(sx, sy, 1);
                }
                barrier(list, &layer.keys, UAV, SRV);
                barrier(list, &layer.image, UAV, SRV);
            }
            list.SetComputeRootDescriptorTable(1, self.heap.gpu(base + VIEWS * 2));
            set_constants(&constants);
            barrier(list, out.color, COMMON, UAV);
            list.SetPipelineState(&self.resolve);
            let (gx, gy) = groups(view.width, view.height);
            list.Dispatch(gx, gy, 1);
            barrier(list, out.color, UAV, COMMON);
            if let Some(depth) = out.depth {
                barrier(list, depth, COMMON, UAV);
                list.SetPipelineState(&self.resolve_depth);
                let (dx, dy) = groups(constants.out_depth_width, constants.out_depth_height);
                list.Dispatch(dx, dy, 1);
                barrier(list, depth, UAV, COMMON);
            }
            for layer in layers {
                barrier(list, &layer.keys, SRV, COMMON);
                barrier(list, &layer.image, SRV, COMMON);
            }
            read.iter().for_each(|r| barrier(list, r, SRV, COMMON));
        }
        Ok(())
    }

    /// The constants of a splat of `source` into `view`; `None` when its cameras do not make a
    /// reprojection (left out, as a missing fill).
    fn source_constants(&self, view: &View, source: &Source, base: &Constants) -> Option<Constants> {
        // SAFETY: reads resource descriptors.
        let (color, depth) = unsafe { (source.color.GetDesc(), source.depth.GetDesc()) };
        // Both cameras moved by the target's position: the matrix is built from small numbers.
        let to_origin = |camera: &Mat34| {
            let mut c = *camera;
            (c[3], c[7], c[11]) = (c[3] - view.camera[3], c[7] - view.camera[7], c[11] - view.camera[11]);
            c
        };
        let m = reprojection(&source.projection, &to_origin(&source.camera), &view.projection, &to_origin(&view.camera))?;
        Some(Constants { m, source_width: color.Width as u32, source_height: color.Height, depth_width: depth.Width as u32, depth_height: depth.Height, ..*base })
    }

    /// Null views for the source colour and depth of the set at `set`.
    ///
    /// # Safety
    /// `set` is a set of our own heap.
    unsafe fn null_sources(&self, set: u32, format: DXGI_FORMAT) {
        // SAFETY: null views into our own heap.
        unsafe {
            self.device.CreateShaderResourceView(None, Some(&srv_desc(format)), self.heap.cpu(set));
            self.device.CreateShaderResourceView(None, Some(&srv_desc(DXGI_FORMAT_R32_FLOAT)), self.heap.cpu(set + 1));
        }
    }

    /// Null views for the layers the resolves read, in the set at `set`.
    ///
    /// # Safety
    /// `set` is a set of our own heap.
    unsafe fn null_resolve_inputs(&self, set: u32, format: DXGI_FORMAT) {
        // SAFETY: null views into our own heap.
        unsafe {
            for k in 2..4 {
                self.device.CreateShaderResourceView(None, Some(&srv_desc(format)), self.heap.cpu(set + k));
            }
            for k in 4..6 {
                self.device.CreateShaderResourceView(None, Some(&srv_desc(DXGI_FORMAT_R32_UINT)), self.heap.cpu(set + k));
            }
        }
    }

    /// Null views for the outputs, in the set at `set`.
    ///
    /// # Safety
    /// `set` is a set of our own heap.
    unsafe fn null_outputs(&self, set: u32, format: DXGI_FORMAT) {
        // SAFETY: null views into our own heap.
        unsafe {
            self.device.CreateUnorderedAccessView(None, None, Some(&uav_desc(format)), self.heap.cpu(set + 8));
            self.device.CreateUnorderedAccessView(None, None, Some(&uav_desc(DXGI_FORMAT_R32_FLOAT)), self.heap.cpu(set + 9));
        }
    }
}

/// The format a colour texture of `format` is read as: its own, or the UNORM or float member of a
/// typeless family.
fn read_format(format: DXGI_FORMAT) -> DXGI_FORMAT {
    match format {
        DXGI_FORMAT_R8G8B8A8_TYPELESS | DXGI_FORMAT_B8G8R8A8_TYPELESS | DXGI_FORMAT_R10G10B10A2_TYPELESS | DXGI_FORMAT_R16G16B16A16_TYPELESS => {
            uav_format(format).expect("a family the passes write")
        }
        other => other,
    }
}

/// The format a depth copy (plane 0, 32-bit float) is read as.
fn depth_read_format(format: DXGI_FORMAT) -> Option<DXGI_FORMAT> {
    match format {
        DXGI_FORMAT_R32_TYPELESS | DXGI_FORMAT_R32_FLOAT => Some(DXGI_FORMAT_R32_FLOAT),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use monaka_channel::d3d12::{Recorder, placed, read_texture, subresource};
    use monaka_core::depth::DepthMapping;
    use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;

    #[test]
    fn shaders_compile() {
        for entry in [s!("Clear"), s!("Splat"), s!("Paint"), s!("Resolve"), s!("ResolveDepth")] {
            compute::compile(REPROJECT12_SOURCE, "reproject12.hlsl", entry).unwrap();
        }
        assert_eq!(uav_format(DXGI_FORMAT_R8G8B8A8_UNORM_SRGB), Some(DXGI_FORMAT_R8G8B8A8_UNORM));
        assert_eq!(uav_format(DXGI_FORMAT_R11G11B10_FLOAT), None);
    }

    fn device() -> ID3D12Device {
        let mut device: Option<ID3D12Device> = None;
        // SAFETY: creates a device on the default adapter.
        unsafe { D3D12CreateDevice(None::<&windows::core::IUnknown>, D3D_FEATURE_LEVEL_11_0, &mut device) }.unwrap();
        device.unwrap()
    }

    fn queue(device: &ID3D12Device) -> ID3D12CommandQueue {
        // SAFETY: creates a direct queue on a live device.
        unsafe { device.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC { Type: D3D12_COMMAND_LIST_TYPE_DIRECT, ..Default::default() }) }.unwrap()
    }

    /// A `w` x `h` texture of `format` (four bytes a texel) holding `bytes`, in COMMON.
    fn upload(device: &ID3D12Device, queue: &ID3D12CommandQueue, (w, h): (u32, u32), format: DXGI_FORMAT, bytes: &[u8]) -> ID3D12Resource {
        let texture = d3d12::texture(device, w, h, format, D3D12_RESOURCE_FLAG_NONE).unwrap();
        // SAFETY: a layout query, an upload buffer of ours written through its mapping within its
        // size, and a copy recorded and waited for.
        unsafe {
            let desc = texture.GetDesc();
            let (mut footprint, mut total) = (D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default(), 0u64);
            device.GetCopyableFootprints(&desc, 0, 1, 0, Some(&mut footprint), None, None, Some(&mut total));
            let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_UPLOAD, ..Default::default() };
            let buffer_desc = D3D12_RESOURCE_DESC {
                Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
                Width: total,
                Height: 1,
                DepthOrArraySize: 1,
                MipLevels: 1,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
                ..Default::default()
            };
            let mut buffer: Option<ID3D12Resource> = None;
            device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &buffer_desc, D3D12_RESOURCE_STATE_GENERIC_READ, None, &mut buffer).unwrap();
            let buffer = buffer.unwrap();
            let mut mapped = std::ptr::null_mut();
            buffer.Map(0, None, Some(&mut mapped)).unwrap();
            let row = w as usize * 4;
            for y in 0..h as usize {
                std::ptr::copy_nonoverlapping(bytes[y * row..].as_ptr(), mapped.cast::<u8>().add(footprint.Offset as usize + y * footprint.Footprint.RowPitch as usize), row);
            }
            buffer.Unmap(0, None);
            let mut recorder = Recorder::new(device, 1).unwrap();
            let list = recorder.begin().unwrap().unwrap();
            barrier(&list, &texture, D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_COPY_DEST);
            list.CopyTextureRegion(&subresource(&texture, 0), 0, 0, 0, &placed(&buffer, footprint), None);
            barrier(&list, &texture, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COMMON);
            recorder.submit(queue).unwrap();
            assert!(recorder.idle(2000));
        }
        texture
    }

    fn pixel(queue: &ID3D12CommandQueue, texture: &ID3D12Resource, x: usize, y: usize) -> [u8; 4] {
        let (_, _, pitch, bytes) = read_texture(queue, texture, D3D12_RESOURCE_STATE_COMMON).unwrap();
        bytes[y * pitch + x * 4..][..4].try_into().unwrap()
    }

    fn depth_at(queue: &ID3D12CommandQueue, texture: &ID3D12Resource, x: usize, y: usize) -> f32 {
        let (_, _, pitch, bytes) = read_texture(queue, texture, D3D12_RESOURCE_STATE_COMMON).unwrap();
        f32::from_le_bytes(bytes[y * pitch + x * 4..][..4].try_into().unwrap())
    }

    #[test]
    fn other_eye_from_this_frame_and_gaps_from_its_own_last_frame() {
        let device = device();
        let queue = queue(&device);
        let mut reprojector = Reprojector12::new(&device).unwrap();
        // Colour 64x8, depth at half that (the render size): reverse-Z infinite with P00 = P11 = 1
        // and near 0.1, a wall 2 m away everywhere (depth = near / distance).
        let (w, h, near) = (64u32, 8u32, 0.1f32);
        let mut projection = [0.0f32; 16];
        (projection[0], projection[5], projection[11], projection[14]) = (1.0, 1.0, near, -1.0);
        let wall = near / 2.0;
        let depth: Vec<u8> = (0..(w / 2) * (h / 2)).flat_map(|_| wall.to_le_bytes()).collect();
        let depth = upload(&device, &queue, (w / 2, h / 2), DXGI_FORMAT_R32_TYPELESS, &depth);
        let camera = |x: f32| -> Mat34 { [1.0, 0.0, 0.0, x, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0] };
        // This frame: the left camera, red = 4 * column. The right eye's last frame: plain green.
        let columns: Vec<u8> = (0..h).flat_map(|_| (0..w).flat_map(|x| [(x * 4) as u8, 0, 0, 255])).collect();
        let green: Vec<u8> = (0..w * h).flat_map(|_| [0, 255, 0, 255]).collect();
        let left = upload(&device, &queue, (w, h), DXGI_FORMAT_R8G8B8A8_UNORM, &columns);
        let last_right = upload(&device, &queue, (w, h), DXGI_FORMAT_R8G8B8A8_UNORM, &green);
        let uav = D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS;
        let out = d3d12::texture(&device, w, h, DXGI_FORMAT_R8G8B8A8_UNORM, uav).unwrap();
        let out_depth = d3d12::texture(&device, w / 2, h / 2, DXGI_FORMAT_R32_TYPELESS, uav).unwrap();
        let mapping = DepthMapping::from_projection(&projection, near).unwrap();
        // The camera positions far from the origin, as a game world's are.
        let view = View { projection, camera: camera(300.5), mapping, width: w, height: h };
        let primary = Source { color: &left, depth: &depth, projection, camera: camera(299.5) };
        let fill = Source { color: &last_right, depth: &depth, projection, camera: camera(300.5) };
        let targets = Targets { color: &out, depth: Some(&out_depth) };
        let run = |reprojector: &mut Reprojector12, fill: Option<&Source>| {
            let mut recorder = Recorder::new(&device, 1).unwrap();
            let list = recorder.begin().unwrap().unwrap();
            reprojector.record(&list, &view, &primary, fill, &targets).unwrap();
            recorder.submit(&queue).unwrap();
            assert!(recorder.idle(2000));
        };

        // Cameras 1 m apart, wall 2 m away: the right eye sees each column 16 pixels further left.
        run(&mut reprojector, Some(&fill));
        assert_eq!(pixel(&queue, &out, 20, 3), [36 * 4, 0, 0, 255]);
        // The right edge was out of the left camera's view: its own last frame fills it.
        assert_eq!(pixel(&queue, &out, 55, 3), [0, 255, 0, 255]);
        // The right eye's depth: the wall, at the render size.
        assert!((depth_at(&queue, &out_depth, 10, 2) - wall).abs() < 1e-6);
        assert!((depth_at(&queue, &out_depth, 28, 2) - wall).abs() < 1e-6, "filled from the last frame's depth");
        // Without a fill frame, the gap continues its neighbour instead.
        run(&mut reprojector, None);
        assert_eq!(pixel(&queue, &out, 55, 3), [63 * 4, 0, 0, 255]);
    }
}
