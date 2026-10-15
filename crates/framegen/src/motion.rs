//! Motion vectors for alternate-eye stereo, on the GPU: a game's previous frame is the other eye's,
//! so its motion vectors point at the wrong image for anything that keeps a history per eye (an
//! upscaler, frame generation). Either the vectors are rebased onto this eye's previous frame by the
//! camera (`motion_fix.hlsl`: the game's depth and motion vectors in), or the game's own vectors are
//! chained over two frames (`motion_compose.hlsl`: the previous frame's vectors and this frame's
//! in), or, for an eye warped from the other eye's frame, the real eye's vectors are carried into
//! the warped eye's view (`motion_warp.hlsl`: the warped depth, the real eye's depth and its
//! vectors in), into a texture of our own given instead. Recorded on the game's own command list just
//! before the consumer runs, reading the game's textures in a shader-readable state and leaving
//! them so. The vector conventions are The Beast's (DLSS, frame generation); Kingdom Come:
//! Deliverance II (FSR) uses the chaining.

use monaka_channel::d3d12::barrier;
use monaka_channel::d3d12::compute::{self, DescriptorHeap, Table, srv_desc, uav_desc};
use monaka_streamline::per_eye::{MotionRebase, Rebase};
use std::ffi::c_void;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::s;

const SOURCE: &str = include_str!("motion_fix.hlsl");
/// The chaining of the game's vectors over two frames ([`MotionFix::composer`]).
const COMPOSE_SOURCE: &str = include_str!("motion_compose.hlsl");
/// The carrying of the real eye's vectors into the warped eye's view ([`MotionFix::warper`]).
const WARP_SOURCE: &str = include_str!("motion_warp.hlsl");
/// Descriptor sets in the ring (the inputs, then the output), so a set still read by a frame in
/// flight on the GPU is not rewritten.
const SETS: u32 = 8;
/// The most inputs a pass reads.
const MAX_INPUTS: u32 = 3;
const THREADS: u32 = 8;

/// The shaders' constants (56 root constants), the matrices row-major with row vectors as
/// Streamline's: from this frame's clip space to the game's previous frame's (`game_to_prev`) and
/// to the consumer's previous image of this eye (`same_to_prev`), and, for a warped eye, from its
/// clip space to the real eye's (`target_to_source`); the output's size (0: the last input's), the
/// game's motion-vector scale, and how far object motion is carried (`object_scale`: the consumer's
/// frame interval over the game's).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Constants {
    pub game_to_prev: [f32; 16],
    pub same_to_prev: [f32; 16],
    pub target_to_source: [f32; 16],
    pub size: [u32; 2],
    pub mvec_scale: [f32; 2],
    pub object_scale: f32,
    pub pad: [f32; 3],
}

impl Constants {
    /// For the chaining shader ([`MotionFix::composer`]): `scale` turns the game's stored vectors
    /// into render pixels (1 for vectors already in pixels, The Beast's; the render size for vectors
    /// in screen units, KCD2's FSR ones). The output keeps the game's units.
    pub fn chaining(scale: [f32; 2]) -> Self {
        Self { mvec_scale: scale, ..Self::default() }
    }
}

/// The state the output is tagged in for DLSS (readable by any shader), as the game's own were.
pub const READABLE: D3D12_RESOURCE_STATES = D3D12_RESOURCE_STATES(D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE.0 | D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE.0);

pub struct MotionFix {
    device: ID3D12Device,
    root: ID3D12RootSignature,
    pipeline: ID3D12PipelineState,
    heap: DescriptorHeap,
    /// How many textures the pass reads, in shader order.
    inputs: u32,
    output: Option<(ID3D12Resource, u32, u32)>,
    next: u32,
}

/// The view of plane 0 of a tagged texture as the shader reads it.
fn read_format(format: DXGI_FORMAT) -> Option<DXGI_FORMAT> {
    match format {
        DXGI_FORMAT_R32G8X24_TYPELESS | DXGI_FORMAT_D32_FLOAT_S8X24_UINT | DXGI_FORMAT_R32_FLOAT_X8X24_TYPELESS => Some(DXGI_FORMAT_R32_FLOAT_X8X24_TYPELESS),
        DXGI_FORMAT_R32_TYPELESS | DXGI_FORMAT_D32_FLOAT | DXGI_FORMAT_R32_FLOAT => Some(DXGI_FORMAT_R32_FLOAT),
        DXGI_FORMAT_R16G16B16A16_TYPELESS | DXGI_FORMAT_R16G16B16A16_FLOAT => Some(DXGI_FORMAT_R16G16B16A16_FLOAT),
        DXGI_FORMAT_R16G16_TYPELESS | DXGI_FORMAT_R16G16_FLOAT => Some(DXGI_FORMAT_R16G16_FLOAT),
        DXGI_FORMAT_R32G32_TYPELESS | DXGI_FORMAT_R32G32_FLOAT => Some(DXGI_FORMAT_R32G32_FLOAT),
        _ => None,
    }
}

impl MotionFix {
    /// The rebase onto this eye's previous frame by the camera (`motion_fix.hlsl`; its inputs the
    /// depth, then the motion vectors).
    pub fn new(device: &ID3D12Device) -> Result<Self, String> {
        Self::with_shader(device, SOURCE, "motion_fix.hlsl", s!("Fix"), 2)
    }

    /// The chaining of the game's vectors over two frames (`motion_compose.hlsl`; its inputs the
    /// previous frame's motion vectors, in [`READABLE`] while recorded, then this frame's).
    pub fn composer(device: &ID3D12Device) -> Result<Self, String> {
        Self::with_shader(device, COMPOSE_SOURCE, "motion_compose.hlsl", s!("Compose"), 2)
    }

