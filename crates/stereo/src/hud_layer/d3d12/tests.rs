use super::*;
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;

#[test]
fn the_shader_compiles_and_the_lay_over_initialises() {
    let mut device: Option<ID3D12Device> = None;
    // SAFETY: creates a device on the default adapter.
    unsafe { D3D12CreateDevice(None::<&windows::core::IUnknown>, D3D_FEATURE_LEVEL_11_0, &mut device) }.unwrap();
    make(&device.unwrap()).unwrap();
    assert_eq!(size_of::<Constants>(), 128, "the shader's constant buffer");
    let params = Params::default().leaving_out(&[[1.0; 4]; 7]);
    assert_eq!(params.piece_count, 5, "at most five pieces");
}

/// A 64x36 B8G8R8A8 texture cleared to `rgba`, in COMMON, the GPU done with it.
fn cleared(device: &ID3D12Device, queue: &ID3D12CommandQueue, rgba: [f32; 4]) -> ID3D12Resource {
    use monaka_channel::d3d12::{barrier, wait_fence};
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
    use windows::core::Interface;
    // SAFETY: creation and recording on our own test device and queue.
    unsafe {
        let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() };
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            Width: 64,
            Height: 36,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Flags: D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
            ..Default::default()
        };
        let mut texture: Option<ID3D12Resource> = None;
        device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_RENDER_TARGET, None, &mut texture).unwrap();
        let texture = texture.unwrap();
        let views: ID3D12DescriptorHeap = device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC { Type: D3D12_DESCRIPTOR_HEAP_TYPE_RTV, NumDescriptors: 1, ..Default::default() }).unwrap();
        let view = views.GetCPUDescriptorHandleForHeapStart();
        device.CreateRenderTargetView(&texture, None, view);
        let allocator: ID3D12CommandAllocator = device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT).unwrap();
        let list: ID3D12GraphicsCommandList = device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None::<&ID3D12PipelineState>).unwrap();
        list.ClearRenderTargetView(view, &rgba, None);
        barrier(&list, &texture, D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATE_COMMON);
        list.Close().unwrap();
        queue.ExecuteCommandLists(&[Some(list.cast::<ID3D12CommandList>().unwrap())]);
        let fence: ID3D12Fence = device.CreateFence(0, D3D12_FENCE_FLAG_NONE).unwrap();
        queue.Signal(&fence, 1).unwrap();
        assert!(wait_fence(&fence, 1, 5000));
        texture
    }
}

/// The frame's middle pixel (blue, green, red, alpha bytes).
fn middle(queue: &ID3D12CommandQueue, frame: &ID3D12Resource) -> [u8; 4] {
    let (width, height, row, bytes) = monaka_channel::d3d12::read_texture(queue, frame, D3D12_RESOURCE_STATE_COMMON).unwrap();
    let at = (height as usize / 2) * row + (width as usize / 2) * 4;
    bytes[at..at + 4].try_into().unwrap()
}

#[test]
fn a_see_through_layer_covers_where_its_alpha_is_low_and_leaves_the_frame_where_high() {
    let mut device: Option<ID3D12Device> = None;
    // SAFETY: creates a device on the default adapter.
    unsafe { D3D12CreateDevice(None::<&windows::core::IUnknown>, D3D_FEATURE_LEVEL_11_0, &mut device) }.unwrap();
    let device = device.unwrap();
    // SAFETY: a queue on our own device.
    let queue: ID3D12CommandQueue = unsafe { device.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC { Type: D3D12_COMMAND_LIST_TYPE_DIRECT, ..Default::default() }) }.unwrap();
    let params = Params { rect: [0.0, 0.0, 64.0, 36.0], see_through: true, ..Default::default() };
    // Nothing drawn (alpha 1: all shows through): the blue frame stays.
    let frame = cleared(&device, &queue, [0.0, 0.0, 1.0, 1.0]);
    assert!(lay(&queue, &frame, D3D12_RESOURCE_STATE_COMMON, &cleared(&device, &queue, [0.0, 0.0, 0.0, 1.0]), &params));
    assert_eq!(middle(&queue, &frame), [255, 0, 0, 255]);
    // Red drawn fully (alpha 0: nothing shows through): red.
    let frame = cleared(&device, &queue, [0.0, 0.0, 1.0, 1.0]);
    assert!(lay(&queue, &frame, D3D12_RESOURCE_STATE_COMMON, &cleared(&device, &queue, [1.0, 0.0, 0.0, 0.0]), &params));
    assert_eq!(middle(&queue, &frame), [0, 0, 255, 255]);
}
