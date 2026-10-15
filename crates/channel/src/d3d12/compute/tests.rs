use super::*;
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;

const SOURCE: &str = "cbuffer C : register(b0) { uint N; }; Texture2D<float> In : register(t0); RWTexture2D<float> Out : register(u0);\n\
    [numthreads(8, 8, 1)] void Main(uint3 id : SV_DispatchThreadID) { if (id.x < N) Out[id.xy] = In.Load(int3(id.xy, 0)); }";

#[test]
fn a_pass_is_made_and_a_bad_shader_is_named() {
    let mut device: Option<ID3D12Device> = None;
    // SAFETY: plain device creation for a test.
    unsafe { D3D12CreateDevice(None::<&windows::core::IUnknown>, D3D_FEATURE_LEVEL_11_0, &mut device) }.unwrap();
    let device = device.unwrap();
    let root = root_signature(&device, 1, &[Table { srvs: 1, uavs: 1 }]).unwrap();
    shader_pipeline(&device, &root, SOURCE, "test.hlsl", s!("Main")).unwrap();
    let heap = DescriptorHeap::new(&device, 4).unwrap();
    assert!(heap.cpu(3).ptr > heap.cpu(0).ptr && heap.gpu(1).ptr > heap.gpu(0).ptr);
    let error = compile("this is not hlsl", "broken.hlsl", s!("Main")).unwrap_err();
    assert!(error.starts_with("broken.hlsl Main:"), "{error}");
}
