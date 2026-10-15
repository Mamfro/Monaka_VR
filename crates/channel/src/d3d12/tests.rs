use super::*;
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM;

/// A 5x3 RGBA8 texture (20-byte rows, so the 256-byte copy pitch must be packed away) uploaded
/// through a placed footprint and read back whole.
#[test]
fn a_texture_comes_back_tightly_packed() {
    let mut device: Option<ID3D12Device> = None;
    // SAFETY: test setup: a device, its queue, an upload buffer filled through its mapping, and
    // a copy recorded on our own list.
    unsafe {
        D3D12CreateDevice(None::<&windows::core::IUnknown>, D3D_FEATURE_LEVEL_11_0, &mut device).unwrap();
        let device = device.unwrap();
        let queue: ID3D12CommandQueue = device.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC { Type: D3D12_COMMAND_LIST_TYPE_DIRECT, ..Default::default() }).unwrap();
        let target = texture(&device, 5, 3, DXGI_FORMAT_R8G8B8A8_UNORM, D3D12_RESOURCE_FLAG_NONE).unwrap();
        let pixels: Vec<u8> = (0..5 * 3 * 4).map(|i| i as u8).collect();
        let desc = target.GetDesc();
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
        let mut upload: Option<ID3D12Resource> = None;
        device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &buffer_desc, D3D12_RESOURCE_STATE_GENERIC_READ, None, &mut upload).unwrap();
        let upload = upload.unwrap();
        let mut mapped = std::ptr::null_mut();
        upload.Map(0, None, Some(&mut mapped)).unwrap();
        for (y, row) in pixels.chunks_exact(20).enumerate() {
            std::ptr::copy_nonoverlapping(row.as_ptr(), mapped.cast::<u8>().add(y * footprint.Footprint.RowPitch as usize), row.len());
        }
        upload.Unmap(0, None);
        let mut recorder = Recorder::new(&device, 1).unwrap();
        let list = recorder.begin().unwrap().unwrap();
        barrier(&list, &target, D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_COPY_DEST);
        list.CopyTextureRegion(&subresource(&target, 0), 0, 0, 0, &placed(&upload, footprint), None);
        barrier(&list, &target, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COMMON);
        recorder.submit(&queue).unwrap();
        assert!(recorder.idle(2000));
        assert_eq!(read_texture(&queue, &target, D3D12_RESOURCE_STATE_COMMON).unwrap(), (5, 3, 20, pixels));
    }
}
