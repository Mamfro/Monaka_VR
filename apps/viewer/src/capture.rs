//! A capture of what the headset shows, asked for with Monaka's chord ([`crate::chord`]): the next
//! [`PAIRS`] pairs as they arrive, both eyes at full size, each with its pose record, into a new
//! `capture-N` folder of the run's `captures` folder (`--captures`). With frame warping the eyes
//! take turns being real, so consecutive pairs show each eye both real and warped.
//!
//! Each pair is copied on the GPU as it arrives; the copies are read back one image per frame
//! (a few milliseconds each: a short hitch), and written as JPEG on a thread of their own (an
//! uncompressed PNG of a headset eye is tens of megabytes).
//!
//! The first pair also goes into one image, the eyes side by side ([`BOTH`]), and that file onto
//! the clipboard ([`crate::clipboard`]), ready to paste.

use monaka_channel::d3d::{self, create_texture, read_image};
use monaka_core::protocol::{RenderedPair, dxgi_format};
use std::path::{Path, PathBuf};
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D};

/// Pairs per capture.
pub const PAIRS: usize = 6;
/// The images' JPEG quality (1-100).
const JPEG_QUALITY: u8 = 92;
/// The first pair side by side, left eye first: the image put on the clipboard.
const BOTH: &str = "pair-1-both.jpg";

struct Kept {
    eyes: [ID3D11Texture2D; 2],
    sequence: u64,
    record: Option<RenderedPair>,
}

pub struct Capture {
    folder: PathBuf,
    /// The capture under way: its folder, and the pairs still to keep.
    current: Option<(PathBuf, usize)>,
    kept: Vec<Kept>,
    /// Images read back so far of the kept pairs (two per pair).
    read: usize,
    /// The first pair's images as read back (width, height, format, texels), left first.
    first: Vec<(u32, u32, u32, Vec<u8>)>,
    lines: Vec<String>,
}

impl Capture {
    /// Captures go into `folder` (made when the first is asked for).
    pub fn new(folder: PathBuf) -> Self {
        Self { folder, current: None, kept: Vec::new(), read: 0, first: Vec::new(), lines: Vec::new() }
    }

    /// Asks for a capture of the next pairs; false while one is still under way.
    pub fn start(&mut self) -> bool {
        if self.current.is_some() || !self.kept.is_empty() {
            return false;
        }
        let number = (1..).find(|n| !self.folder.join(format!("capture-{n}")).exists()).unwrap_or(1);
        let folder = self.folder.join(format!("capture-{number}"));
        if let Err(e) = std::fs::create_dir_all(&folder) {
            println!("Capture: cannot make {}: {e}", folder.display());
            return false;
        }
        println!("Capture: the next {PAIRS} pairs to {}", folder.display());
        self.lines = vec![format!("{PAIRS} consecutive pairs as the headset showed them (left, right); pose records as the producer sent them")];
        self.current = Some((folder, PAIRS));
        true
    }

    /// A pair arrived (`eyes` hold it now): kept while a capture wants it.
    pub fn arrived(&mut self, device: &ID3D11Device, context: &ID3D11DeviceContext, eyes: [&ID3D11Texture2D; 2], sequence: u64, record: Option<&RenderedPair>) {
        let Some((_, wanted)) = self.current.as_mut().filter(|(_, wanted)| *wanted > 0) else { return };
        let copy = |eye: &ID3D11Texture2D| -> Option<ID3D11Texture2D> {
            let copy = create_texture(device, &d3d::private_copy_desc(&d3d::texture_desc(eye))).ok()?;
            d3d::copy_texture(context, &copy, eye);
            Some(copy)
        };
        let (Some(left), Some(right)) = (copy(eyes[0]), copy(eyes[1])) else {
            println!("Capture: a pair could not be copied; stopped");
            self.current = None;
            return;
        };
        self.kept.push(Kept { eyes: [left, right], sequence, record: record.copied() });
        *wanted -= 1;
    }

    /// Once a capture's pairs are all kept: reads one image back and hands it to a writer thread.
    pub fn step(&mut self, device: &ID3D11Device, context: &ID3D11DeviceContext) {
        let Some((folder, 0)) = self.current.clone() else { return };
        let Some(pair) = self.kept.get(self.read / 2) else {
            self.finish(&folder);
            return;
        };
        let (index, eye) = (self.read / 2, self.read % 2);
        if eye == 0 {
            self.lines.push(format!("pair {} (sequence {}): {}", index + 1, pair.sequence, describe(pair.record.as_ref())));
        }
        let texture = &pair.eyes[eye];
        let format = d3d::texture_desc(texture).Format.0 as u32;
        match read_image(device, context, texture) {
            Ok((width, height, texels)) => {
                let path = folder.join(format!("pair-{}-{}.jpg", index + 1, ["left", "right"][eye]));
                if index == 0 {
                    self.first.push((width, height, format, texels.clone()));
                    if self.first.len() == 2 {
                        let (eyes, both) = (std::mem::take(&mut self.first), folder.join(BOTH));
                        std::thread::spawn(move || copy_both(&both, &eyes));
                    }
                }
                std::thread::spawn(move || write_jpeg(&path, width, height, format, &texels));
            }
            Err(e) => println!("Capture: pair {} {} unreadable: {e}", index + 1, ["left", "right"][eye]),
        }
        self.read += 1;
    }

