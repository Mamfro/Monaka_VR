//! Rendered frames redrawn from another camera on D3D12, as [`crate::Reprojector`] does on D3D11
//! with the same shader and constants (`reproject.hlsl`, [`crate::reproject`]): for alternate-eye
//! rendering with depth, the other eye of this moment is this frame reprojected (the primary), and
//! what this frame could not see is filled from that eye's own last real frame reprojected to now
//! (the fill). The colour is read at its own size and the depth at the render size (UV scaled), so
//! a frame upscaled after its depth was made reprojects at the display size; a depth texture larger
//! than the frame's render area is read in the part that lines up with the image
//! ([`Source::depth_rect`]), moved by the jitter the frame was rendered with. Besides the image, the
//! target's depth is written at a size of the caller's choice (the render size, beside the real
//! eye's), in the same part of its texture, in the target projection's own depth convention.
//!
//! Recorded on a command list of the caller's; the caller owns the outputs, and every texture it
//! passes is in COMMON and left there.

use crate::reproject::{Constants, DepthRect, depth_area};
use crate::{Error, Result, View};
use monaka_channel::d3d12::compute::{self, DescriptorHeap, Table, srv_desc, uav_desc};
use monaka_channel::d3d12::{self, barrier, uav_barrier};
use monaka_core::camera::{Mat34, Mat44};
use std::ffi::c_void;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::{Interface, s};

#[cfg(test)]
mod tests;

/// The shader on D3D12: its images written as float (the views' format gives the storage).
pub const REPROJECT12_SOURCE: &str = concat!("#define IMAGE float4\n", include_str!("reproject.hlsl"));

const CONSTANTS: u32 = (size_of::<Constants>() / 4) as u32;

/// One frame to redraw: its colour (any size) and depth (plane 0 as R32: `R32_TYPELESS` or
/// `R32_FLOAT`, at the render size, or larger with the render area at `depth_rect`), both in
/// COMMON, the sub-pixel jitter the depth was rendered with (in its pixels), and the canonical
/// camera and projection it was rendered with.
#[derive(Clone, Copy)]
pub struct Source<'a> {
    pub color: &'a ID3D12Resource,
    pub depth: &'a ID3D12Resource,
    pub depth_rect: DepthRect,
    pub jitter: [f32; 2],
    pub projection: Mat44,
    pub camera: Mat34,
}

/// Where a redraw goes: `color` (the view's size, a format with unordered access, see
/// [`uav_format`]) and, optionally, `depth` (R32, any size, with unordered access; written in the
/// part `depth_rect` names, the primary's by default). Both in COMMON.
#[derive(Clone, Copy)]
pub struct Targets<'a> {
    pub color: &'a ID3D12Resource,
    pub depth: Option<&'a ID3D12Resource>,
    pub depth_rect: DepthRect,
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
    /// The fill first where its depth agrees with the primary's (`reproject.hlsl`).
    pub fill_first: bool,
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
            fill_first: false,
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
        let out_area = out_depth.map(|depth| depth_area(out.depth_rect.or(primary.depth_rect), (depth.Width as u32, depth.Height)));
        let constants = Constants::redraw(view, out_area.map(|(_, size)| size), out_area.map_or([0, 0], |(origin, _)| origin), self.fill_first);
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
        // The resolves: both layers read, the outputs written, the primary's depth read (asking its
        // eye about the fill).
        let set = base + VIEWS * 2;
        let resolve = constants.resolve(splats[0].as_ref());
        // SAFETY: views of live textures (or null views) into our own heap.
        unsafe {
            self.null_sources(set, format);
            if splats[0].is_some() {
                let depth_format = depth_read_format(primary.depth.GetDesc().Format).ok_or(Error::Unsupported("depth format"))?;
                self.device.CreateShaderResourceView(primary.depth, Some(&srv_desc(depth_format)), self.heap.cpu(set + 1));
            }
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
            set_constants(&resolve);
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
        let area = depth_area(source.depth_rect, (depth.Width as u32, depth.Height));
        base.splat(view, (color.Width as u32, color.Height), area, source.jitter, &source.projection, &source.camera)
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
