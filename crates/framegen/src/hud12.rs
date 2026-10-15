//! The HUD around frame generation (D3D12). Frame generation must not interpolate the HUD (it
//! would move with the world), and the game's own HUD-less frame is black while its DLSS frame
//! generation is held off (in same-frame stereo). Its UI layer is real, though,
//! and premultiplied: frame = world * (1 - alpha) + ui. So [`HudComposer::unblend`] recovers the
//! world from the frame and the layer, frame generation interpolates that, and
//! [`HudComposer::compose`] lays the newest real frame's layer over the generated frame. The shaders
//! are `hud12.hlsl`.

use monaka_channel::d3d12::barrier;
use monaka_channel::d3d12::compute::{self, DescriptorHeap, Table};
use std::ffi::c_void;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM;
use windows::core::s;

const SOURCE: &str = include_str!("hud12.hlsl");
/// One table: the UI layer, the frame read (t0, t1) and the texture written (u0).
const TABLE: u32 = 3;
/// Per eye: the un-blend table and the compose table.
const PER_EYE: u32 = 2 * TABLE;

/// The textures one table's views were made for.
type Viewed = Option<[ID3D12Resource; 3]>;

pub struct HudComposer {
    device: ID3D12Device,
    root: ID3D12RootSignature,
    unblend: ID3D12PipelineState,
    compose: ID3D12PipelineState,
    heap: DescriptorHeap,
    /// Views are rewritten only when a table's textures change (a new texture means the work that
    /// used the old views is being replaced).
    viewed: [[Viewed; 2]; 2],
}

// SAFETY: D3D12 objects are free-threaded; the composer is used under its owner's lock.
unsafe impl Send for HudComposer {}

impl HudComposer {
    pub fn new(device: &ID3D12Device) -> Result<Self, String> {
        let mut options = D3D12_FEATURE_DATA_D3D12_OPTIONS::default();
        // SAFETY: a feature query into a local.
        let supported = unsafe { device.CheckFeatureSupport(D3D12_FEATURE_D3D12_OPTIONS, &mut options as *mut _ as *mut c_void, size_of_val(&options) as u32) }.is_ok()
            && options.TypedUAVLoadAdditionalFormats.as_bool();
        if !supported {
            return Err("the GPU cannot read RGBA8 storage textures in shaders".into());
        }
        let root = compute::root_signature(device, 2, &[Table { srvs: 2, uavs: 1 }])?;
        let pipeline = |entry| compute::shader_pipeline(device, &root, &monaka_core::hud::with_hlsl(SOURCE), "hud12.hlsl", entry).map_err(|e| format!("HUD {e}"));
        let (unblend, compose) = (pipeline(s!("Unblend"))?, pipeline(s!("Compose"))?);
        let heap = DescriptorHeap::new(device, 2 * PER_EYE).map_err(|e| format!("HUD {e}"))?;
        Ok(Self { device: device.clone(), root, unblend, compose, heap, viewed: Default::default() })
    }

    /// Runs `pipeline` for eye `eye` with table `table` (0 un-blend, 1 compose) reading `ui` and
    /// `frame` and writing `target` (all RGBA8, one size, in COMMON; `target` allows unordered
    /// access). Everything ends in COMMON.
    fn run(&mut self, list: &ID3D12GraphicsCommandList, eye: usize, table: usize, ui: &ID3D12Resource, frame: &ID3D12Resource, target: &ID3D12Resource) {
        let base = eye as u32 * PER_EYE + table as u32 * TABLE;
        let textures = [ui.clone(), frame.clone(), target.clone()];
        if self.viewed[eye][table].as_ref() != Some(&textures) {
            let (srv, uav) = (compute::srv_desc(DXGI_FORMAT_R8G8B8A8_UNORM), compute::uav_desc(DXGI_FORMAT_R8G8B8A8_UNORM));
            // SAFETY: views of live textures into our own heap.
            unsafe {
                self.device.CreateShaderResourceView(ui, Some(&srv), self.heap.cpu(base));
                self.device.CreateShaderResourceView(frame, Some(&srv), self.heap.cpu(base + 1));
                self.device.CreateUnorderedAccessView(target, None, Some(&uav), self.heap.cpu(base + 2));
            }
            self.viewed[eye][table] = Some(textures);
        }
        // SAFETY: reads a descriptor.
        let desc = unsafe { target.GetDesc() };
        let size = [desc.Width as u32, desc.Height];
        let reads: Vec<&ID3D12Resource> = if frame == target { vec![ui] } else { vec![ui, frame] };
        for r in &reads {
            barrier(list, r, D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE);
        }
        barrier(list, target, D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
        let pipeline = if table == 0 { &self.unblend } else { &self.compose };
        // SAFETY: records compute work with our own heap, root signature and pipelines.
        unsafe {
            list.SetDescriptorHeaps(&[Some(self.heap.heap().clone())]);
            list.SetComputeRootSignature(&self.root);
            list.SetPipelineState(pipeline);
            list.SetComputeRoot32BitConstants(0, 2, size.as_ptr() as *const c_void, 0);
            list.SetComputeRootDescriptorTable(1, self.heap.gpu(base));
            list.Dispatch(size[0].div_ceil(8), size[1].div_ceil(8), 1);
        }
        barrier(list, target, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COMMON);
        for r in &reads {
            barrier(list, r, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE, D3D12_RESOURCE_STATE_COMMON);
        }
    }

    /// Records the world without the HUD: `frame` minus the UI layer `ui`, into `world`.
    pub fn unblend(&mut self, list: &ID3D12GraphicsCommandList, eye: usize, ui: &ID3D12Resource, frame: &ID3D12Resource, world: &ID3D12Resource) {
        self.run(list, eye, 0, ui, frame, world);
    }

    /// Records laying the UI layer `ui` over `frame` in place.
    pub fn compose(&mut self, list: &ID3D12GraphicsCommandList, eye: usize, ui: &ID3D12Resource, frame: &ID3D12Resource) {
        self.run(list, eye, 1, ui, frame, frame);
    }
}

#[cfg(test)]
mod tests;
