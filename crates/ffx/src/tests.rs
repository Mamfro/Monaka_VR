use super::*;
use monaka_channel::d3d12::{self, Recorder};
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_12_0;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R16G16_FLOAT, DXGI_FORMAT_R32_FLOAT};
use windows::Win32::System::LibraryLoader::SetDllDirectoryW;

/// A folder with AMD's FidelityFX DLLs, from `MONAKA_FFX_DIR` (a game installs them: Dying
/// Light 2's are in its `ph\work\bin\x64`). The tests that need them pass without it.
pub(crate) fn ffx_dir() -> Option<std::path::PathBuf> {
    let dir = std::path::PathBuf::from(std::env::var_os("MONAKA_FFX_DIR")?);
    dir.join(LOADER).exists().then_some(dir)
}

/// The loader from [`ffx_dir`], or none on a machine without one.
pub(crate) fn api() -> Option<Api> {
    let dir = ffx_dir()?;
    // SAFETY: points the DLL search path at that folder, where the loader finds its providers
    // (in a game it is the executable's folder).
    unsafe { SetDllDirectoryW(&HSTRING::from(dir.as_path())) }.ok()?;
    Api::load(Some(&dir), &[]).ok()
}

#[test]
fn a_games_ffx_states_are_the_d3d12_backends() {
    let readable = D3D12_RESOURCE_STATES(D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE.0 | D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE.0);
    assert_eq!(state::d3d12(state::PIXEL_COMPUTE_READ), Some(readable));
    assert_eq!(state::d3d12(state::COMMON), Some(D3D12_RESOURCE_STATE_COMMON));
    assert_eq!(state::d3d12(state::COMPUTE_READ), Some(D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE));
    assert_eq!(state::d3d12(state::GENERIC_READ), Some(D3D12_RESOURCE_STATE_GENERIC_READ));
    assert_eq!(state::d3d12(state::UNORDERED_ACCESS), Some(D3D12_RESOURCE_STATE_UNORDERED_ACCESS));
    assert_eq!(state::d3d12(0), None);
    assert_eq!(state::d3d12(1 << 12), None);
}

#[test]
fn interpolates_without_a_swapchain() {
    run(false);
}

#[test]
fn interpolates_with_a_hudless_frame() {
    run(true);
}

/// Three frames of a still picture; the generated frame must be the picture. With `hudless`
/// each frame also passes a HUD-less copy of it (no HUD: the two are equal).
fn run(hudless: bool) {
    let Some(api) = api() else { return eprintln!("Dying Light 2 not installed here; skipped") };
    // SAFETY: plain D3D12 setup for a test.
    let (device, queue) = unsafe {
        let mut device: Option<ID3D12Device> = None;
        D3D12CreateDevice(None::<&windows::core::IUnknown>, D3D_FEATURE_LEVEL_12_0, &mut device).unwrap();
        let device = device.unwrap();
        let queue: ID3D12CommandQueue = device.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC { Type: D3D12_COMMAND_LIST_TYPE_DIRECT, ..Default::default() }).unwrap();
        (device, queue)
    };
    let (w, h) = (256, 144);
    let size = Size2 { width: w, height: h };
    let mut generator = FrameGen::new(api, &device, size, size, DXGI_FORMAT_R8G8B8A8_UNORM, create::DEPTH_INVERTED | create::DEPTH_INFINITE).unwrap();
    eprintln!("provider: {}", generator.version());
    let none = D3D12_RESOURCE_FLAG_NONE;
    let color = d3d12::texture(&device, w, h, DXGI_FORMAT_R8G8B8A8_UNORM, none).unwrap();
    let depth = d3d12::texture(&device, w, h, DXGI_FORMAT_R32_FLOAT, none).unwrap();
    let motion = d3d12::texture(&device, w, h, DXGI_FORMAT_R16G16_FLOAT, none).unwrap();
    let output = d3d12::texture(&device, w, h, DXGI_FORMAT_R8G8B8A8_UNORM, D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS).unwrap();
    let hud_free = d3d12::texture(&device, w, h, DXGI_FORMAT_R8G8B8A8_UNORM, none).unwrap();
    let camera = Camera {
        render_size: size,
        motion_vector_scale: Float2 { x: 1.0, y: 1.0 },
        near: 0.05,
        far: 10_000.0,
        fov_vertical: 1.0,
        up: [0.0, 1.0, 0.0],
        right: [1.0, 0.0, 0.0],
        forward: [0.0, 0.0, -1.0],
        ..Camera::default()
    };
    let mut recorder = Recorder::new(&device, 2).unwrap();
    // A still picture: the frame halfway between two copies of it is the picture.
    let upload = upload_buffer(&device, w, h, [40, 120, 200, 255]);
    let readback = readback_buffer(&device, w, h);
    for frame in 1..=3u64 {
        let list = loop {
            if let Some(list) = recorder.begin().unwrap() {
                break list;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        };
        copy_buffer_to_texture(&list, &upload, &color, w, h);
        if hudless {
            copy_buffer_to_texture(&list, &upload, &hud_free, w, h);
            generator.configure(frame, Some(Resource::texture(&hud_free, state::COMMON, None))).unwrap();
        }
        generator
            .prepare(&list, frame, &camera, Resource::texture(&depth, state::COMMON, None), Resource::texture(&motion, state::COMMON, None), 16.6)
            .unwrap();
        generator.generate(&list, frame, Resource::texture(&color, state::COMMON, None), Resource::texture(&output, state::COMMON, None), frame == 1).unwrap();
        if frame == 3 {
            copy_texture_to_buffer(&list, &output, &readback, w, h);
        }
        recorder.submit(&queue).unwrap();
    }
    assert!(recorder.idle(5000), "the GPU finished the generated frames");
    let pixel = read_pixel(&readback, w, 100, 70);
    eprintln!("generated pixel {pixel:?}");
    for (got, want) in pixel.iter().zip([40u8, 120, 200]) {
        assert!(got.abs_diff(want) <= 2, "generated {pixel:?}, expected the still picture");
    }
}

