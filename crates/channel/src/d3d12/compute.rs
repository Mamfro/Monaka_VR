//! Compute passes on D3D12: shaders compiled from HLSL source (cs_5_0), a root signature of root
//! constants and descriptor tables, pipelines on it, and a shader-visible descriptor heap.

use std::ffi::c_void;
use std::mem::ManuallyDrop;
use windows::Win32::Graphics::Direct3D::Fxc::{D3DCOMPILE_OPTIMIZATION_LEVEL3, D3DCompile};
use windows::Win32::Graphics::Direct3D::ID3DBlob;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;
use windows::core::{PCSTR, s};

/// A blob's bytes (compiled code, a serialized root signature).
pub fn blob_bytes(blob: &ID3DBlob) -> &[u8] {
    // SAFETY: a blob's buffer is valid for its size while the blob lives.
    unsafe { std::slice::from_raw_parts(blob.GetBufferPointer() as *const u8, blob.GetBufferSize()) }
}

/// Compute shader `entry` of `source` (cs_5_0, optimised); the error names `name` and the entry
/// and carries the compiler's text.
pub fn compile(source: &str, name: &str, entry: PCSTR) -> Result<ID3DBlob, String> {
    compile_for(source, name, entry, s!("cs_5_0"))
}

/// Shader `entry` of `source` for `target` (`vs_5_0`, `ps_5_0`, `cs_5_0`; optimised), as
/// [`compile`].
pub fn compile_for(source: &str, name: &str, entry: PCSTR, target: PCSTR) -> Result<ID3DBlob, String> {
    let mut code: Option<ID3DBlob> = None;
    let mut messages: Option<ID3DBlob> = None;
    // SAFETY: compiles our own source text into local out-pointers.
    let result = unsafe {
        D3DCompile(source.as_ptr() as *const c_void, source.len(), PCSTR::null(), None, None, entry, target, D3DCOMPILE_OPTIMIZATION_LEVEL3, 0, &mut code, Some(&mut messages))
    };
    match (result, code) {
        (Ok(()), Some(code)) => Ok(code),
        (result, _) => {
            let text = messages.as_ref().map(|m| String::from_utf8_lossy(blob_bytes(m)).into_owned()).unwrap_or_default();
            // SAFETY: the entry is a NUL-terminated name of ours.
            let entry = unsafe { entry.to_string() }.unwrap_or_default();
            Err(format!("{name} {entry}: {} {text}", result.err().map(|e| e.to_string()).unwrap_or_default()))
        }
    }
}

/// One descriptor table: `srvs` shader resource views from t0, then `uavs` unordered access views
/// from u0, in that order in the heap.
#[derive(Clone, Copy, Debug)]
pub struct Table {
    pub srvs: u32,
    pub uavs: u32,
}

/// A root signature: `constants` 32-bit root constants at b0 (parameter 0), then one descriptor
/// table per entry of `tables` (parameters 1 on).
pub fn root_signature(device: &ID3D12Device, constants: u32, tables: &[Table]) -> Result<ID3D12RootSignature, String> {
    let ranges: Vec<Vec<D3D12_DESCRIPTOR_RANGE>> = tables
        .iter()
        .map(|t| {
            let range = |kind, count, offset| D3D12_DESCRIPTOR_RANGE { RangeType: kind, NumDescriptors: count, OffsetInDescriptorsFromTableStart: offset, ..Default::default() };
            let mut ranges = Vec::new();
            if t.srvs > 0 {
                ranges.push(range(D3D12_DESCRIPTOR_RANGE_TYPE_SRV, t.srvs, 0));
            }
            if t.uavs > 0 {
                ranges.push(range(D3D12_DESCRIPTOR_RANGE_TYPE_UAV, t.uavs, t.srvs));
            }
            ranges
        })
        .collect();
    let mut params = vec![D3D12_ROOT_PARAMETER {
        ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
        Anonymous: D3D12_ROOT_PARAMETER_0 { Constants: D3D12_ROOT_CONSTANTS { ShaderRegister: 0, RegisterSpace: 0, Num32BitValues: constants } },
        ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
    }];
    params.extend(ranges.iter().map(|r| D3D12_ROOT_PARAMETER {
        ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
        Anonymous: D3D12_ROOT_PARAMETER_0 { DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE { NumDescriptorRanges: r.len() as u32, pDescriptorRanges: r.as_ptr() } },
        ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
    }));
    let desc = D3D12_ROOT_SIGNATURE_DESC { NumParameters: params.len() as u32, pParameters: params.as_ptr(), ..Default::default() };
    let mut blob: Option<ID3DBlob> = None;
    // SAFETY: serializes a descriptor whose arrays outlive the call, then creates on a live device.
    unsafe {
        D3D12SerializeRootSignature(&desc, D3D_ROOT_SIGNATURE_VERSION_1, &mut blob, None).map_err(|e| format!("root signature: {e}"))?;
        let blob = blob.ok_or("root signature: no blob")?;
        device.CreateRootSignature(0, blob_bytes(&blob)).map_err(|e| format!("root signature: {e}"))
    }
}

