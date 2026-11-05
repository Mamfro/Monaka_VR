//! A home scene's files, read into what the GPU draws ([`super::ScenePass`]).
//!
//! A scene is a folder of captures: one folder per capture point, `c` in the middle and `f`, `b`, `l`, `r`, `u`, `d` half
//! a metre forward, back, left, right, up and down from it (those present), each with `depth.exr` and an image for each
//! time of day it was captured at: `hour_HH.png` on the hour, `hour_HH_MM.png` past it (any times,
//! as many as were captured: [`Minutes`]). Every image is a cube map as one strip, six square faces side by
//! side (front, right, back, left, up, down), one pixel per ray of the game's own chunky look; the
//! depth is each ray's distance (metres) to what it hit, 0 for the sky.
//!
//! Each pixel is drawn as a tile at its own distance, so the room has depth and leaning shows it
//! from a little to the side ([`corners`]). Where the middle capture could not see (behind the
//! stools, as a lean shows), tiles from the other captures fill in ([`unseen`]).

use std::path::{Path, PathBuf};

/// The capture points (folder, offset): x right, y up, z back, as the room is (forward is -z).
pub const PLACES: [(&str, [f32; 3]); 7] = [
    ("c", [0.0; 3]),
    ("f", [0.0, 0.0, -0.5]),
    ("b", [0.0, 0.0, 0.5]),
    ("l", [-0.5, 0.0, 0.0]),
    ("r", [0.5, 0.0, 0.0]),
    ("u", [0.0, 0.5, 0.0]),
    ("d", [0.0, -0.5, 0.0]),
];

