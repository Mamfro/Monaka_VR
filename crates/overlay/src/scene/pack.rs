//! A home scene packed into one file ([`EXTENSION`]): small to download, read straight into what
//! the GPU draws.
//!
//! Its capture points' images are JPEG, every colour kept at full resolution (the hard pixels show
//! any colour smeared across them). Each time of day captured is stored the cheapest way that
//! comes out as clean, once Monaka has dithered it, as a JPEG of the whole image would: as its
//! change from the time before (most of the night is nearly free), with whole copies of the few
//! 8 by 8 blocks the change could not carry cleanly (a shadow's moving edge: [`Frame`]), or whole.
//! The capture points beside the middle keep only the 8 by 8 blocks of pixels they are drawn with,
//! side by side in a small image. The depth is 16-bit (in steps even for near and far), deflated.
//! Which of a side point's pixels are drawn is worked out when packing, not when loading.
//!
//! The file: [`MAGIC`], then the face size (u32), the times (a count, then each: [`Minutes`], u16), the capture
//! points (a count, u8); each point: its name (one byte), its offset (3 f32), whether it keeps all
//! its blocks (u8); if not, its blocks and the pixels it draws (each a deflated list of deltas);
//! its depth (deflated); then each time, in order: its kind ([`WHOLE`] or [`CHANGE`]) and its JPEG;
//! a change then its patched blocks (a deflated list of deltas) and their JPEG.
//! After the points, the room's sound, if it has one (an Ogg Vorbis file; length 0 for none).
//! Lengths are u32; everything little-endian.

use super::files::{self, Loaded, Minutes, Place, SKY};
use std::path::Path;

/// A packed room's file extension.
pub const EXTENSION: &str = "monakaroom";
const MAGIC: &[u8; 8] = b"MKROOM02";

/// A time stored whole, or as its change from the time before.
pub const WHOLE: u8 = 0;
pub const CHANGE: u8 = 1;

const BLOCK: usize = 8;
/// A side point's blocks go this many to a row of its image.
const COLUMNS: usize = 64;
/// How a capture point's hours are stored: a whole hour's JPEG quality, the qualities a change may
/// be stored at (worst first), and how far a change may leave the dithered hour from the captured
/// one (the share of pixels more than one dither step off, and the most steps any is).
struct Grade {
    whole: u8,
    changes: [u8; 4],
    off_share: f64,
    worst_steps: i32,
    /// A block a change leaves worse than this is patched: any pixel more steps off, or more
    /// pixels than this more than one step off.
    block_steps: i32,
    block_off: usize,
}

/// The middle: as clean as a whole hour's JPEG q92 everywhere (which looks as captured).
const MIDDLE: Grade = Grade { whole: 92, changes: [70, 80, 88, 94], off_share: 0.007, worst_steps: 7, block_steps: 2, block_off: 2 };
/// The points beside it: only seen in the gaps a lean opens, for a moment, so plainer.
const BESIDE: Grade = Grade { whole: 72, changes: [55, 65, 75, 85], off_share: 0.03, worst_steps: 12, block_steps: 3, block_off: 6 };

/// The dither the hours are judged after (`scene.hlsl`): 32 steps, an 8 by 8 Bayer matrix.
const STEPS: f32 = 32.0;
const BAYER8: [f32; 64] = [
    0.0, 32.0, 8.0, 40.0, 2.0, 34.0, 10.0, 42.0, 48.0, 16.0, 56.0, 24.0, 50.0, 18.0, 58.0, 26.0, 12.0, 44.0, 4.0, 36.0, 14.0, 46.0, 6.0, 38.0, 60.0, 28.0, 52.0, 20.0, 62.0, 30.0, 54.0, 22.0, 3.0, 35.0, 11.0, 43.0, 1.0, 33.0, 9.0, 41.0, 51.0, 19.0, 59.0, 27.0,
    49.0, 17.0, 57.0, 25.0, 15.0, 47.0, 7.0, 39.0, 13.0, 45.0, 5.0, 37.0, 63.0, 31.0, 55.0, 23.0, 61.0, 29.0, 53.0, 21.0,
];

/// Whether `path` is a packed room.
pub fn is_pack(path: &Path) -> bool {
    path.is_file() && path.extension().is_some_and(|e| e.eq_ignore_ascii_case(EXTENSION))
}

/// A strip of six faces, `n` pixels each, in 8 by 8 blocks (the last row and column of blocks
/// reaching past it where it is not a whole number of them).
#[derive(Clone, Copy)]
struct Grid {
    n: usize,
    width: usize,
    height: usize,
    across: usize,
    down: usize,
}

impl Grid {
    fn new(face: u32) -> Self {
        let n = face as usize;
        Grid { n, width: n * 6, height: n, across: (n * 6).div_ceil(BLOCK), down: n.div_ceil(BLOCK) }
    }

