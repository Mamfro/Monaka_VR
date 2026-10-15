//! A recorded debug stream: what the viewer kept of the last seconds of a run, one [`Frame`] per
//! pair it showed: both eyes' poses and fields of view, the head and the controllers as tracked,
//! and the debug drawing ([`monaka_contract::debug_draw`]) each eye was drawn with. The viewer writes it,
//! the analysis mode and the playground's playback read it.
//!
//! The file: the 8 bytes `MNKSTRM1`, a little-endian `u32` version and a reserved `u32`, then
//! frames, each a `u32` byte length followed by that many bytes:
//!
//! - `u64` pair sequence, `u64` viewer clock (ms);
//! - per eye: orientation (4 `f32`, x y z w), position (3 `f32`), field of view (4 `f32`: left,
//!   right, up, down angles);
//! - the head, then the left aim, right aim, left grip, right grip controller poses: each an
//!   orientation (4 `f32`), a position (3 `f32`) and a `u32` (1: tracked);
//! - per eye: a `u32` count, then that many primitives of 32 bytes (`a` and `b`, 3 `f32` each;
//!   colour `u32`; category `u16`; kind `u16`).
//!
//! All positions are in the headset's tracking space, in metres.

use monaka_contract::debug_draw::{Category, KIND_SEGMENT, Primitive};
use crate::math::{Quat, Vec3};

#[cfg(test)]
mod tests;

pub const MAGIC: &[u8; 8] = b"MNKSTRM1";
pub const VERSION: u32 = 1;

/// A pose in tracking space; `tracked` false for a controller or head that was not.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pose {
    pub orientation: Quat,
    pub position: Vec3,
    pub tracked: bool,
}

/// An eye as its image was drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Eye {
    pub orientation: Quat,
    pub position: Vec3,
    /// Angles (radians): left, right, up, down.
    pub fov: [f32; 4],
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Frame {
    pub sequence: u64,
    /// The viewer's clock (ms) when it showed the pair.
    pub tick: u64,
    pub eyes: [Eye; 2],
    pub head: Pose,
    /// Left aim, right aim, left grip, right grip.
    pub controllers: [Pose; 4],
    pub primitives: [Vec<Primitive>; 2],
}

/// The file's start.
pub fn header() -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    out.extend(VERSION.to_le_bytes());
    out.extend(0u32.to_le_bytes());
    out
}

impl Frame {
    /// The frame as it goes in the file, its length first.
    pub fn encode(&self) -> Vec<u8> {
        let mut body = Vec::with_capacity(256 + 32 * (self.primitives[0].len() + self.primitives[1].len()));
        body.extend(self.sequence.to_le_bytes());
        body.extend(self.tick.to_le_bytes());
        for eye in &self.eyes {
            floats(&mut body, &eye.orientation);
            floats(&mut body, &eye.position);
            floats(&mut body, &eye.fov);
        }
        for pose in std::iter::once(&self.head).chain(&self.controllers) {
            floats(&mut body, &pose.orientation);
            floats(&mut body, &pose.position);
            body.extend(u32::from(pose.tracked).to_le_bytes());
        }
        for eye in &self.primitives {
            body.extend((eye.len() as u32).to_le_bytes());
            for p in eye {
                floats(&mut body, &p.a);
                floats(&mut body, &p.b);
                body.extend(p.colour.to_le_bytes());
                body.extend(p.category.to_le_bytes());
                body.extend(p.kind.to_le_bytes());
            }
        }
        let mut out = (body.len() as u32).to_le_bytes().to_vec();
        out.extend(body);
        out
    }
}

fn floats(out: &mut Vec<u8>, values: &[f32]) {
    for v in values {
        out.extend(v.to_le_bytes());
    }
}

/// Reads a stream file's frames; a frame cut short at the end (a file still being written) is
/// left out.
pub fn read(bytes: &[u8]) -> Result<Vec<Frame>, String> {
    if bytes.len() < 16 || &bytes[..8] != MAGIC {
        return Err("not a Monaka debug stream".into());
    }
    let version = u32::from_le_bytes(bytes[8..12].try_into().map_err(|_| "short header")?);
    if version != VERSION {
        return Err(format!("stream version {version}, this reads {VERSION}"));
    }
    let mut frames = Vec::new();
    let mut at = 16;
    while at + 4 <= bytes.len() {
        let length = u32::from_le_bytes(bytes[at..at + 4].try_into().map_err(|_| "short length")?) as usize;
        let Some(body) = bytes.get(at + 4..at + 4 + length) else { break };
        frames.push(decode(body).ok_or_else(|| format!("frame {} is malformed", frames.len()))?);
        at += 4 + length;
    }
    Ok(frames)
}

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Cursor<'_> {
    fn take<const N: usize>(&mut self) -> Option<[u8; N]> {
        let out = self.bytes.get(self.at..self.at + N)?.try_into().ok()?;
        self.at += N;
        Some(out)
    }

    fn u16(&mut self) -> Option<u16> {
        self.take().map(u16::from_le_bytes)
    }

    fn u32(&mut self) -> Option<u32> {
        self.take().map(u32::from_le_bytes)
    }

    fn u64(&mut self) -> Option<u64> {
        self.take().map(u64::from_le_bytes)
    }

    fn floats<const N: usize>(&mut self) -> Option<[f32; N]> {
        let mut out = [0.0; N];
        for v in &mut out {
            *v = f32::from_le_bytes(self.take()?);
        }
        Some(out)
    }
}

