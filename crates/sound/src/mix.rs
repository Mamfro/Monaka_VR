//! The binaural mix: what each ear hears of each of the recording's channels, from where its place
//! is from the head ([`hearing`]). Each channel goes through the ear responses measured on a dummy
//! head for its direction ([`Ears`], MIT's KEMAR set: `KEMAR.md`), which carry everything a head
//! and its ears do to a sound from there: which ear hears it first and louder, and the colour the
//! outer ears give it for ahead, behind, above and below. That colour is what puts a sound out in
//! the room rather than in the head. A few soft echoes off the surroundings add to it.
//!
//! As the head turns, each block of the mix fades from the responses it had to the new ones, so
//! nothing clicks.

use monaka_core::math::{Quat, Vec3, conjugate, length, rotate, sub};
use std::sync::OnceLock;

/// Where a sound is from the head, as the mix needs it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Hearing {
    /// Its direction from the head, in the head's axes (x right, y up, z back), of unit length.
    pub direction: Vec3,
    /// How loud for its distance: 1 where it was placed from, more nearer, less further.
    pub gain: f32,
}

/// Where a sound at `source` is from a head at `head` turned `orientation` (room axes: x right, y
/// up, z back), the sound placed `near` metres from where the head started.
pub fn hearing(source: Vec3, orientation: Quat, head: Vec3, near: f32) -> Hearing {
    let towards = rotate(conjugate(orientation), sub(source, head));
    let distance = length(towards).max(0.1);
    Hearing { direction: towards.map(|c| c / distance), gain: (near / distance).clamp(0.7, 1.4) }
}

/// Taps in each ear response, and the rate they were measured at (the mix's rate).
pub const TAPS: usize = 128;
pub const RATE: u32 = 44_100;

/// A direction's responses, the left ear's and the right's.
type Pair = [[f32; TAPS]; 2];

/// The measured ear responses, by elevation ring (lowest first) and azimuth on the right (0 ahead
/// to 180 behind), scaled so that a sound straight ahead keeps its level.
pub struct Ears {
    rings: Vec<(f32, Vec<(f32, Pair)>)>,
}

/// The responses as packed (`KEMAR.md`).
const PACKED: &[u8] = include_bytes!("kemar.bin");