    fn block_of(&self, pixel: usize) -> u32 {
        let (x, y) = (pixel % self.width, pixel / self.width);
        ((y / BLOCK) * self.across + x / BLOCK) as u32
    }

    /// The strip pixel at `(i, j)` of block `block`, if the block reaches it.
    fn pixel(&self, block: u32, i: usize, j: usize) -> Option<usize> {
        let (x, y) = ((block as usize % self.across) * BLOCK + i, (block as usize / self.across) * BLOCK + j);
        (x < self.width && y < self.height).then_some(y * self.width + x)
    }

    /// The pixel's Bayer threshold, as its face has it (the shader's: from each face's top left).
    fn threshold(&self, pixel: usize) -> f32 {
        let (x, y) = (pixel % self.width % self.n, pixel / self.width);
        (BAYER8[(y & 7) * 8 + (x & 7)] + 0.5) / 64.0
    }
}

/// How a point's blocks lie in its images: the strip's own layout when it keeps them all, else
/// [`COLUMNS`] to a row in order.
struct Layout {
    blocks: Vec<u32>,
    columns: usize,
}

impl Layout {
    fn new(grid: &Grid, blocks: Option<Vec<u32>>) -> Self {
        match blocks {
            Some(blocks) => Layout { blocks, columns: COLUMNS },
            None => Layout { blocks: (0..(grid.across * grid.down) as u32).collect(), columns: grid.across },
        }
    }

    fn size(&self) -> (usize, usize) {
        let rows = self.blocks.len().div_ceil(self.columns).max(1);
        (self.columns.min(self.blocks.len().max(1)) * BLOCK, rows * BLOCK)
    }

    /// Each image pixel's strip pixel (none: past the strip's edge, or a spare place).
    fn map(&self, grid: &Grid) -> Vec<Option<usize>> {
        let (width, height) = self.size();
        let mut map = vec![None; width * height];
        for (k, &block) in self.blocks.iter().enumerate() {
            let (cx, cy) = ((k % self.columns) * BLOCK, (k / self.columns) * BLOCK);
            for j in 0..BLOCK {
                for i in 0..BLOCK {
                    map[(cy + j) * width + cx + i] = grid.pixel(block, i, j);
                }
            }
        }
        map
    }
}

/// The image of `rgba` (a strip) that `map` lays out, RGB; past the strip's edge, its nearest
/// pixel's colour carries on (so the blocks there stay smooth).
fn gather(rgba: &[u8], map: &[Option<usize>]) -> Vec<u8> {
    let mut last = 0;
    map.iter()
        .flat_map(|at| {
            if let Some(p) = at {
                last = *p;
            }
            [rgba[last * 4], rgba[last * 4 + 1], rgba[last * 4 + 2]]
        })
        .collect()
}

/// Each block's pixels that are never drawn take the mean of its drawn ones: JPEG spends nothing on
/// them then.
fn smooth_undrawn(image: &mut [u8], width: usize, map: &[Option<usize>], drawn: &[bool]) {
    if drawn.iter().all(|d| *d) {
        return;
    }
    let rows = map.len() / width;
    for by in (0..rows).step_by(BLOCK) {
        for bx in (0..width).step_by(BLOCK) {
            let pixels: Vec<usize> = (0..BLOCK * BLOCK).map(|k| (by + k / BLOCK) * width + bx + k % BLOCK).filter(|&k| k < map.len()).collect();
            let used: Vec<usize> = pixels.iter().copied().filter(|&k| map[k].is_some_and(|p| drawn[p])).collect();
            if used.is_empty() || used.len() == pixels.len() {
                continue;
            }
            let mean = [0, 1, 2].map(|c| (used.iter().map(|&k| u32::from(image[k * 3 + c])).sum::<u32>() / used.len() as u32) as u8);
            for &k in &pixels {
                if !map[k].is_some_and(|p| drawn[p]) {
                    image[k * 3..k * 3 + 3].copy_from_slice(&mean);
                }
            }
        }
    }
}

/// Puts an image's pixels (RGB) back into a strip (RGBA) where `map` says.
fn scatter(rgb: &[u8], map: &[Option<usize>], strip: &mut [u8]) {
    for (k, at) in map.iter().enumerate() {
        if let Some(p) = at {
            strip[p * 4..p * 4 + 3].copy_from_slice(&rgb[k * 3..k * 3 + 3]);
            strip[p * 4 + 3] = 255;
        }
    }
}