/// A compute pipeline running `code` on `root`.
pub fn pipeline(device: &ID3D12Device, root: &ID3D12RootSignature, code: &ID3DBlob) -> Result<ID3D12PipelineState, String> {
    let desc = D3D12_COMPUTE_PIPELINE_STATE_DESC {
        pRootSignature: ManuallyDrop::new(Some(root.clone())),
        // SAFETY: the blob's buffer outlives the call.
        CS: D3D12_SHADER_BYTECODE { pShaderBytecode: unsafe { code.GetBufferPointer() }, BytecodeLength: unsafe { code.GetBufferSize() } },
        ..Default::default()
    };
    // SAFETY: a valid descriptor on a live device; the root signature reference taken above is
    // given back right after.
    let made = unsafe { device.CreateComputePipelineState(&desc) };
    drop(ManuallyDrop::into_inner(desc.pRootSignature));
    made.map_err(|e| format!("pipeline: {e}"))
}

/// [`compile`] then [`pipeline`].
pub fn shader_pipeline(device: &ID3D12Device, root: &ID3D12RootSignature, source: &str, name: &str, entry: PCSTR) -> Result<ID3D12PipelineState, String> {
    pipeline(device, root, &compile(source, name, entry)?)
}

/// A shader-visible heap of shader resource and unordered access views.
pub struct DescriptorHeap {
    heap: ID3D12DescriptorHeap,
    increment: u32,
}

impl DescriptorHeap {
    pub fn new(device: &ID3D12Device, count: u32) -> Result<Self, String> {
        let desc = D3D12_DESCRIPTOR_HEAP_DESC { Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, NumDescriptors: count, Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE, NodeMask: 0 };
        // SAFETY: creation calls on a live device.
        unsafe {
            let heap = device.CreateDescriptorHeap(&desc).map_err(|e| format!("descriptor heap: {e}"))?;
            Ok(Self { heap, increment: device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV) })
        }
    }

    /// The heap, for `SetDescriptorHeaps`.
    pub fn heap(&self) -> &ID3D12DescriptorHeap {
        &self.heap
    }

    /// Where view `index` is written.
    pub fn cpu(&self, index: u32) -> D3D12_CPU_DESCRIPTOR_HANDLE {
        // SAFETY: reads the heap's start handle.
        let start = unsafe { self.heap.GetCPUDescriptorHandleForHeapStart() };
        D3D12_CPU_DESCRIPTOR_HANDLE { ptr: start.ptr + (index * self.increment) as usize }
    }

    /// Where a table starting at view `index` is bound.
    pub fn gpu(&self, index: u32) -> D3D12_GPU_DESCRIPTOR_HANDLE {
        // SAFETY: reads the heap's start handle.
        let start = unsafe { self.heap.GetGPUDescriptorHandleForHeapStart() };
        D3D12_GPU_DESCRIPTOR_HANDLE { ptr: start.ptr + (index * self.increment) as u64 }
    }
}

/// A view of a 2D texture's first mip as `format`.
pub fn srv_desc(format: DXGI_FORMAT) -> D3D12_SHADER_RESOURCE_VIEW_DESC {
    D3D12_SHADER_RESOURCE_VIEW_DESC {
        Format: format,
        ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
        Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
        Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 { Texture2D: D3D12_TEX2D_SRV { MipLevels: 1, ..Default::default() } },
    }
}

/// An unordered access view of a 2D texture's first mip as `format`.
pub fn uav_desc(format: DXGI_FORMAT) -> D3D12_UNORDERED_ACCESS_VIEW_DESC {
    D3D12_UNORDERED_ACCESS_VIEW_DESC { Format: format, ViewDimension: D3D12_UAV_DIMENSION_TEXTURE2D, ..Default::default() }
}

#[cfg(test)]
mod tests;
