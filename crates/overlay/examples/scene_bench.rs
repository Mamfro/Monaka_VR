//! How long the GPU takes to draw a home scene for one eye, timed with GPU timestamps, at a headset
//! eye's size; and how long reading a time of day's colours takes.
//!
//! ```text
//! cargo run --release -p monaka_overlay --example scene_bench -- <scene folder or .monakaroom> [eye size]
//! ```

use monaka_channel::d3d::{create_device, create_texture, simple_texture_desc};
use monaka_overlay::eye::EyeView;
use monaka_overlay::scene::{ScenePass, files};
use std::path::PathBuf;
use std::time::Instant;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM_SRGB;

const FRAMES: usize = 60;

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let path = PathBuf::from(args.first().ok_or("give: <scene> [eye size]")?);
    let size: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(2160);
    let started = Instant::now();
    let loaded = files::load(&path, |times| times.iter().copied().filter(|t| *t == 15 * 60).collect())?;
    println!("{}: read in {} ms", loaded.name, started.elapsed().as_millis());
    let places: Vec<&str> = loaded.places.iter().map(|p| p.name).collect();
    let started = Instant::now();
    files::load_time(&path, &places, 14 * 60 + 30, loaded.face)?;
    println!("one time of day's colours (14:30, all points) read in {} ms", started.elapsed().as_millis());
    let tiles: usize = loaded.places.iter().map(|p| p.tiles.len() + p.skirts.len()).sum();
    let (device, context, _) = create_device(None).map_err(|e| e.to_string())?;
    let mut pass = ScenePass::new(&device, &loaded)?;
    let mut desc = simple_texture_desc(size, size, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB);
    desc.BindFlags = (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32;
    let image = create_texture(&device, &desc).map_err(|e| e.to_string())?;
    let query = |kind| -> Result<ID3D11Query, String> {
        let mut q = None;
        // SAFETY: a valid descriptor and a local out-pointer.
        unsafe { device.CreateQuery(&D3D11_QUERY_DESC { Query: kind, MiscFlags: 0 }, Some(&mut q)) }.map_err(|e| e.to_string())?;
        q.ok_or_else(|| "no query".to_owned())
    };
    let (disjoint, begin, end) = (query(D3D11_QUERY_TIMESTAMP_DISJOINT)?, query(D3D11_QUERY_TIMESTAMP)?, query(D3D11_QUERY_TIMESTAMP)?);
    let half = 50f32.to_radians();
    let mut times = Vec::new();
    for frame in 0..FRAMES + 5 {
        let yaw = (frame as f32 * 7.0).to_radians() / 2.0;
        let eye = EyeView { orientation: [0.0, yaw.sin(), 0.0, yaw.cos()], position: [0.02, 0.0, 0.0], fov: [-half, half, half, -half] };
        // SAFETY: our own live queries on the immediate context, read once the GPU is done.
        unsafe {
            context.Begin(&disjoint);
            context.End(&begin);
        }
        pass.draw(&device, &context, &image, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB, &eye, 15 * 60, 15 * 60, 0.0)?;
        // SAFETY: as above.
        unsafe {
            context.End(&end);
            context.End(&disjoint);
            let mut timing = D3D11_QUERY_DATA_TIMESTAMP_DISJOINT::default();
            // "Not ready" comes back as success too: wait until the answer is filled in.
            while timing.Frequency == 0 {
                let _ = context.GetData(&disjoint, Some((&mut timing as *mut D3D11_QUERY_DATA_TIMESTAMP_DISJOINT).cast()), size_of::<D3D11_QUERY_DATA_TIMESTAMP_DISJOINT>() as u32, 0);
            }
            let (mut a, mut b) = (0u64, 0u64);
            while a == 0 || b == 0 {
                let _ = context.GetData(&begin, Some((&mut a as *mut u64).cast()), 8, 0);
                let _ = context.GetData(&end, Some((&mut b as *mut u64).cast()), 8, 0);
            }
            if frame >= 5 && !timing.Disjoint.as_bool() {
                times.push((b - a) as f64 / timing.Frequency as f64 * 1000.0);
            }
        }
    }
    times.sort_by(f64::total_cmp);
    let median = times[times.len() / 2];
    println!("{} tiles; one eye at {size}x{size}: {median:.2} ms on the GPU (median of {}), both eyes {:.2} ms", tiles, times.len(), median * 2.0);
    Ok(())
}