    fn finish(&mut self, folder: &Path) {
        let _ = std::fs::write(folder.join("pairs.txt"), self.lines.join("\n") + "\n");
        println!("Capture saved: {}", folder.display());
        (self.current, self.read) = (None, 0);
        self.kept.clear();
        self.first.clear();
    }
}

/// A pair's pose record in a line: each eye's position and orientation, or why there is none.
fn describe(record: Option<&RenderedPair>) -> String {
    let Some(record) = record else { return "no pose record (a flat screen)".into() };
    let eye = |e: usize| {
        let p = &record.eyes[e];
        format!("position {:?} orientation {:?}", p.position, p.orientation)
    };
    format!("left {}; right {}", eye(0), eye(1))
}

/// The texels as an RGB JPEG (the sRGB formats are stored as their plain ones). A warped eye's
/// alpha says where each pixel came from (`monaka_warp`'s `reproject.hlsl`): such an image also
/// gets `<name>-sources.jpg`, the real eye's pixels as they are, its own last frame's tinted green,
/// a neighbour's copy solid blue, nothing solid magenta.
fn write_jpeg(path: &Path, width: u32, height: u32, format: u32, texels: &[u8]) {
    let Some(pixels) = rgba(format, texels) else {
        println!("Capture: {} not written (image format {format})", path.display());
        return;
    };
    let save = |path: &Path, rgb: Vec<u8>| {
        if let Err(e) = save_jpeg(path, width, height, &rgb) {
            println!("Capture: cannot write {}: {e}", path.display());
        }
    };
    save(path, pixels.iter().flat_map(|[r, g, b, _]| [*r, *g, *b]).collect());
    if pixels.iter().all(|p| p[3] == 255) {
        return;
    }
    let sources = pixels
        .iter()
        .flat_map(|&[r, g, b, a]| match a {
            213.. => [r, g, b],
            128..=212 => [(r as u16 * 2 / 5) as u8, (g as u16 * 2 / 5 + 153) as u8, (b as u16 * 2 / 5) as u8],
            43..=127 => [0, 0, 255],
            _ => [255, 0, 255],
        })
        .collect();
    save(&path.with_file_name(format!("{}-sources.jpg", path.file_stem().unwrap_or_default().to_string_lossy())), sources);
}

/// The texels as RGBA (the sRGB formats are stored as their plain ones); None for a format with
/// no 8-bit reading.
fn rgba(format: u32, texels: &[u8]) -> Option<Vec<[u8; 4]>> {
    use dxgi_format::*;
    let plain = match format {
        R8G8B8A8_UNORM_SRGB => R8G8B8A8_UNORM,
        B8G8R8A8_UNORM_SRGB => B8G8R8A8_UNORM,
        other => other,
    };
    texels.as_chunks::<4>().0.iter().map(|t| monaka_core::bmp::dxgi_rgba8(plain, *t)).collect()
}

fn save_jpeg(path: &Path, width: u32, height: u32, rgb: &[u8]) -> Result<(), String> {
    // Full-resolution colour (no chroma subsampling): captures are cropped and judged up close.
    let mut encoder = jpeg_encoder::Encoder::new_file(path, JPEG_QUALITY).map_err(|e| e.to_string())?;
    encoder.set_sampling_factor(jpeg_encoder::SamplingFactor::R_4_4_4);
    encoder.encode(rgb, width as u16, height as u16, jpeg_encoder::ColorType::Rgb).map_err(|e| e.to_string())
}

/// Two eyes (width, height, format, texels) side by side as RGB, left first, the shorter one's
/// rows below it black: the image's width, height and pixels.
fn side_by_side(eyes: &[(u32, u32, u32, Vec<u8>)]) -> Option<(u32, u32, Vec<u8>)> {
    let [(lw, lh, lf, lt), (rw, rh, rf, rt)] = eyes else { return None };
    let (left, right) = (rgba(*lf, lt)?, rgba(*rf, rt)?);
    let (width, height) = (lw + rw, (*lh).max(*rh));
    let mut rgb = Vec::with_capacity((width * height * 3) as usize);
    for y in 0..height {
        for (pixels, w, h) in [(&left, *lw, *lh), (&right, *rw, *rh)] {
            for x in 0..w {
                let [r, g, b, _] = if y < h { pixels[(y * w + x) as usize] } else { [0; 4] };
                rgb.extend_from_slice(&[r, g, b]);
            }
        }
    }
    Some((width, height, rgb))
}

/// The first pair side by side into `path`, and that file onto the clipboard.
fn copy_both(path: &Path, eyes: &[(u32, u32, u32, Vec<u8>)]) {
    let Some((width, height, rgb)) = side_by_side(eyes) else {
        println!("Capture: {} not written (image format)", path.display());
        return;
    };
    if let Err(e) = save_jpeg(path, width, height, &rgb) {
        println!("Capture: cannot write {}: {e}", path.display());
        return;
    }
    match crate::clipboard::copy_file(path) {
        Ok(()) => println!("Capture: {} copied to the clipboard", path.display()),
        Err(e) => println!("Capture: {} not copied to the clipboard: {e}", path.display()),
    }
}

#[cfg(test)]
mod tests;