const PITCH_ALIGN: u32 = 256;

fn pitch(w: u32) -> u32 {
    (w * 4).div_ceil(PITCH_ALIGN) * PITCH_ALIGN
}

fn buffer(device: &ID3D12Device, bytes: u64, heap: D3D12_HEAP_TYPE, state: D3D12_RESOURCE_STATES) -> ID3D12Resource {
    let props = D3D12_HEAP_PROPERTIES { Type: heap, ..Default::default() };
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: bytes,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        ..Default::default()
    };
    let mut out: Option<ID3D12Resource> = None;
    // SAFETY: test resource creation.
    unsafe { device.CreateCommittedResource(&props, D3D12_HEAP_FLAG_NONE, &desc, state, None, &mut out) }.unwrap();
    out.unwrap()
}

pub(crate) fn upload_buffer(device: &ID3D12Device, w: u32, h: u32, rgba: [u8; 4]) -> ID3D12Resource {
    let upload = buffer(device, (pitch(w) * h) as u64, D3D12_HEAP_TYPE_UPLOAD, D3D12_RESOURCE_STATE_GENERIC_READ);
    // SAFETY: fills our own upload buffer.
    unsafe {
        let mut data = std::ptr::null_mut();
        upload.Map(0, None, Some(&mut data)).unwrap();
        let bytes = std::slice::from_raw_parts_mut(data as *mut u8, (pitch(w) * h) as usize);
        for pixel in bytes.chunks_exact_mut(4) {
            pixel.copy_from_slice(&rgba);
        }
        upload.Unmap(0, None);
    }
    upload
}

pub(crate) fn readback_buffer(device: &ID3D12Device, w: u32, h: u32) -> ID3D12Resource {
    buffer(device, (pitch(w) * h) as u64, D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST)
}

fn footprint(buffer: &ID3D12Resource, w: u32, h: u32) -> D3D12_TEXTURE_COPY_LOCATION {
    D3D12_TEXTURE_COPY_LOCATION {
        // SAFETY: borrowed for the copy; ManuallyDrop skips the Release.
        pResource: std::mem::ManuallyDrop::new(Some(unsafe { std::mem::transmute_copy(buffer) })),
        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                Offset: 0,
                Footprint: D3D12_SUBRESOURCE_FOOTPRINT { Format: DXGI_FORMAT_R8G8B8A8_UNORM, Width: w, Height: h, Depth: 1, RowPitch: pitch(w) },
            },
        },
    }
}

pub(crate) fn copy_buffer_to_texture(list: &ID3D12GraphicsCommandList, upload: &ID3D12Resource, texture: &ID3D12Resource, w: u32, h: u32) {
    d3d12::barrier(list, texture, D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_COPY_DEST);
    // SAFETY: records a copy between live test resources.
    unsafe { list.CopyTextureRegion(&d3d12::subresource(texture, 0), 0, 0, 0, &footprint(upload, w, h), None) };
    d3d12::barrier(list, texture, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COMMON);
}

pub(crate) fn copy_texture_to_buffer(list: &ID3D12GraphicsCommandList, texture: &ID3D12Resource, readback: &ID3D12Resource, w: u32, h: u32) {
    d3d12::barrier(list, texture, D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_COPY_SOURCE);
    // SAFETY: records a copy between live test resources.
    unsafe { list.CopyTextureRegion(&footprint(readback, w, h), 0, 0, 0, &d3d12::subresource(texture, 0), None) };
    d3d12::barrier(list, texture, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_COMMON);
}

pub(crate) fn read_pixel(readback: &ID3D12Resource, w: u32, x: u32, y: u32) -> [u8; 4] {
    // SAFETY: maps our own finished readback buffer.
    unsafe {
        let mut data = std::ptr::null_mut();
        readback.Map(0, None, Some(&mut data)).unwrap();
        let at = (data as *const u8).add((y * pitch(w) + x * 4) as usize);
        let pixel = [*at, *at.add(1), *at.add(2), *at.add(3)];
        readback.Unmap(0, None);
        pixel
    }
}