fn encode_jpeg(rgb: &[u8], width: usize, height: usize, quality: u8) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut encoder = jpeg_encoder::Encoder::new(&mut out, quality);
    encoder.set_sampling_factor(jpeg_encoder::SamplingFactor::R_4_4_4);
    encoder.set_optimized_huffman_tables(true);
    encoder.encode(rgb, width as u16, height as u16, jpeg_encoder::ColorType::Rgb).map_err(|e| format!("JPEG: {e}"))?;
    Ok(out)
}

fn decode_jpeg(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let options = zune_core::options::DecoderOptions::default().jpeg_set_out_colorspace(zune_core::colorspace::ColorSpace::RGB);
    zune_jpeg::JpegDecoder::new_with_options(zune_core::bytestream::ZCursor::new(bytes), options).decode().map_err(|e| format!("JPEG: {e:?}"))
}

/// The hour that `change` (an image of an hour's change, around mid-grey) makes of `before`.
fn apply_change(before: &[u8], change: &[u8]) -> Vec<u8> {
    before.iter().zip(change).map(|(&b, &c)| (i16::from(b) + i16::from(c) - 128).clamp(0, 255) as u8).collect()
}

use files::{distance_code as depth_code, distance_of as depth_of};

/// A sorted list as deflated deltas (each a variable-length number).
fn pack_list(values: &[u32]) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut last = 0u32;
    for &v in values {
        let mut delta = v - last;
        last = v;
        loop {
            let low = (delta & 0x7f) as u8;
            delta >>= 7;
            bytes.push(if delta == 0 { low } else { low | 0x80 });
            if delta == 0 {
                break;
            }
        }
    }
    miniz_oxide::deflate::compress_to_vec(&bytes, 10)
}

fn unpack_list(packed: &[u8]) -> Result<Vec<u32>, String> {
    let bytes = miniz_oxide::inflate::decompress_to_vec(packed).map_err(|e| format!("a list: {e:?}"))?;
    let (mut values, mut last, mut delta, mut shift) = (Vec::new(), 0u32, 0u32, 0);
    for b in bytes {
        delta |= u32::from(b & 0x7f) << shift;
        if b & 0x80 == 0 {
            last += delta;
            values.push(last);
            (delta, shift) = (0, 0);
        } else {
            shift += 7;
        }
    }
    Ok(values)
}

/// The depth of `blocks` (each pixel of each, in order) as 16-bit codes, each from the one before,
/// low bytes then high, deflated.
fn pack_depth(grid: &Grid, depth: &[f32], blocks: &[u32]) -> Vec<u8> {
    let codes: Vec<u16> = blocks.iter().flat_map(|&b| (0..BLOCK * BLOCK).map(move |k| grid.pixel(b, k % BLOCK, k / BLOCK).map_or(0, |p| depth_code(depth[p])))).collect();
    let mut last = 0u16;
    let deltas: Vec<u16> = codes
        .iter()
        .map(|&c| {
            let d = c.wrapping_sub(last);
            last = c;
            d
        })
        .collect();
    let bytes: Vec<u8> = deltas.iter().map(|d| (d & 0xff) as u8).chain(deltas.iter().map(|d| (d >> 8) as u8)).collect();
    miniz_oxide::deflate::compress_to_vec(&bytes, 10)
}

/// A strip's depth from [`pack_depth`]'s blocks: unknown where no block held it (as sky).
fn unpack_depth(grid: &Grid, packed: &[u8], blocks: &[u32]) -> Result<Vec<f32>, String> {
    let bytes = miniz_oxide::inflate::decompress_to_vec(packed).map_err(|e| format!("depth: {e:?}"))?;
    let count = blocks.len() * BLOCK * BLOCK;
    if bytes.len() != count * 2 {
        return Err("depth: not as many as its blocks hold".into());
    }
    let mut depth = vec![SKY; grid.width * grid.height];
    let mut last = 0u16;
    for (k, block) in blocks.iter().enumerate() {
        for j in 0..BLOCK * BLOCK {
            let at = k * BLOCK * BLOCK + j;
            last = last.wrapping_add(u16::from(bytes[at]) | u16::from(bytes[count + at]) << 8);
            if let Some(p) = grid.pixel(*block, j % BLOCK, j / BLOCK) {
                depth[p] = depth_of(last);
            }
        }
    }
    Ok(depth)
}

/// The pixels drawn and those round them, in order: a pixel's tile corners read its neighbours.
fn with_ring(grid: &Grid, tiles: &[u32]) -> Vec<u32> {
    let mut near = vec![false; grid.width * grid.height];
    for &t in tiles {
        let (x, y) = ((t as usize % grid.width) as isize, (t as usize / grid.width) as isize);
        for dy in -1..=1 {
            for dx in -1..=1 {
                let (nx, ny) = (x + dx, y + dy);
                if nx >= 0 && ny >= 0 && (nx as usize) < grid.width && (ny as usize) < grid.height {
                    near[ny as usize * grid.width + nx as usize] = true;
                }
            }
        }
    }
    (0..near.len() as u32).filter(|p| near[*p as usize]).collect()
}