fn decode(body: &[u8]) -> Option<Frame> {
    let mut c = Cursor { bytes: body, at: 0 };
    let mut frame = Frame { sequence: c.u64()?, tick: c.u64()?, ..Frame::default() };
    for eye in &mut frame.eyes {
        *eye = Eye { orientation: c.floats()?, position: c.floats()?, fov: c.floats()? };
    }
    for pose in std::iter::once(&mut frame.head).chain(&mut frame.controllers) {
        *pose = Pose { orientation: c.floats()?, position: c.floats()?, tracked: c.u32()? != 0 };
    }
    for eye in &mut frame.primitives {
        let count = c.u32()? as usize;
        // A count past what the bytes hold is a malformed frame, not an allocation.
        if count > body.len() / 32 {
            return None;
        }
        eye.reserve(count);
        for _ in 0..count {
            eye.push(Primitive { a: c.floats()?, b: c.floats()?, colour: c.u32()?, category: c.u16()?, kind: c.u16()? });
        }
    }
    Some(frame)
}

/// A short report of a recording: its length and rate, the drawings per frame, the skeleton's
/// shake (the bones' frame-to-frame wobble that is not motion), and the controllers' fastest
/// moves.
pub fn report(frames: &[Frame]) -> String {
    let mut out = Vec::new();
    let (Some(first), Some(last)) = (frames.first(), frames.last()) else { return "No frames.".into() };
    let seconds = last.tick.saturating_sub(first.tick) as f32 / 1000.0;
    out.push(format!("{} frames over {seconds:.1} s ({:.0} pairs/s)", frames.len(), (frames.len().saturating_sub(1)) as f32 / seconds.max(1e-3)));
    let counts: Vec<String> = Category::ALL
        .iter()
        .filter_map(|c| {
            let total: usize = frames.iter().map(|f| f.primitives[0].iter().filter(|p| p.category == *c as u16).count()).sum();
            (total > 0).then(|| format!("{} {:.0}", c.label(), total as f32 / frames.len() as f32))
        })
        .collect();
    out.push(if counts.is_empty() { "No drawings (nothing was switched on).".into() } else { format!("Drawings per frame, left eye: {}", counts.join(", ")) });
    out.extend(shake(frames));
    let names = ["left aim", "right aim", "left grip", "right grip"];
    for (index, name) in names.iter().enumerate() {
        let mut fastest = 0.0f32;
        for pair in frames.windows(2) {
            let (a, b) = (pair[0].controllers[index], pair[1].controllers[index]);
            let dt = pair[1].tick.saturating_sub(pair[0].tick) as f32 / 1000.0;
            if a.tracked && b.tracked && dt > 0.0 {
                fastest = fastest.max(crate::math::distance(a.position, b.position) / dt);
            }
        }
        let lost = frames.iter().filter(|f| !f.controllers[index].tracked).count();
        out.push(format!("Controller {name}: fastest {fastest:.1} m/s, untracked in {lost} frames"));
    }
    out.join("
")
}

/// The skeleton's shake: each bone's end (in the frames drawing the usual number of bones) as a
/// path; the second difference of a path is its acceleration per frame, mostly tracking or
/// animation noise at headset rates. The worst bones first, in millimetres.
fn shake(frames: &[Frame]) -> Vec<String> {
    let bones = |f: &Frame| -> Vec<[f32; 3]> { f.primitives[0].iter().filter(|p| p.category == Category::Skeleton as u16 && p.kind == KIND_SEGMENT).map(|p| p.b).collect() };
    let paths: Vec<Vec<[f32; 3]>> = frames.iter().map(bones).collect();
    let mut sizes: Vec<usize> = paths.iter().map(Vec::len).filter(|n| *n > 0).collect();
    sizes.sort_unstable();
    let Some(&usual) = sizes.get(sizes.len() / 2) else { return Vec::new() };
    let kept: Vec<&Vec<[f32; 3]>> = paths.iter().filter(|p| p.len() == usual).collect();
    if kept.len() < 3 {
        return Vec::new();
    }
    let mut worst: Vec<(f32, usize, [f32; 3])> = (0..usual)
        .map(|bone| {
            let wobble: Vec<f32> = kept.windows(3).map(|w| crate::math::length([0, 1, 2].map(|k| w[2][bone][k] - 2.0 * w[1][bone][k] + w[0][bone][k]))).collect();
            let rms = (wobble.iter().map(|v| v * v).sum::<f32>() / wobble.len() as f32).sqrt();
            (rms * 1000.0, bone, kept[0][bone])
        })
        .collect();
    worst.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut out = vec![format!("Skeleton shake over {} frames of {usual} bones (mm per frame, worst first):", kept.len())];
    out.extend(worst.iter().take(8).map(|(mm, bone, at)| format!("  bone {bone}: {mm:.2} mm (first at {:.2}, {:.2}, {:.2})", at[0], at[1], at[2])));
    out
}
