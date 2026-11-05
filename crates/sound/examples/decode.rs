//! Decodes an Ogg Vorbis file as a room's sound would be, and says what it holds and how loud it
//! is (nothing plays).
//!
//! ```text
//! cargo run -p monaka_sound --example decode -- <file.ogg>
//! ```

use monaka_sound::Hearing;

fn main() -> Result<(), String> {
    let path = std::env::args().nth(1).ok_or("give an .ogg file")?;
    let clip = monaka_sound::decode(std::path::Path::new(&path))?;
    let level = |clip: &monaka_sound::Clip| {
        let peak = clip.samples.iter().map(|s| i32::from(*s).abs()).max().unwrap_or(0);
        let rms = (clip.samples.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / clip.samples.len() as f64).sqrt();
        let db = |v: f64| 20.0 * (v / 32768.0).log10();
        format!("peak {:.1} dBFS, average (RMS) {:.1} dBFS", db(f64::from(peak)), db(rms))
    };
    println!("{}", level(&clip));
    // How long the binaural mix takes, turning, for ten seconds of sound (nothing plays).
    let mut mixer = monaka_sound::mix::Mixer::new(clip.rate);
    let mut block = vec![0.0f32; 2 * 441];
    let started = std::time::Instant::now();
    for n in 0..1000 {
        let turn = n as f32 * 0.01;
        let heard = [[-1.0f32, 0.0, 0.0], [1.0, 0.0, 0.0]].map(|[x, y, z]| Hearing { direction: [x * turn.cos() - z * turn.sin(), y, x * turn.sin() + z * turn.cos()], gain: 1.0 });
        mixer.render(&clip.samples, usize::from(clip.channels), heard, &mut block);
    }
    println!("mixing 10 s of sound, turning, took {} ms", started.elapsed().as_millis());
    let started = std::time::Instant::now();
    let still = [Hearing { direction: [-1.0, 0.0, 0.0], gain: 1.0 }, Hearing { direction: [1.0, 0.0, 0.0], gain: 1.0 }];
    for _ in 0..1000 {
        mixer.render(&clip.samples, usize::from(clip.channels), still, &mut block);
    }
    println!("mixing 10 s of sound, still, took {} ms", started.elapsed().as_millis());
    let frames = clip.samples.len() / usize::from(clip.channels);
    println!("{path}: {} channels, {} Hz, {:.1} s", clip.channels, clip.rate, frames as f64 / f64::from(clip.rate));
    Ok(())
}
