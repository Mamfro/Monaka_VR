//! Draws a home scene as an eye would see it, into a PNG: to look at a scene's captures without a
//! headset.
//!
//! ```text
//! cargo run --release -p monaka_overlay --example scene_preview -- <scene folder> <time> <out.png> [x y z yaw pitch [fov]]
//! ```
//!
//! The eye is at `x y z` (metres from the middle capture; x right, y up, z back), turned `yaw`
//! degrees left and `pitch` up, seeing `fov` degrees across (default 100).

use monaka_channel::d3d::{create_device, create_texture, read_image, simple_texture_desc};
use monaka_overlay::eye::EyeView;
use monaka_overlay::scene::{ScenePass, files};
use std::path::PathBuf;
use std::time::Instant;
use windows::Win32::Graphics::Direct3D11::{D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM_SRGB;

const WIDTH: u32 = 1200;
const HEIGHT: u32 = 900;

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [folder, hour, out, rest @ ..] = args.as_slice() else {
        return Err("give: <scene folder> <time> <out.png> [x y z yaw pitch [fov]]".into());
    };
    let number = |i: usize, default: f32| rest.get(i).map_or(Ok(default), |v| v.parse::<f32>().map_err(|_| format!("{v} is no number")));
    let (position, yaw, pitch, fov) = ([number(0, 0.0)?, number(1, 0.0)?, number(2, 0.0)?], number(3, 0.0)?, number(4, 0.0)?, number(5, 100.0)?);
    // `15` or `15:30`.
    let (h, m) = hour.split_once(':').unwrap_or((hour, "0"));
    let hour: u16 = h.parse::<u16>().ok().zip(m.parse::<u16>().ok()).filter(|(h, m)| *h < 24 && *m < 60).map(|(h, m)| h * 60 + m).ok_or("the time is 0:00 to 23:59")?;
    let started = Instant::now();
    let mut loaded = files::load(&PathBuf::from(folder), |_| vec![hour])?;
    // `SCENE_PLACES=c,l`: only those capture points (to see what each fills).
    if let Ok(only) = std::env::var("SCENE_PLACES") {
        let keep: Vec<bool> = loaded.places.iter().map(|p| only.split(',').any(|n| n == p.name)).collect();
        let mut flags = keep.iter();
        loaded.places.retain(|_| *flags.next().unwrap_or(&false));
        for (_, images) in &mut loaded.colours {
            let mut flags = keep.iter();
            images.retain(|_| *flags.next().unwrap_or(&false));
        }
    }
    println!("{}: {} pixel faces, {} times of day, read in {} ms", loaded.name, loaded.face, loaded.times.len(), started.elapsed().as_millis());
    println!("  walls: {:.1} m left, {:.1} m right", loaded.sides[0], loaded.sides[1]);
    for place in &loaded.places {
        println!("  {}: {} tiles", place.name, place.tiles.len());
    }
    let (device, context, _) = create_device(None).map_err(|e| e.to_string())?;
    let mut pass = ScenePass::new(&device, &loaded)?;
    let mut desc = simple_texture_desc(WIDTH, HEIGHT, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB);
    desc.BindFlags = (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32;
    let image = create_texture(&device, &desc).map_err(|e| e.to_string())?;
    // Turned about y (yaw), then about the turned x (pitch).
    let (y, p) = (yaw.to_radians() / 2.0, pitch.to_radians() / 2.0);
    let orientation = [p.sin() * y.cos(), y.sin() * p.cos(), -y.sin() * p.sin(), y.cos() * p.cos()];
    let half = (fov.to_radians() / 2.0).tan();
    let tall = (half * HEIGHT as f32 / WIDTH as f32).atan();
    let eye = EyeView { orientation, position, fov: [-half.atan(), half.atan(), tall, -tall] };
    if !pass.draw(&device, &context, &image, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB, &eye, hour, hour, 0.0)? {
        return Err(format!("{} was not captured", files::clock_text(hour)));
    }
    let (width, height, pixels) = read_image(&device, &context, &image).map_err(|e| e.to_string())?;
    let file = std::fs::File::create(out).map_err(|e| format!("{out}: {e}"))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header().and_then(|mut w| w.write_image_data(&pixels)).map_err(|e| format!("{out}: {e}"))?;
    println!("Wrote {out}");
    Ok(())
}
