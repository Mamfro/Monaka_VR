use super::*;
use crate::state;
use crate::tests::{api, copy_buffer_to_texture, copy_texture_to_buffer, read_pixel, readback_buffer, upload_buffer};
use monaka_channel::d3d12::{self, Recorder};
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_12_0;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R16G16_FLOAT, DXGI_FORMAT_R32_FLOAT};

/// A flat picture rendered at 128x72 and upscaled to 256x144 over a few jittered frames comes
/// out as the same flat picture.
#[test]
fn upscales_a_still_picture() {
    let Some(api) = api() else { return eprintln!("Dying Light 2 not installed here; skipped") };
    // SAFETY: plain D3D12 setup for a test.
    let (device, queue) = unsafe {
        let mut device: Option<ID3D12Device> = None;
        D3D12CreateDevice(None::<&windows::core::IUnknown>, D3D_FEATURE_LEVEL_12_0, &mut device).unwrap();
        let device = device.unwrap();
        let queue: ID3D12CommandQueue = device.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC { Type: D3D12_COMMAND_LIST_TYPE_DIRECT, ..Default::default() }).unwrap();
        (device, queue)
    };
    let (rw, rh, dw, dh) = (128, 72, 256, 144);
    let render = Size2 { width: rw, height: rh };
    let display = Size2 { width: dw, height: dh };
    let mut upscaler = Upscaler::new(api, &device, render, display, create::DEPTH_INVERTED | create::DEPTH_INFINITE).unwrap();
    eprintln!("provider: {}", upscaler.version());
    let phases = upscaler.jitter_phase_count(rw).unwrap();
    assert!(phases >= 8, "a 2x upscale cycles through at least 8 jitter phases, got {phases}");
    let none = D3D12_RESOURCE_FLAG_NONE;
    let color = d3d12::texture(&device, rw, rh, DXGI_FORMAT_R8G8B8A8_UNORM, none).unwrap();
    let depth = d3d12::texture(&device, rw, rh, DXGI_FORMAT_R32_FLOAT, none).unwrap();
    let motion = d3d12::texture(&device, rw, rh, DXGI_FORMAT_R16G16_FLOAT, none).unwrap();
    let output = d3d12::texture(&device, dw, dh, DXGI_FORMAT_R8G8B8A8_UNORM, D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS).unwrap();
    let mut recorder = Recorder::new(&device, 2).unwrap();
    let upload = upload_buffer(&device, rw, rh, [40, 120, 200, 255]);
    let readback = readback_buffer(&device, dw, dh);
    for frame in 0..6 {
        let jitter = upscaler.jitter_offset(frame, phases).unwrap();
        assert!(jitter.x.abs() <= 0.5 && jitter.y.abs() <= 0.5, "jitter stays within a pixel: {jitter:?}");
        let list = loop {
            if let Some(list) = recorder.begin().unwrap() {
                break list;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        };
        copy_buffer_to_texture(&list, &upload, &color, rw, rh);
        let inputs = Frame {
            color: Resource::texture(&color, state::COMMON, None),
            depth: Resource::texture(&depth, state::COMMON, None),
            motion_vectors: Resource::texture(&motion, state::COMMON, None),
            reactive: None,
            output: Resource::texture(&output, state::COMMON, None),
            render_size: render,
            jitter,
            motion_vector_scale: Float2 { x: rw as f32, y: rh as f32 },
            sharpness: None,
            frame_time_ms: 16.6,
            reset: frame == 0,
            near: 0.05,
            far: 10_000.0,
            fov_vertical: 1.0,
            flags: 0,
        };
        upscaler.dispatch(&list, &inputs).unwrap();
        if frame == 5 {
            copy_texture_to_buffer(&list, &output, &readback, dw, dh);
        }
        recorder.submit(&queue).unwrap();
    }
    assert!(recorder.idle(5000), "the GPU finished the upscaled frames");
    let pixel = read_pixel(&readback, dw, 200, 100);
    eprintln!("upscaled pixel {pixel:?}");
    for (got, want) in pixel.iter().zip([40u8, 120, 200]) {
        assert!(got.abs_diff(want) <= 3, "upscaled {pixel:?}, expected the still picture");
    }
}
