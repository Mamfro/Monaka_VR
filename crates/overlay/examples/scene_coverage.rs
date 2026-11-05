//! How much of what a leaning head sees no capture point of a scene saw, for sets of its capture
//! points: to judge how many a scene needs and where. The gaps are counted with the skirts left
//! out (what they would have to make up), over heads leaning `reach` metres every way and looking
//! four ways round.
//!
//! ```text
//! cargo run --release -p monaka_overlay --example scene_coverage -- <scene folder> [reach] [c,l,r ...]
//! ```
//!
//! With no sets given, it tries: the middle alone; with left and right; with left, right and down;
//! one on each axis (right, up, back); and all of them.

use monaka_channel::d3d::{create_device, create_texture, read_image, simple_texture_desc};
use monaka_overlay::eye::EyeView;
use monaka_overlay::scene::{ScenePass, files};
use std::path::PathBuf;
use windows::Win32::Graphics::Direct3D11::{D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM_SRGB;

const WIDTH: u32 = 480;
const HEIGHT: u32 = 360;
/// The time of day looked at: 15:00.
const TIME: u16 = 15 * 60;

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let folder = PathBuf::from(args.first().ok_or("give: <scene folder> [reach] [sets...]")?);
    let reach: f32 = args.get(1).and_then(|r| r.parse().ok()).unwrap_or(0.25);
    let sets: Vec<String> = if args.len() > 2 { args[2..].to_vec() } else { ["c", "c,l,r", "c,l,r,d", "c,r,u,b", "c,f,b,l,r,u,d"].map(String::from).to_vec() };
    let all = files::load(&folder, |_| vec![TIME])?;
    let (device, context, _) = create_device(None).map_err(|e| e.to_string())?;
    let mut desc = simple_texture_desc(WIDTH, HEIGHT, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB);
    desc.BindFlags = (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32;
    let image = create_texture(&device, &desc).map_err(|e| e.to_string())?;
    // Heads leaning every way: the six along the axes and the eight between them.
    let mut leans: Vec<[f32; 3]> = Vec::new();
    for x in [-1.0f32, 0.0, 1.0] {
        for y in [-1.0f32, 0.0, 1.0] {
            for z in [-1.0f32, 0.0, 1.0] {
                let count = [x, y, z].iter().filter(|v| **v != 0.0).count();
                if count == 1 || count == 3 {
                    let length = (x * x + y * y + z * z).sqrt();
                    leans.push([x, y, z].map(|v| v / length * reach));
                }
            }
        }
    }
    println!("{} leaning {reach} m every way ({} heads, 4 ways each), at {}:", all.name, leans.len(), monaka_overlay::scene::files::clock_text(TIME));
    for set in &sets {
        let wanted: Vec<&str> = set.split(',').collect();
        let mut loaded = files::load(&folder, |_| vec![TIME])?;
        let keep: Vec<bool> = loaded.places.iter().map(|p| wanted.contains(&p.name)).collect();
        let mut flags = keep.iter();
        loaded.places.retain(|_| *flags.next().unwrap_or(&false));
        for (_, images) in &mut loaded.colours {
            let mut flags = keep.iter();
            images.retain(|_| *flags.next().unwrap_or(&false));
        }
        if loaded.places.first().is_none_or(|p| p.name != "c") {
            println!("  {set}: needs the middle (c)");
            continue;
        }
        // The gaps as they are: no skirts drawn over them.
        loaded.places[0].skirts.clear();
        let mut pass = ScenePass::new(&device, &loaded)?;
        let (mut gaps, mut worst, mut views) = (0.0f64, 0.0f64, 0);
        for lean in &leans {
            for yaw in [0.0f32, 90.0, 180.0, 270.0] {
                let (y, p) = (yaw.to_radians() / 2.0, (-10.0f32).to_radians() / 2.0);
                let orientation = [p.sin() * y.cos(), y.sin() * p.cos(), -y.sin() * p.sin(), y.cos() * p.cos()];
                let half = 45f32.to_radians().tan();
                let tall = (half * HEIGHT as f32 / WIDTH as f32).atan();
                let eye = EyeView { orientation, position: *lean, fov: [-half.atan(), half.atan(), tall, -tall] };
                pass.draw(&device, &context, &image, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB, &eye, TIME, TIME, 0.0)?;
                let (_, _, pixels) = read_image(&device, &context, &image).map_err(|e| e.to_string())?;
                // What no capture covered shows the background: sRGB 12, 12, 16.
                let empty = pixels.as_chunks::<4>().0.iter().filter(|p| p[0] == 12 && p[1] == 12 && p[2] == 16).count() as f64 / (WIDTH * HEIGHT) as f64;
                gaps += empty;
                worst = worst.max(empty);
                views += 1;
            }
        }
        let tiles: usize = loaded.places.iter().map(|p| p.tiles.len()).sum();
        println!("  {set:>16}: {:.3}% of the view unseen on average, {:.2}% at worst ({} tiles)", gaps / f64::from(views) * 100.0, worst * 100.0, tiles);
    }
    Ok(())
}