/// Each face's direction straight out, its image's up and its image's right, in the room's axes.
pub const LOOK: [[f32; 3]; 6] = [[0.0, 0.0, -1.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, -1.0, 0.0]];
pub const TOP: [[f32; 3]; 6] = [[0.0, 1.0, 0.0], [0.0, 1.0, 0.0], [0.0, 1.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.0, 0.0, -1.0]];
pub const RIGHT: [[f32; 3]; 6] = [[1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [-1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [1.0, 0.0, 0.0], [1.0, 0.0, 0.0]];

/// The dither the scene is shown with: 32 steps a channel at full strength. Captures come without it (smooth, so the hours blend before the dither); captures
/// made with it come out the same, as dithering them again changes nothing.
pub const SHADES: u32 = 32;
pub const DITHER: f32 = 1.0;

/// How far away the sky is drawn (metres): far enough that leaning does not move it.
pub const SKY: f32 = 1000.0;
/// The nearest distance a 16-bit distance holds (metres: [`distance_code`]).
pub const NEAREST: f32 = 0.01;

/// A distance as 16 bits: 0 for none known (the sky), else in even steps of its log from
/// [`NEAREST`] to [`SKY`] (each about 0.02% from the next). `scene.hlsl` reads them back.
pub fn distance_code(distance: f32) -> u16 {
    let (low, high) = (NEAREST.ln(), SKY.ln());
    (1.0 + ((distance.clamp(NEAREST, SKY).ln() - low) / (high - low) * 65534.0).round()) as u16
}

/// A distance from its [`distance_code`].
pub fn distance_of(code: u16) -> f32 {
    if code == 0 {
        return SKY;
    }
    let (low, high) = (NEAREST.ln(), SKY.ln());
    (low + f32::from(code - 1) / 65534.0 * (high - low)).exp()
}
/// Neighbouring pixels are one surface while their distances differ by less than this much
/// (as a ratio, in logs): a floor seen low ahead changes by a few percent a pixel, an edge by more.
const SAME_SURFACE: f32 = 0.08;
/// A point another capture saw is one the middle saw too while the middle's ray towards it goes at
/// least this close to it (a fraction of its distance).
const SEEN_WITHIN: f32 = 0.04;

/// One capture point, ready for the GPU.
pub struct Place {
    pub name: &'static str,
    pub offset: [f32; 3],
    /// Each pixel's corners' distances (top left, top right, bottom left, bottom right), as
    /// [`corners`] makes them.
    pub corners: Vec<[f32; 4]>,
    /// The pixels drawn: all of the middle's, and of the others only those the middle did not see.
    pub tiles: Vec<u32>,
    /// The middle's edges between a near thing and what is behind it ([`skirts`]); none for the
    /// others.
    pub skirts: Vec<u32>,
}

/// A time of day: minutes past midnight.
pub type Minutes = u16;

/// A scene as read: its places, and the colours of the times asked for.
pub struct Loaded {
    pub name: String,
    pub face: u32,
    /// Dither steps per channel (0: none), and its strength.
    pub shades: u32,
    pub dither: f32,
    /// Every time of day the scene was captured at, in order.
    pub times: Vec<Minutes>,
    /// How far the middle capture sees to its left and right, level (metres; [`SKY`] for none).
    pub sides: [f32; 2],
    pub places: Vec<Place>,
    /// Each time read: its colour images (RGBA, as [`read_colour`]), one per place, in order.
    pub colours: Vec<(Minutes, Vec<Vec<u8>>)>,
}

/// The first scene in `scenes` (by name): a packed room ([`super::pack`]), or a folder with a middle
/// capture.
pub fn find(scenes: &Path) -> Option<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(scenes).ok()?.flatten().map(|e| e.path()).filter(|p| super::pack::is_pack(p) || p.join("c").join("depth.exr").is_file()).collect();
    found.sort();
    found.into_iter().next()
}

/// The times of day every capture point of `folder` has an image for, in order.
pub fn times(folder: &Path, places: &[&str]) -> Vec<Minutes> {
    let Some(first) = places.first() else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(folder.join(first)) else { return Vec::new() };
    let mut times: Vec<Minutes> = entries.flatten().filter_map(|e| time_of(&e.file_name().to_string_lossy())).filter(|&t| places.iter().all(|place| colour_path(folder, place, t).is_file())).collect();
    times.sort_unstable();
    times.dedup();
    times
}

/// The time an image's file name says: `hour_HH.png`, or `hour_HH_MM.png`.
pub(crate) fn time_of(name: &str) -> Option<Minutes> {
    let rest = name.strip_prefix("hour_")?.strip_suffix(".png")?;
    let (hour, minute) = rest.split_once('_').unwrap_or((rest, "0"));
    let (hour, minute): (Minutes, Minutes) = (hour.parse().ok()?, minute.parse().ok()?);
    (hour < 24 && minute < 60).then_some(hour * 60 + minute)
}

/// The image of `place` captured at `time`.
pub fn colour_path(folder: &Path, place: &str, time: Minutes) -> PathBuf {
    let (hour, minute) = (time / 60, time % 60);
    let name = if minute == 0 { format!("hour_{hour:02}.png") } else { format!("hour_{hour:02}_{minute:02}.png") };
    folder.join(place).join(name)
}

/// A time of day as people write it (`7:30`).
pub fn clock_text(time: Minutes) -> String {
    format!("{}:{:02}", time / 60, time % 60)
}

/// Reads the scene in `folder` (or packed there: [`super::pack`]), with the colours of `wanted`
/// times (those it has).
pub fn load(folder: &Path, wanted: impl Fn(&[Minutes]) -> Vec<Minutes>) -> Result<Loaded, String> {
    if super::pack::is_pack(folder) {
        return super::pack::load(folder, wanted);
    }
    let name = folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let present: Vec<(&'static str, [f32; 3])> = PLACES.into_iter().filter(|(place, _)| folder.join(place).join("depth.exr").is_file()).collect();
    let (face, centre) = read_depth(&folder.join("c").join("depth.exr"))?;
    let mut depths = vec![centre];
    for (place, _) in &present[1..] {
        let (size, depth) = read_depth(&folder.join(place).join("depth.exr"))?;
        if size != face {
            return Err(format!("{place}/depth.exr has {size}-pixel faces, c/depth.exr {face}"));
        }
        depths.push(depth);
    }
    let names: Vec<&str> = present.iter().map(|(place, _)| *place).collect();
    let times = times(folder, &names);
    if times.is_empty() {
        return Err("no time of day has an image at every capture point".into());
    }
    let mut places = Vec::new();
    for (index, ((place, offset), depth)) in present.iter().zip(&depths).enumerate() {
        let (tiles, edges) = if index == 0 { ((0..depth.len() as u32).collect(), skirts(depth, face)) } else { (unseen(&depths[0], depth, *offset, face), Vec::new()) };
        places.push(Place { name: place, offset: *offset, corners: corners(depth, face), tiles, skirts: edges });
    }
    let colours = wanted(&times).into_iter().map(|time| load_time(folder, &names, time, face).map(|images| (time, images))).collect::<Result<_, _>>()?;
    let sides = [[-1.0, 0.0, 0.0], [1.0, 0.0, 0.0]].map(|dir| depths[0][strip_pixel(face, dir)]);
    Ok(Loaded { name, face, shades: SHADES, dither: DITHER, times, sides, places, colours })
}

/// The colour images of `time`, one per place.
pub fn load_time(folder: &Path, places: &[&str], time: Minutes, face: u32) -> Result<Vec<Vec<u8>>, String> {
    if super::pack::is_pack(folder) {
        return super::pack::load_time(folder, time);
    }
    places
        .iter()
        .map(|place| {
            let path = colour_path(folder, place, time);
            let (width, height, pixels) = read_colour(&path)?;
            if (width, height) != (face * 6, face) {
                return Err(format!("{} is {width}x{height}, not {}x{face}", path.display(), face * 6));
            }
            Ok(pixels)
        })
        .collect()
}

/// A depth strip: its face size, and each pixel's distance (metres; [`SKY`] for the sky).
pub fn read_depth(path: &Path) -> Result<(u32, Vec<f32>), String> {
    use exr::prelude::{FlatSamples, read_first_flat_layer_from_file};
    let image = read_first_flat_layer_from_file(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let layer = image.layer_data;
    let (width, height) = (layer.size.0, layer.size.1);
    if width != height * 6 || height == 0 {
        return Err(format!("{} is {width}x{height}, not six square faces side by side", path.display()));
    }
    let channel = layer.channel_data.list.first().ok_or_else(|| format!("{} has no channel", path.display()))?;
    let samples: Vec<f32> = match &channel.sample_data {
        FlatSamples::F32(values) => values.clone(),
        FlatSamples::F16(values) => values.iter().map(|v| v.to_f32()).collect(),
        FlatSamples::U32(values) => values.iter().map(|v| *v as f32).collect(),
    };
    let distances = samples.into_iter().map(|d| if d.is_finite() && d > 0.0 { d.min(SKY) } else { SKY }).collect();
    Ok((height as u32, distances))
}

/// A PNG as RGBA, 8 bits a channel: its width, height and pixels, rows top first.
pub fn read_colour(path: &Path) -> Result<(u32, u32, Vec<u8>), String> {
    let failed = |e: png::DecodingError| format!("{}: {e}", path.display());
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut decoder = png::Decoder::new(std::io::BufReader::new(file));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(failed)?;
    let mut buffer = vec![0; reader.output_buffer_size().ok_or_else(|| format!("{}: too large", path.display()))?];
    let info = reader.next_frame(&mut buffer).map_err(failed)?;
    let pixels = &buffer[..info.buffer_size()];
    let rgba = match info.color_type {
        png::ColorType::Rgba => pixels.to_vec(),
        png::ColorType::Rgb => pixels.as_chunks::<3>().0.iter().flat_map(|[r, g, b]| [*r, *g, *b, 255]).collect(),
        png::ColorType::GrayscaleAlpha => pixels.as_chunks::<2>().0.iter().flat_map(|[g, a]| [*g, *g, *g, *a]).collect(),
        png::ColorType::Grayscale => pixels.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return Err(format!("{}: palette left unexpanded", path.display())),
    };
    Ok((info.width, info.height, rgba))
}

/// The direction (not of unit length) through the point `(u, v)` of face `face`, `n` pixels a
/// side: `u` from its left edge, `v` from its top, in pixels.
pub fn direction(n: u32, face: usize, u: f32, v: f32) -> [f32; 3] {
    let a = 2.0 * u / n as f32 - 1.0;
    let b = 1.0 - 2.0 * v / n as f32;
    [0, 1, 2].map(|k| LOOK[face][k] + a * RIGHT[face][k] + b * TOP[face][k])
}

/// The pixel of the strip (`n` pixels a face) that direction `dir` passes through.
pub fn strip_pixel(n: u32, dir: [f32; 3]) -> usize {
    let dot = |a: [f32; 3]| a[0] * dir[0] + a[1] * dir[1] + a[2] * dir[2];
    let face = (0..6).max_by(|&a, &b| dot(LOOK[a]).total_cmp(&dot(LOOK[b]))).unwrap_or(0);
    let ahead = dot(LOOK[face]);
    let along = |axis: [f32; 3]| dot(axis) / ahead;
    let size = n as f32;
    let i = (((along(RIGHT[face]) + 1.0) * 0.5 * size) as i64).clamp(0, n as i64 - 1) as usize;
    let j = (((1.0 - along(TOP[face])) * 0.5 * size) as i64).clamp(0, n as i64 - 1) as usize;
    j * 6 * n as usize + face * n as usize + i
}

/// Whether two distances are one surface: both sky, or near alike.
fn same_surface(a: f32, b: f32) -> bool {
    match (a >= SKY, b >= SKY) {
        (true, true) => true,
        (false, false) => (a / b).ln().abs() < SAME_SURFACE,
        _ => false,
    }
}

/// Each pixel's corners' distances (top left, top right, bottom left, bottom right). A corner is
/// the mean of the pixels meeting there that are one surface with this one: where neighbours are
/// one surface they agree, and the tiles join without cracks; at an edge the tile ends where the
/// pixel does, and leaning shows the gap behind it (which another capture fills: [`unseen`]).
pub fn corners(depth: &[f32], n: u32) -> Vec<[f32; 4]> {
    let size = n as usize;
    let width = size * 6;
    // The pixels meeting at each grid point of each face: inside the face, the four around it;
    // on its edges, those the directions just past it pass through on the faces beside.
    let meeting = |face: usize, x: usize, y: usize| -> [usize; 4] {
        [(-0.5, -0.5), (0.5, -0.5), (-0.5, 0.5), (0.5, 0.5)].map(|(du, dv)| {
            let (u, v) = (x as f32 + du, y as f32 + dv);
            if u > 0.0 && v > 0.0 && u < n as f32 && v < n as f32 {
                v as usize * width + face * size + u as usize
            } else {
                strip_pixel(n, direction(n, face, u, v))
            }
        })
    };
    let mut out = vec![[0.0; 4]; depth.len()];
    for face in 0..6 {
        for y in 0..=size {
            for x in 0..=size {
                let around = meeting(face, x, y);
                // The pixels of this face with this grid point as a corner, and which corner.
                for (corner, (px, py)) in [(3usize, (x.wrapping_sub(1), y.wrapping_sub(1))), (2, (x, y.wrapping_sub(1))), (1, (x.wrapping_sub(1), y)), (0, (x, y))] {
                    if px >= size || py >= size {
                        continue;
                    }
                    let pixel = py * width + face * size + px;
                    let own = depth[pixel];
                    let (sum, count) = around.iter().map(|&p| depth[p]).filter(|&d| same_surface(own, d)).fold((0.0, 0), |(s, c), d| (s + d, c + 1));
                    out[pixel][corner] = if count == 0 { own } else { sum / count as f32 };
                }
            }
        }
    }
    out
}

/// The pixels of a capture at `offset` (its strip `depth`) that the middle capture (`centre`) did
/// not see, and those beside them (so the patches overlap what the middle drew, not leave a seam).
pub fn unseen(centre: &[f32], depth: &[f32], offset: [f32; 3], n: u32) -> Vec<u32> {
    let size = n as usize;
    let width = size * 6;
    let hidden: Vec<bool> = (0..depth.len())
        .map(|pixel| {
            let (x, y) = (pixel % width, pixel / width);
            let dir = direction(n, x / size, (x % size) as f32 + 0.5, y as f32 + 0.5);
            let length = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
            let d = depth[pixel];
            if d >= SKY {
                // The sky is as far one way from every capture: the middle saw it if it saw sky there.
                return centre[strip_pixel(n, dir)] < SKY;
            }
            let point = [0, 1, 2].map(|k| offset[k] + dir[k] / length * d);
            let far = (point[0] * point[0] + point[1] * point[1] + point[2] * point[2]).sqrt();
            let seen_to = centre[strip_pixel(n, point)];
            seen_to < far * (1.0 - SEEN_WITHIN)
        })
        .collect();
    (0..depth.len())
        .filter(|&pixel| {
            let (x, y) = (pixel % width, pixel / width);
            let i = x % size;
            let beside = [(i > 0).then(|| pixel - 1), (i + 1 < size).then(|| pixel + 1), (y > 0).then(|| pixel - width), (y + 1 < size).then(|| pixel + width)];
            // Beside one hidden, on its surface: the patch overlaps the middle's tiles there, and a
            // near thing's edge (which the middle drew) is not drawn again from aside.
            hidden[pixel] || beside.into_iter().flatten().any(|p| hidden[p] && same_surface(depth[p], depth[pixel]))
        })
        .map(|pixel| pixel as u32)
        .collect()
}

/// Marks a skirt along a pixel's bottom edge (else its right edge).
pub const SKIRT_BOTTOM: u32 = 1 << 31;

/// The edges within a face between pixels that are not one surface, each as its upper or left
/// pixel ([`SKIRT_BOTTOM`] set for the edge below it). Each is drawn as a skirt joining the two
/// tiles, coloured as the farther, wherever no tile was drawn: a gap no capture saw into shows
/// what lies behind it drawn on, not a hole.
pub fn skirts(depth: &[f32], n: u32) -> Vec<u32> {
    let size = n as usize;
    let width = size * 6;
    let mut edges = Vec::new();
    for pixel in 0..depth.len() {
        let (i, y) = (pixel % width % size, pixel / width);
        if i + 1 < size && !same_surface(depth[pixel], depth[pixel + 1]) {
            edges.push(pixel as u32);
        }
        if y + 1 < size && !same_surface(depth[pixel], depth[pixel + width]) {
            edges.push(pixel as u32 | SKIRT_BOTTOM);
        }
    }
    edges
}

/// How long a capture fades into the next (seconds before the next's time).
pub const FADE: f64 = 10.0;

/// A time's seconds past midnight.
fn seconds_of(time: Minutes) -> f64 {
    f64::from(time) * 60.0
}

/// What shows at `seconds` past midnight with the scene's `times`: the capture now (the last one
/// at or before then, yesterday's last before the first), the next one, and how far into it the
/// fade is (0 to 1).
pub fn shown_times(times: &[Minutes], seconds: f64) -> (Minutes, Minutes, f32) {
    let Some(&last) = times.last() else { return (0, 0, 0.0) };
    let now = times.iter().rev().copied().find(|&t| seconds_of(t) <= seconds).unwrap_or(last);
    let next = times.iter().copied().find(|&t| t > now).unwrap_or(times[0]);
    let until = (seconds_of(next) - seconds).rem_euclid(86_400.0);
    let fade = if next == now { 0.0 } else { (1.0 - until / FADE).clamp(0.0, 1.0) };
    (now, next, fade as f32)
}

/// What shows at `seconds` past midnight blended all the way between the captured `times` (as the
/// player sweeps the time): the capture last before then, the next, and how far between them (0
/// to 1).
pub fn blended_times(times: &[Minutes], seconds: f64) -> (Minutes, Minutes, f32) {
    let (now, next, _) = shown_times(times, seconds);
    if now == next {
        return (now, next, 0.0);
    }
    let span = (seconds_of(next) - seconds_of(now)).rem_euclid(86_400.0);
    let into = (seconds - seconds_of(now)).rem_euclid(86_400.0);
    (now, next, (into / span).clamp(0.0, 1.0) as f32)
}

/// The seconds past midnight of the capture nearest `seconds` (round midnight either way).
pub fn nearest_time(times: &[Minutes], seconds: f64) -> f64 {
    let apart = |t: Minutes| {
        let d = (seconds_of(t) - seconds).rem_euclid(86_400.0);
        if d > 43_200.0 { d - 86_400.0 } else { d }
    };
    times.iter().map(|&t| apart(t)).min_by(|a, b| a.abs().total_cmp(&b.abs())).map_or(seconds, |d| seconds + d)
}

/// Seconds past midnight on this computer's clock.
pub fn local_seconds() -> f64 {
    // SAFETY: fills and returns a plain struct.
    let time = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    f64::from(time.wHour) * 3600.0 + f64::from(time.wMinute) * 60.0 + f64::from(time.wSecond) + f64::from(time.wMilliseconds) / 1000.0
}