impl Ears {
    /// The responses, read once.
    pub fn get() -> &'static Ears {
        static EARS: OnceLock<Ears> = OnceLock::new();
        EARS.get_or_init(|| Ears::read(PACKED).expect("the packed ear responses are whole"))
    }

    fn read(bytes: &[u8]) -> Option<Ears> {
        let mut at = 0usize;
        let mut take = |n: usize| {
            let part = bytes.get(at..at + n)?;
            at += n;
            Some(part)
        };
        if take(4)? != b"HRIR" {
            return None;
        }
        let u16_of = |b: &[u8]| u16::from_le_bytes([b[0], b[1]]);
        let (rate, taps) = (u32::from_le_bytes(take(4)?.try_into().ok()?), usize::from(u16_of(take(2)?)));
        if rate != RATE || taps != TAPS {
            return None;
        }
        let ring_count = u16_of(take(2)?);
        let mut rings = Vec::new();
        for _ in 0..ring_count {
            let elevation = f32::from(i16::from_le_bytes(take(2)?.try_into().ok()?));
            let count = u16_of(take(2)?);
            let mut ring = Vec::new();
            for _ in 0..count {
                let azimuth = f32::from(u16_of(take(2)?));
                let samples = take(TAPS * 4)?;
                let mut pair = [[0.0; TAPS]; 2];
                for (tap, [l0, l1, r0, r1]) in samples.as_chunks::<4>().0.iter().enumerate() {
                    pair[0][tap] = f32::from(i16::from_le_bytes([*l0, *l1])) / 32768.0;
                    pair[1][tap] = f32::from(i16::from_le_bytes([*r0, *r1])) / 32768.0;
                }
                ring.push((azimuth, pair));
            }
            rings.push((elevation, ring));
        }
        let mut ears = Ears { rings };
        // Ahead keeps its level: the response straight ahead passes as much as no response would.
        let ahead = ears.pair([0.0, 0.0, -1.0]);
        let scale = 1.0 / ahead[0].iter().map(|v| v * v).sum::<f32>().sqrt();
        for (_, ring) in &mut ears.rings {
            for (_, pair) in ring {
                for ear in pair.iter_mut() {
                    ear.iter_mut().for_each(|v| *v *= scale);
                }
            }
        }
        Some(ears)
    }

    /// The responses for `direction` (the head's axes, of unit length): blended from the measured
    /// directions round it, the left side the right's mirrored.
    pub fn pair(&self, direction: Vec3) -> Pair {
        let [x, y, z] = direction;
        let elevation = y.clamp(-1.0, 1.0).asin().to_degrees();
        let azimuth = x.atan2(-z).to_degrees();
        let left = azimuth < 0.0;
        let azimuth = azimuth.abs();
        // The two rings round the elevation, and how far between them.
        let below = self.rings.iter().rposition(|(e, _)| *e <= elevation).unwrap_or(0);
        let above = (below + 1).min(self.rings.len() - 1);
        let (low, high) = (self.rings[below].0, self.rings[above].0);
        let up = if above == below || elevation <= low { 0.0 } else { ((elevation - low) / (high - low)).clamp(0.0, 1.0) };
        let mut blended = [[0.0f32; TAPS]; 2];
        for (ring, weight) in [(below, 1.0 - up), (above, up)] {
            if weight <= 0.0 {
                continue;
            }
            for (pair, share) in Self::around(&self.rings[ring].1, azimuth) {
                for ear in 0..2 {
                    for tap in 0..TAPS {
                        blended[ear][tap] += pair[ear][tap] * share * weight;
                    }
                }
            }
        }
        if left {
            blended.swap(0, 1);
        }
        blended
    }

    /// The measured directions of a ring on either side of `azimuth`, and each one's share.
    fn around(ring: &[(f32, Pair)], azimuth: f32) -> Vec<(&Pair, f32)> {
        let after = ring.iter().position(|(a, _)| *a >= azimuth).unwrap_or(ring.len() - 1);
        let before = after.saturating_sub(1);
        let (a, b) = (ring[before].0, ring[after].0);
        if before == after || b <= a {
            return vec![(&ring[after].1, 1.0)];
        }
        let t = ((azimuth - a) / (b - a)).clamp(0.0, 1.0);
        vec![(&ring[before].1, 1.0 - t), (&ring[after].1, t)]
    }
}

/// The echoes off the surroundings, each ear its own (ms after the sound, how loud): a little
/// different in each ear, as a real space's are.
const ECHOES: [[(f32, f32); 4]; 2] = [[(7.3, 0.14), (13.1, 0.10), (19.7, 0.07), (29.0, 0.04)], [(8.9, 0.14), (15.4, 0.10), (21.2, 0.07), (31.5, 0.04)]];
/// The echoes are duller than the sound (Hz).
const ECHO_CUTOFF: f32 = 3000.0;
/// The whole mix's level: each ear hears both channels and their echoes.
const LEVEL: f32 = 0.7;

/// A sound that moved less than this (radians, about half a degree) keeps the responses it has: no
/// ear tells the difference, and the mix need not fade between two.
const STILL: f32 = 0.009;

/// The mix as it goes: where in the recording, the responses and gains each channel had last,
/// the last samples each channel went in with (the responses reach back over them), the echoes'
/// filter memory.
pub struct Mixer {
    /// Recording frames per mix frame.
    step: f64,
    at: f64,
    last: Option<[(Pair, f32, Vec3); 2]>,
    history: [Vec<f32>; 2],
    echo: [f32; 2],
}