/// The depth at `pixels` as 16-bit codes, each from the one before, low bytes then high, deflated.
fn pack_depth_at(depth: &[f32], pixels: &[u32]) -> Vec<u8> {
    let mut last = 0u16;
    let deltas: Vec<u16> = pixels
        .iter()
        .map(|&p| {
            let c = depth_code(depth[p as usize]);
            let d = c.wrapping_sub(last);
            last = c;
            d
        })
        .collect();
    let bytes: Vec<u8> = deltas.iter().map(|d| (d & 0xff) as u8).chain(deltas.iter().map(|d| (d >> 8) as u8)).collect();
    miniz_oxide::deflate::compress_to_vec(&bytes, 10)
}

/// A strip's depth from [`pack_depth_at`]'s `pixels`: unknown elsewhere (as sky).
fn unpack_depth_at(grid: &Grid, packed: &[u8], pixels: &[u32]) -> Result<Vec<f32>, String> {
    let bytes = miniz_oxide::inflate::decompress_to_vec(packed).map_err(|e| format!("depth: {e:?}"))?;
    if bytes.len() != pixels.len() * 2 {
        return Err("depth: not as many as its pixels".into());
    }
    let mut depth = vec![SKY; grid.width * grid.height];
    let mut last = 0u16;
    for (k, &p) in pixels.iter().enumerate() {
        last = last.wrapping_add(u16::from(bytes[k]) | u16::from(bytes[pixels.len() + k]) << 8);
        depth[p as usize] = depth_of(last);
    }
    Ok(depth)
}

/// How far `made` (RGB, as `map` lays it out) is from `captured` (RGBA strip) once dithered, over the
/// strip pixels `drawn` says: the share more than a step off, and the most steps.
fn dither_error(grid: &Grid, captured: &[u8], made: &[u8], map: &[Option<usize>], drawn: &[bool]) -> (f64, i32) {
    let (mut off, mut worst, mut count) = (0usize, 0i32, 0usize);
    let step = |v: u8, t: f32| (f32::from(v) / 255.0 * STEPS + t).floor() as i32;
    for (k, at) in map.iter().enumerate() {
        let Some(p) = *at else { continue };
        if !drawn[p] {
            continue;
        }
        count += 1;
        let t = grid.threshold(p);
        let far = (0..3).map(|c| (step(captured[p * 4 + c], t) - step(made[k * 3 + c], t)).abs()).max().unwrap_or(0);
        off += usize::from(far > 1);
        worst = worst.max(far);
    }
    (off as f64 / count.max(1) as f64, worst)
}

/// One time of day's image as stored: whole, or as its change from the time before with the blocks
/// it could not carry patched over (their places in the image, in order, and their JPEG).
struct Frame {
    kind: u8,
    jpeg: Vec<u8>,
    patches: Vec<u32>,
    patch_jpeg: Vec<u8>,
}

impl Frame {
    fn bytes(&self) -> usize {
        self.jpeg.len() + self.patch_jpeg.len() + if self.patches.is_empty() { 0 } else { pack_list(&self.patches).len() }
    }
}

/// The image pixels of block `k` of an image `columns` blocks and `width` pixels across.
fn block_pixels(k: usize, columns: usize, width: usize) -> impl Iterator<Item = usize> {
    (0..BLOCK * BLOCK).map(move |i| ((k / columns) * BLOCK + i / BLOCK) * width + (k % columns) * BLOCK + i % BLOCK)
}

/// Blocks `list` of `image` (`columns` blocks, `width` pixels across), side by side [`COLUMNS`] to a
/// row: the patch image, and its size.
fn take_blocks(image: &[u8], width: usize, columns: usize, list: &[u32]) -> (Vec<u8>, usize, usize) {
    let (pw, ph) = (COLUMNS.min(list.len().max(1)) * BLOCK, list.len().div_ceil(COLUMNS).max(1) * BLOCK);
    let mut patch = vec![0u8; pw * ph * 3];
    for (n, &k) in list.iter().enumerate() {
        for (to, from) in block_pixels(n, COLUMNS, pw).zip(block_pixels(k as usize, columns, width)) {
            patch[to * 3..to * 3 + 3].copy_from_slice(&image[from * 3..from * 3 + 3]);
        }
    }
    (patch, pw, ph)
}

/// Puts a patch image's blocks (as [`take_blocks`] laid them) back over `image` at `list`.
fn put_blocks(image: &mut [u8], width: usize, columns: usize, list: &[u32], patch: &[u8]) {
    let pw = COLUMNS.min(list.len().max(1)) * BLOCK;
    for (n, &k) in list.iter().enumerate() {
        for (from, to) in block_pixels(n, COLUMNS, pw).zip(block_pixels(k as usize, columns, width)) {
            image[to * 3..to * 3 + 3].copy_from_slice(&patch[from * 3..from * 3 + 3]);
        }
    }
}