    /// The carrying of the real eye's vectors into an eye warped from its frame (`motion_warp.hlsl`;
    /// its inputs the warped eye's depth, the real eye's depth, then the real eye's motion vectors).
    pub fn warper(device: &ID3D12Device) -> Result<Self, String> {
        Self::with_shader(device, WARP_SOURCE, "motion_warp.hlsl", s!("Warp"), 3)
    }

    fn with_shader(device: &ID3D12Device, source: &str, name: &str, entry: windows::core::PCSTR, inputs: u32) -> Result<Self, String> {
        // The constants, then one table: the inputs and the output.
        let root = compute::root_signature(device, (size_of::<Constants>() / 4) as u32, &[Table { srvs: inputs, uavs: 1 }])?;
        let pipeline = compute::shader_pipeline(device, &root, source, name, entry)?;
        let heap = DescriptorHeap::new(device, SETS * (MAX_INPUTS + 1))?;
        Ok(Self { device: device.clone(), root, pipeline, heap, inputs, output: None, next: 0 })
    }

    /// Our output for motion vectors of this size, made on first use (in [`READABLE`]).
    pub fn output(&mut self, width: u32, height: u32) -> Result<ID3D12Resource, String> {
        if let Some((texture, w, h)) = &self.output
            && (*w, *h) == (width, height)
        {
            return Ok(texture.clone());
        }
        let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() };
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            Width: width as u64,
            Height: height,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: DXGI_FORMAT_R16G16B16A16_FLOAT,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Flags: D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
            ..Default::default()
        };
        let mut texture: Option<ID3D12Resource> = None;
        // SAFETY: creation with a valid descriptor into a local out-pointer.
        unsafe { self.device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, READABLE, None, &mut texture) }.map_err(|e| format!("output texture: {e}"))?;
        let texture = texture.ok_or("output texture: none")?;
        self.output = Some((texture.clone(), width, height));
        Ok(texture)
    }

    /// Records the pass on `list` (the game's native list before DLSS evaluates, or a list of the
    /// caller's): `inputs` are its textures in shader order (plane 0 of a depth, motion vectors;
    /// shader-readable, left so), as many as the pass reads; the result goes into
    /// [`MotionFix::output`], of `constants.size` or, when that is zero, of the last input's size,
    /// which ends in [`READABLE`].
    pub fn record(&mut self, list: &ID3D12GraphicsCommandList, inputs: &[&ID3D12Resource], constants: &Constants) -> Result<(), String> {
        if inputs.len() != self.inputs as usize {
            return Err(format!("{} inputs for a pass of {}", inputs.len(), self.inputs));
        }
        let mut formats = Vec::with_capacity(inputs.len());
        for input in inputs {
            // SAFETY: a descriptor read of a live resource.
            let desc = unsafe { input.GetDesc() };
            formats.push(read_format(desc.Format).ok_or_else(|| format!("input format {} not handled", desc.Format.0))?);
        }
        // SAFETY: a descriptor read of a live resource.
        let last = unsafe { inputs.last().ok_or("no inputs")?.GetDesc() };
        let (width, height) = if constants.size == [0, 0] { (last.Width as u32, last.Height) } else { (constants.size[0], constants.size[1]) };
        let output = self.output(width, height)?;
        let base = (self.next % SETS) * (MAX_INPUTS + 1);
        self.next = self.next.wrapping_add(1);
        let mut constants = *constants;
        constants.size = [width, height];
        // SAFETY: views of live resources written into our own heap; commands recorded on a list
        // being recorded, with our heap, root signature and pipeline (DLSS's evaluate, right after,
        // sets its own and Streamline puts the game's back).
        unsafe {
            for (k, (input, format)) in inputs.iter().zip(&formats).enumerate() {
                self.device.CreateShaderResourceView(*input, Some(&srv_desc(*format)), self.heap.cpu(base + k as u32));
            }
            self.device.CreateUnorderedAccessView(&output, None, Some(&uav_desc(DXGI_FORMAT_R16G16B16A16_FLOAT)), self.heap.cpu(base + self.inputs));
            barrier(list, &output, READABLE, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
            list.SetDescriptorHeaps(&[Some(self.heap.heap().clone())]);
            list.SetComputeRootSignature(&self.root);
            list.SetPipelineState(&self.pipeline);
            list.SetComputeRoot32BitConstants(0, (size_of::<Constants>() / 4) as u32, &constants as *const Constants as *const c_void, 0);
            list.SetComputeRootDescriptorTable(1, self.heap.gpu(base));
            list.Dispatch(width.div_ceil(THREADS), height.div_ceil(THREADS), 1);
            barrier(list, &output, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, READABLE);
        }
        Ok(())
    }
}

/// The rebase per-eye DLSS records ([`monaka_streamline::per_eye`]).
pub fn rebase(device: &ID3D12Device) -> Result<Box<dyn MotionRebase>, String> {
    Ok(Box::new(MotionFix::new(device)?))
}

impl MotionRebase for MotionFix {
    fn output(&mut self, width: u32, height: u32) -> Result<ID3D12Resource, String> {
        MotionFix::output(self, width, height)
    }

    fn record(&mut self, list: &ID3D12GraphicsCommandList, depth: &ID3D12Resource, motion: &ID3D12Resource, rebase: &Rebase) -> Result<(), String> {
        let constants = Constants { game_to_prev: rebase.game_to_prev, same_to_prev: rebase.same_to_prev, mvec_scale: rebase.mvec_scale, object_scale: rebase.object_scale, ..Constants::default() };
        MotionFix::record(self, list, &[depth, motion], &constants)
    }

    fn readable(&self) -> D3D12_RESOURCE_STATES {
        READABLE
    }
}

#[cfg(test)]
mod tests;