impl Mixer {
    /// A mix of a recording at `rate` samples a second.
    pub fn new(rate: u32) -> Self {
        Self { step: f64::from(rate) / f64::from(RATE), at: 0.0, last: None, history: [vec![0.0; TAPS - 1], vec![0.0; TAPS - 1]], echo: [0.0; 2] }
    }

    /// Mixes the next `out.len() / 2` frames (at [`RATE`]) of the looping recording `samples`
    /// (`channels` interleaved; a mono one is both sources) into `out` (left, right interleaved),
    /// its two channels heard as `hearing` says, fading there from how they were heard before.
    pub fn render(&mut self, samples: &[i16], channels: usize, hearing: [Hearing; 2], out: &mut [f32]) {
        let ears = Ears::get();
        let frames = samples.len() / channels;
        let count = out.len() / 2;
        // The recording at frame `t` (a fraction between frames, looping round), channel `c`.
        let sample = |t: f64, c: usize| -> f32 {
            let t = t.rem_euclid(frames as f64);
            let i = t as usize % frames;
            let j = (i + 1) % frames;
            let f = (t - t.floor()) as f32;
            let at = |k: usize| f32::from(samples[k * channels + c.min(channels - 1)]) / 32768.0;
            at(i) + (at(j) - at(i)) * f
        };
        let moved = |h: &Hearing, (_, gain, direction): &(Pair, f32, Vec3)| {
            let along: f32 = (0..3).map(|k| h.direction[k] * direction[k]).sum();
            along.clamp(-1.0, 1.0).acos() > STILL || (h.gain - gain).abs() > 0.01
        };
        let to: [(Pair, f32, Vec3); 2] = std::array::from_fn(|s| match &self.last {
            Some(last) if !moved(&hearing[s], &last[s]) => last[s],
            _ => (ears.pair(hearing[s].direction), hearing[s].gain, hearing[s].direction),
        });
        let from = self.last.unwrap_or(to);
        out.fill(0.0);
        for source in 0..2 {
            // This block's samples of the channel, after the ones the responses reach back over.
            let mut input = std::mem::take(&mut self.history[source]);
            input.extend((0..count).map(|n| sample(self.at + n as f64 * self.step, source)));
            let ((old, old_gain, _), (new, new_gain, _)) = (&from[source], &to[source]);
            let still = old_gain == new_gain && old == new;
            for (n, frame) in out.as_chunks_mut::<2>().0.iter_mut().enumerate() {
                let window = &input[n..n + TAPS];
                let k = (n + 1) as f32 / count as f32;
                for (ear, heard) in frame.iter_mut().enumerate() {
                    let through = |pair: &Pair| -> f32 { pair[ear].iter().rev().zip(window).map(|(h, x)| h * x).sum() };
                    let now = through(new) * new_gain;
                    *heard += if still { now } else { through(old) * old_gain * (1.0 - k) + now * k };
                }
            }
            self.history[source] = input.split_off(input.len() - (TAPS - 1));
        }
        let pass = 1.0 - (-2.0 * std::f32::consts::PI * ECHO_CUTOFF / RATE as f32).exp();
        let ms = self.step * f64::from(RATE) / 1000.0;
        for (n, frame) in out.as_chunks_mut::<2>().0.iter_mut().enumerate() {
            let t = self.at + n as f64 * self.step;
            for (ear, heard) in frame.iter_mut().enumerate() {
                let echoed: f32 = ECHOES[ear].iter().map(|(after, loud)| loud * (sample(t - f64::from(*after) * ms, 0) + sample(t - f64::from(*after) * ms, 1)) * 0.5).sum();
                self.echo[ear] += pass * (echoed - self.echo[ear]);
                *heard = (*heard + self.echo[ear]) * LEVEL;
            }
        }
        self.at = (self.at + count as f64 * self.step).rem_euclid(frames as f64);
        self.last = Some(to);
    }
}