/// The blocks of an image (laid out as `layout`) that `made` leaves worse than `grade` allows next
/// to `captured` (a strip) once dithered.
fn worse_blocks(grid: &Grid, layout: &Layout, map: &[Option<usize>], captured: &[u8], made: &[u8], drawn: &[bool], grade: &Grade) -> Vec<u32> {
    let (width, _) = layout.size();
    let step = |v: u8, t: f32| (f32::from(v) / 255.0 * STEPS + t).floor() as i32;
    (0..layout.blocks.len() as u32)
        .filter(|&k| {
            let (mut off, mut worst) = (0, 0);
            for i in block_pixels(k as usize, layout.columns, width) {
                let Some(p) = map[i].filter(|p| drawn[*p]) else { continue };
                let t = grid.threshold(p);
                let far = (0..3).map(|c| (step(captured[p * 4 + c], t) - step(made[i * 3 + c], t)).abs()).max().unwrap_or(0);
                off += usize::from(far > 1);
                worst = worst.max(far);
            }
            worst > grade.block_steps || off > grade.block_off
        })
        .collect()
}

/// One capture point's times of day, each stored as the module says; the images come from
/// `image_at` (RGBA strips).
fn pack_times(grid: &Grid, layout: &Layout, drawn: &[bool], grade: &Grade, times: &[Minutes], image_at: impl Fn(Minutes) -> Result<Vec<u8>, String>) -> Result<Vec<Frame>, String> {
    let map = layout.map(grid);
    let (width, height) = layout.size();
    let mut before: Option<Vec<u8>> = None;
    let mut frames = Vec::new();
    for &t in times {
        let captured = image_at(t)?;
        let mut image = gather(&captured, &map);
        smooth_undrawn(&mut image, width, &map, drawn);
        // Whole, unless a change (with its worst blocks patched) comes out smaller and as clean.
        let whole = encode_jpeg(&image, width, height, grade.whole)?;
        let mut best = (Frame { kind: WHOLE, jpeg: whole.clone(), patches: Vec::new(), patch_jpeg: Vec::new() }, decode_jpeg(&whole)?);
        if let Some(before) = &before {
            let change: Vec<u8> = image.iter().zip(before).map(|(&c, &b)| (i16::from(c) - i16::from(b) + 128).clamp(0, 255) as u8).collect();
            for quality in grade.changes {
                let jpeg = encode_jpeg(&change, width, height, quality)?;
                let mut made = apply_change(before, &decode_jpeg(&jpeg)?);
                // The change alone, if it is clean enough as it is.
                let (off, worst) = dither_error(grid, &captured, &made, &map, drawn);
                if off <= grade.off_share && worst <= grade.worst_steps && jpeg.len() < best.0.bytes() {
                    best = (Frame { kind: CHANGE, jpeg: jpeg.clone(), patches: Vec::new(), patch_jpeg: Vec::new() }, made.clone());
                }
                let patches = worse_blocks(grid, layout, &map, &captured, &made, drawn, grade);
                if patches.len() * 2 > layout.blocks.len() {
                    continue;
                }
                let mut patch_jpeg = Vec::new();
                if !patches.is_empty() {
                    let (patch, pw, ph) = take_blocks(&image, width, layout.columns, &patches);
                    patch_jpeg = encode_jpeg(&patch, pw, ph, grade.whole)?;
                    put_blocks(&mut made, width, layout.columns, &patches, &decode_jpeg(&patch_jpeg)?);
                }
                let frame = Frame { kind: CHANGE, jpeg, patches, patch_jpeg };
                let (off, worst) = dither_error(grid, &captured, &made, &map, drawn);
                if off <= grade.off_share && worst <= grade.worst_steps && frame.bytes() < best.0.bytes() {
                    best = (frame, made);
                }
            }
        }
        frames.push(best.0);
        before = Some(best.1);
    }
    Ok(frames)
}

