//! The per-eye upscaler through a renderer's own DLSS calls against Dying Light 2's own per-eye
//! DLSS and camera history, which it was lifted from.

use super::tests::{LAYOUT, packet};
use super::*;
use monaka_core::mat4d::{inverse, mul};

/// DL2's matrices as they were: its packet's offsets written in.
#[derive(Default)]
struct Before {
    history: Option<EyeHistory<Shape, Matrix>>,
}

impl Before {
    #[allow(clippy::needless_range_loop)]
    fn clip(packet: &[u8]) -> Option<Matrix> {
        let mut p = [0.0; 16];
        let mut v = [0.0; 16];
        v[15] = 1.0;
        for i in 0..4 {
            for j in 0..4 {
                p[i * 4 + j] = read_f32(packet, 0x30 + 4 * (j * 4 + i)) as f64;
                if !p[i * 4 + j].is_finite() {
                    return None;
                }
            }
        }
        const BASIS: [usize; 3] = [0x188, 0x17c, 0x194];
        for (i, &row) in BASIS.iter().enumerate() {
            for j in 0..3 {
                let (x, pos) = (read_f32(packet, row + 4 * j) as f64, read_f32(packet, 0x170 + 4 * j) as f64);
                if !x.is_finite() || !pos.is_finite() {
                    return None;
                }
                v[i * 4 + j] = x;
                v[i * 4 + 3] -= x * pos;
            }
        }
        Some(mul(&p, &v))
    }

    #[allow(clippy::needless_range_loop)]
    fn write(packet: &mut [u8], offset: usize, m: &Matrix) {
        for i in 0..4 {
            for j in 0..4 {
                let at = offset + 4 * (j * 4 + i);
                packet[at..at + 4].copy_from_slice(&(m[i * 4 + j] as f32).to_le_bytes());
            }
        }
    }

    fn apply(&mut self, packet: &mut [u8], eye: u32, epoch: u32, viewport: u32, reset: bool) -> Option<bool> {
        if !(1..=2).contains(&eye) {
            return None;
        }
        let current = Self::clip(packet)?;
        let inverse_current = inverse(&current)?;
        let width = u32::from_le_bytes(packet[0x24..0x28].try_into().expect("4 bytes"));
        let height = u32::from_le_bytes(packet[0x28..0x2c].try_into().expect("4 bytes"));
        let eye = (eye - 1) as usize;
        let history = self.history.get_or_insert_with(|| EyeHistory::new(Rules::OWN));
        history.frame(eye, (epoch, viewport, width, height), reset);
        let previous = history.previous(eye).copied();
        let seed = previous.is_none();
        let to_previous = mul(&previous.unwrap_or(current), &inverse_current);
        let to_current = inverse(&to_previous)?;
        Self::write(packet, 0xf0, &to_previous);
        Self::write(packet, 0x130, &to_current);
        if seed {
            packet[0x18..0x1c].copy_from_slice(&1u32.to_le_bytes());
        }
        history.keep(eye, current);
        Some(seed)
    }
}

#[test]
fn each_eyes_matrices_are_dl2s() {
    let (mut before, mut now) = (Before::default(), DlssMatrices::default());
    // A run of frames: the eyes in turn, the camera moving, the epoch and viewport changing.
    let frames = [(1, 1, 3, [0.0, 0.0, 0.0]), (2, 1, 3, [0.064, 0.0, 0.0]), (1, 1, 3, [0.0, 0.0, -0.1]), (2, 1, 3, [0.064, 0.0, -0.1]), (1, 2, 3, [0.1, 0.2, -0.3]), (2, 2, 4, [0.2, 0.2, -0.3]), (3, 2, 4, [0.0; 3])];
    for (eye, epoch, viewport, position) in frames {
        for reset in [false, true] {
            let (mut a, mut b) = (packet(position), packet(position));
            assert_eq!(now.apply(&LAYOUT, &mut a, eye, epoch, viewport, reset), before.apply(&mut b, eye, epoch, viewport, reset), "eye {eye} epoch {epoch}");
            assert_eq!(a, b, "the packets written");
        }
    }
}

/// DL2's camera history epochs as they were (its statics, here a struct).
#[derive(Default)]
struct HistoryBefore {
    epoch: u32,
    reset: bool,
    camera: usize,
    counter: u32,
}

impl HistoryBefore {
    fn begin(&mut self, counter: u32, eye: u32, camera: usize) {
        if eye == 1 && std::mem::take(&mut self.reset) {
            self.epoch += 1;
        }
        if eye != 0 {
            if self.camera != camera || counter.wrapping_sub(self.counter) > 1 {
                self.epoch += 1;
            }
            self.camera = camera;
            self.counter = counter;
        } else {
            self.camera = 0;
            self.epoch += 1;
        }
    }
}

#[test]
fn the_history_epochs_are_dl2s() {
    let now: EyeCameras<16, 8> = EyeCameras::new();
    let mut before = HistoryBefore { epoch: 1, reset: true, ..HistoryBefore::default() };
    let scenes = [(10, 1, 0x100), (10, 2, 0x100), (11, 1, 0x100), (11, 2, 0x100), (11, 0, 0), (12, 1, 0x100), (12, 2, 0x200), (15, 1, 0x200), (15, 2, 0x200), (16, 1, 0x200)];
    for (i, (counter, eye, camera)) in scenes.into_iter().enumerate() {
        if i == 7 {
            now.start_over();
            before.reset = true;
        }
        now.begin_scene(counter, 9, eye, camera);
        before.begin(counter, eye, camera);
        assert_eq!(now.epoch(), before.epoch, "scene {i}");
    }
    let writers = [0x11161b6, 0x11161e4, 0x1116243, 0x11162ad, 0x111632d, 0x11163a3, 0x111640d];
    let before_bit = |rva: usize| writers.iter().position(|&r| r == rva).map_or(0, |i| 1 << i);
    for rva in writers.iter().copied().chain([0, 0x1234]) {
        assert_eq!(writer_bit(&writers, rva), before_bit(rva));
    }
    assert_eq!(all_writers(writers.len()), 127, "DL2's seven writers");
}
