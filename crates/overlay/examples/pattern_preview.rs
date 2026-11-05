//! Draws a made home background ([`monaka_overlay::pattern`]) as an eye would see it, into a PNG.
//!
//! ```text
//! cargo run --release -p monaka_overlay --example pattern_preview -- <drift> <seconds> <out.png> [x y z yaw pitch [fov]]
//! ```
//!
//! The eye is at `x y z` (metres from the middle; x right, y up, z back), turned `yaw` degrees
//! left and `pitch` up, seeing `fov` degrees across (default 100).

use monaka_channel::d3d::{create_device, create_texture, read_image, simple_texture_desc};
use monaka_overlay::eye::EyeView;
use monaka_overlay::pattern::{FACE, Pattern, PatternPass, room};
use monaka_overlay::scene::ScenePass;
use windows::Win32::Graphics::Direct3D11::{D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM_SRGB;

const WIDTH: u32 = 1200;
const HEIGHT: u32 = 900;

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [pattern, seconds, out, rest @ ..] = args.as_slice() else {
        return Err("give: <drift> <seconds> <out.png> [x y z yaw pitch [fov]]".into());
    };
    let pattern = match pattern.to_ascii_lowercase().as_str() {
        "drift" => Pattern::Drift,
        other => return Err(format!("{other}: the patterns are: drift")),
    };
    let seconds: f64 = seconds.parse().map_err(|_| "seconds is a number")?;
    let number = |i: usize, default: f32| rest.get(i).map_or(Ok(default), |v| v.parse::<f32>().map_err(|_| format!("{v} is no number")));
    let (position, yaw, pitch, fov) = ([number(0, 0.0)?, number(1, 0.0)?, number(2, 0.0)?], number(3, 0.0)?, number(4, 0.0)?, number(5, 100.0)?);
    let (device, context, _) = create_device(None).map_err(|e| e.to_string())?;
    let mut pass = ScenePass::new(&device, &room(pattern, FACE))?;
    let paint = PatternPass::new(&device, FACE)?;
    paint.draw(&context, pattern, seconds);
    pass.add_time_views(0, vec![paint.view()]);
    let mut desc = simple_texture_desc(WIDTH, HEIGHT, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB);
    desc.BindFlags = (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32;
    let image = create_texture(&device, &desc).map_err(|e| e.to_string())?;
    let (y, p) = (yaw.to_radians() / 2.0, pitch.to_radians() / 2.0);
    let orientation = [p.sin() * y.cos(), y.sin() * p.cos(), -y.sin() * p.sin(), y.cos() * p.cos()];
    let half = (fov.to_radians() / 2.0).tan();
    let tall = (half * HEIGHT as f32 / WIDTH as f32).atan();
    let eye = EyeView { orientation, position, fov: [-half.atan(), half.atan(), tall, -tall] };
    pass.draw(&device, &context, &image, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB, &eye, 0, 0, 0.0)?;
    let (width, height, pixels) = read_image(&device, &context, &image).map_err(|e| e.to_string())?;
    let file = std::fs::File::create(out).map_err(|e| format!("{out}: {e}"))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header().and_then(|mut w| w.write_image_data(&pixels)).map_err(|e| format!("{out}: {e}"))?;
    println!("Wrote {out}");
    Ok(())
}