/// What packing a room came to, for the one who packed it.
pub struct Report {
    /// Each capture point: its name, its images' bytes, its depth's and lists' bytes, each hour's
    /// kind.
    pub places: Vec<(&'static str, usize, usize, Vec<u8>)>,
    pub sound: usize,
    pub total: usize,
}

/// Packs the captured room in `folder` (as [`files`] reads it), with its sound if it
/// has one: the file's bytes, and what they came to.
pub fn pack_folder(folder: &Path) -> Result<(Vec<u8>, Report), String> {
    let present: Vec<(&'static str, [f32; 3])> = files::PLACES.into_iter().filter(|(place, _)| folder.join(place).join("depth.exr").is_file()).collect();
    if present.first().is_none_or(|(name, _)| *name != "c") {
        return Err(format!("{}: no middle capture (c/depth.exr)", folder.display()));
    }
    let mut depths = Vec::new();
    let mut face = 0;
    for (place, _) in &present {
        let (size, depth) = files::read_depth(&folder.join(place).join("depth.exr"))?;
        if face != 0 && size != face {
            return Err(format!("{place}/depth.exr has {size}-pixel faces, not {face}"));
        }
        face = size;
        depths.push(depth);
    }
    let names: Vec<&str> = present.iter().map(|(p, _)| *p).collect();
    let times = files::times(folder, &names);
    if times.is_empty() {
        return Err("no time of day has an image at every capture point".into());
    }
    let grid = Grid::new(face);
    // Each point packed on its own thread: the middle's hours are most of the work.
    let packed: Vec<Result<PackedBytes, String>> = std::thread::scope(|scope| {
        let jobs: Vec<_> = present
            .iter()
            .enumerate()
            .map(|(index, (place, offset))| {
                let (depths, times, grid) = (&depths, &times, &grid);
                scope.spawn(move || -> Result<PackedBytes, String> {
                    let depth = &depths[index];
                    let all = index == 0;
                    let tiles: Vec<u32> = if all { Vec::new() } else { files::unseen(&depths[0], depth, *offset, face) };
                    let mut drawn = vec![all; depth.len()];
                    for &t in &tiles {
                        drawn[t as usize] = true;
                    }
                    let mut blocks: Vec<u32> = tiles.iter().map(|&t| grid.block_of(t as usize)).collect();
                    blocks.dedup();
                    blocks.sort_unstable();
                    blocks.dedup();
                    let layout = Layout::new(grid, (!all).then(|| blocks.clone()));

                    let mut out = Vec::new();
                    out.push(place.as_bytes()[0]);
                    offset.iter().for_each(|v| out.extend(v.to_le_bytes()));
                    out.push(u8::from(all));
                    let mut lists = 0;
                    if !all {
                        for list in [pack_list(&blocks), pack_list(&tiles)] {
                            lists += list.len();
                            section(&mut out, &list);
                        }
                    }
                    let depth_bytes = if all { pack_depth(grid, depth, &layout.blocks) } else { pack_depth_at(depth, &with_ring(grid, &tiles)) };
                    lists += depth_bytes.len();
                    section(&mut out, &depth_bytes);
                    let frames = pack_times(grid, &layout, &drawn, if all { &MIDDLE } else { &BESIDE }, times, |t| {
                        let path = files::colour_path(folder, place, t);
                        let (w, hh, rgba) = files::read_colour(&path)?;
                        if (w as usize, hh as usize) != (grid.width, grid.height) {
                            return Err(format!("{} is {w}x{hh}, not {}x{}", path.display(), grid.width, grid.height));
                        }
                        Ok(rgba)
                    })?;
                    let mut images = 0;
                    let mut kinds = Vec::new();
                    for frame in frames {
                        out.push(frame.kind);
                        images += frame.bytes();
                        kinds.push(frame.kind);
                        section(&mut out, &frame.jpeg);
                        if frame.kind == CHANGE {
                            section(&mut out, &if frame.patches.is_empty() { Vec::new() } else { pack_list(&frame.patches) });
                            section(&mut out, &frame.patch_jpeg);
                        }
                    }
                    Ok((out, images, lists, kinds))
                })
            })
            .collect();
        jobs.into_iter().map(|job| job.join().unwrap_or_else(|_| Err("a packing thread failed".into()))).collect()
    });
    let mut file = Vec::new();
    file.extend(MAGIC);
    file.extend(face.to_le_bytes());
    file.extend((times.len() as u16).to_le_bytes());
    times.iter().for_each(|t| file.extend(t.to_le_bytes()));
    file.push(present.len() as u8);
    let mut report = Report { places: Vec::new(), sound: 0, total: 0 };
    for ((place, _), result) in present.iter().zip(packed) {
        let (bytes, images, lists, kinds) = result?;
        file.extend(bytes);
        report.places.push((place, images, lists, kinds));
    }
    let sound = std::fs::read(folder.join("ambience.ogg")).unwrap_or_default();
    report.sound = sound.len();
    section(&mut file, &sound);
    report.total = file.len();
    Ok((file, report))
}

/// A capture point packed: its bytes, its images' bytes, its depth's and lists' bytes, each hour's
/// kind.
type PackedBytes = (Vec<u8>, usize, usize, Vec<u8>);

fn section(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend((bytes.len() as u32).to_le_bytes());
    out.extend(bytes);
}

/// A packed room as read: its parts, not yet decoded.
struct Room {
    face: u32,
    times: Vec<Minutes>,
    places: Vec<PackedPlace>,
    sound: Vec<u8>,
}

struct PackedPlace {
    name: &'static str,
    offset: [f32; 3],
    /// The blocks it keeps (none: all), and the pixels drawn (none: all).
    blocks: Option<Vec<u32>>,
    tiles: Option<Vec<u32>>,
    depth: Vec<u8>,
    frames: Vec<Frame>,
}

fn read(path: &Path) -> Result<Room, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut at = 0usize;
    let mut take = |n: usize| -> Result<&[u8], String> {
        let part = bytes.get(at..at + n).ok_or("it ends early")?;
        at += n;
        Ok(part)
    };
    if take(8)? != MAGIC {
        return Err(format!("{}: not a packed room", path.display()));
    }
    let u32_of = |b: &[u8]| u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    let face = u32_of(take(4)?);
    let count = usize::from(u16::from_le_bytes(take(2)?.try_into().map_err(|_| "the times")?));
    let times: Vec<Minutes> = take(count * 2)?.as_chunks::<2>().0.iter().map(|t| u16::from_le_bytes(*t)).collect();
    let place_count = take(1)?[0];
    let mut places = Vec::new();
    for _ in 0..place_count {
        let letter = take(1)?[0];
        let name = files::PLACES.iter().map(|(n, _)| *n).find(|n| n.as_bytes()[0] == letter).ok_or("a capture point it does not know")?;
        let mut offset = [0.0; 3];
        for v in &mut offset {
            *v = f32::from_le_bytes(take(4)?.try_into().map_err(|_| "an offset")?);
        }
        let all = take(1)?[0] != 0;
        let mut section = || -> Result<Vec<u8>, String> {
            let n = u32_of(take(4)?) as usize;
            Ok(take(n)?.to_vec())
        };
        let (blocks, tiles) = if all { (None, None) } else { (Some(unpack_list(&section()?)?), Some(unpack_list(&section()?)?)) };
        let depth = section()?;
        let mut frames = Vec::new();
        for _ in 0..count {
            let kind = take(1)?[0];
            let mut section = || -> Result<Vec<u8>, String> {
                let n = u32_of(take(4)?) as usize;
                Ok(take(n)?.to_vec())
            };
            let jpeg = section()?;
            let (patches, patch_jpeg) = if kind == CHANGE {
                let list = section()?;
                (if list.is_empty() { Vec::new() } else { unpack_list(&list)? }, section()?)
            } else {
                (Vec::new(), Vec::new())
            };
            frames.push(Frame { kind, jpeg, patches, patch_jpeg });
        }
        places.push(PackedPlace { name, offset, blocks, tiles, depth, frames });
    }
    let n = u32_of(take(4)?) as usize;
    let sound = take(n)?.to_vec();
    Ok(Room { face, times, places, sound })
}

/// Reads the packed room at `path`, with the colours of `wanted` times (those it has).
pub fn load(path: &Path, wanted: impl Fn(&[Minutes]) -> Vec<Minutes>) -> Result<Loaded, String> {
    let room = read(path)?;
    let grid = Grid::new(room.face);
    let mut places = Vec::new();
    let mut centre_depth = Vec::new();
    for (index, place) in room.places.iter().enumerate() {
        // The middle's depth by its blocks; the others' round the pixels they draw.
        let depth = match &place.tiles {
            None => unpack_depth(&grid, &place.depth, &Layout::new(&grid, None).blocks)?,
            Some(tiles) => unpack_depth_at(&grid, &place.depth, &with_ring(&grid, tiles))?,
        };
        let tiles = place.tiles.clone().unwrap_or_else(|| (0..depth.len() as u32).collect());
        let skirts = if index == 0 { files::skirts(&depth, room.face) } else { Vec::new() };
        places.push(Place { name: place.name, offset: place.offset, corners: files::corners(&depth, room.face), tiles, skirts });
        if index == 0 {
            centre_depth = depth;
        }
    }
    let sides = [[-1.0, 0.0, 0.0], [1.0, 0.0, 0.0]].map(|dir| centre_depth.get(files::strip_pixel(room.face, dir)).copied().unwrap_or(SKY));
    let colours = wanted(&room.times).into_iter().map(|time| decode_time(&room, &grid, time).map(|images| (time, images))).collect::<Result<_, _>>()?;
    let name = path.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    Ok(Loaded { name, face: room.face, shades: files::SHADES, dither: files::DITHER, times: room.times, sides, places, colours })
}

/// The colour images of `time` in the packed room at `path`, one per place (RGBA strips).
pub fn load_time(path: &Path, time: Minutes) -> Result<Vec<Vec<u8>>, String> {
    let room = read(path)?;
    decode_time(&room, &Grid::new(room.face), time)
}

/// The room's sound (an Ogg Vorbis file), if it has one.
pub fn sound(path: &Path) -> Result<Option<Vec<u8>>, String> {
    let room = read(path)?;
    Ok((!room.sound.is_empty()).then_some(room.sound))
}

/// Each place's strip at `time`: from the last whole one at or before it, its changes added on.
fn decode_time(room: &Room, grid: &Grid, time: Minutes) -> Result<Vec<Vec<u8>>, String> {
    let index = room.times.iter().position(|t| *t == time).ok_or_else(|| format!("no capture at {}", files::clock_text(time)))?;
    room.places
        .iter()
        .map(|place| {
            let first = (0..=index).rev().find(|&i| place.frames[i].kind == WHOLE).ok_or("a time with no whole one before it")?;
            let layout = Layout::new(grid, place.blocks.clone());
            let (width, _) = layout.size();
            let mut image = decode_jpeg(&place.frames[first].jpeg)?;
            for frame in &place.frames[first + 1..=index] {
                image = apply_change(&image, &decode_jpeg(&frame.jpeg)?);
                if !frame.patches.is_empty() {
                    put_blocks(&mut image, width, layout.columns, &frame.patches, &decode_jpeg(&frame.patch_jpeg)?);
                }
            }
            let mut strip = vec![0u8; grid.width * grid.height * 4];
            scatter(&image, &layout.map(grid), &mut strip);
            Ok(strip)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_and_depths_come_back_as_they_went() {
        let values = vec![0, 3, 4, 200, 70_000, 70_001];
        assert_eq!(unpack_list(&pack_list(&values)).unwrap(), values);
        let grid = Grid::new(12);
        let depth: Vec<f32> = (0..grid.width * grid.height).map(|p| 0.5 + (p % 37) as f32 * 0.25).collect();
        let blocks = Layout::new(&grid, None).blocks;
        let back = unpack_depth(&grid, &pack_depth(&grid, &depth, &blocks), &blocks).unwrap();
        assert!(depth.iter().zip(&back).all(|(a, b)| (a - b).abs() / a < 3e-4), "16-bit steps are fine enough");
        assert_eq!(depth_of(0), SKY, "unknown is sky");
    }

    #[test]
    fn patched_blocks_land_where_they_were_taken_from() {
        // An image 3 blocks across and 2 down; blocks 1 and 4 patched.
        let (width, columns) = (3 * BLOCK, 3);
        let image: Vec<u8> = (0..width * 2 * BLOCK * 3).map(|v| (v % 251) as u8).collect();
        let (patch, _, _) = take_blocks(&image, width, columns, &[1, 4]);
        let mut back = vec![0u8; image.len()];
        put_blocks(&mut back, width, columns, &[1, 4], &patch);
        for k in [1usize, 4] {
            for p in block_pixels(k, columns, width) {
                assert_eq!(back[p * 3..p * 3 + 3], image[p * 3..p * 3 + 3], "block {k}");
            }
        }
        assert!(block_pixels(0, columns, width).all(|p| back[p * 3] == 0), "the others left alone");
    }

    #[test]
    fn a_side_points_depth_is_kept_round_the_pixels_it_draws() {
        let grid = Grid::new(16);
        let depth: Vec<f32> = (0..grid.width * grid.height).map(|p| 1.0 + p as f32 * 0.01).collect();
        let tiles = [5 * grid.width as u32 + 7];
        let ring = with_ring(&grid, &tiles);
        assert_eq!(ring.len(), 9, "the pixel and the eight round it");
        let back = unpack_depth_at(&grid, &pack_depth_at(&depth, &ring), &ring).unwrap();
        for &p in &ring {
            assert!((back[p as usize] - depth[p as usize]).abs() / depth[p as usize] < 3e-4);
        }
        assert_eq!(back[0], SKY, "elsewhere unknown");
    }

    #[test]
    fn a_side_points_blocks_go_into_its_image_and_back() {
        let grid = Grid::new(20);
        let layout = Layout::new(&grid, Some(vec![0, 5, 16, 29]));
        let strip: Vec<u8> = (0..grid.width * grid.height).flat_map(|p| [(p % 251) as u8, (p % 13) as u8, 7, 255]).collect();
        let map = layout.map(&grid);
        let mut back = vec![0u8; strip.len()];
        scatter(&gather(&strip, &map), &map, &mut back);
        for block in [0u32, 5, 16, 29] {
            for k in 0..BLOCK * BLOCK {
                if let Some(p) = grid.pixel(block, k % BLOCK, k / BLOCK) {
                    assert_eq!(back[p * 4..p * 4 + 4], strip[p * 4..p * 4 + 4], "block {block}");
                }
            }
        }
    }
}
