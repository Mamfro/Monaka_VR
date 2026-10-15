//! The per-eye upscaler through a renderer's own DLSS calls (not Streamline's): a game whose
//! renderer calls DLSS itself keeps each eye's history apart here. The game hooks its renderer's
//! DLSS calls (constants and evaluate) and its scene's camera history, and says once where those
//! are, which of the scene's calls write its camera, and its constants packet's layout
//! ([`PacketLayout`]); this does the rest:
//!
//! - each eye gets its own DLSS feature: the renderer's viewport id swapped for the eye's own
//!   around each call ([`for_eye`], `monaka_streamline::own_eye_viewport`);
//! - each eye's constants describe its own previous frame: a copy of the packet with the
//!   clip-to-previous-clip matrices rebuilt from that eye's own last clip transform
//!   ([`DlssMatrices`]), the reset flag set when its history starts over;
//! - each eye keeps its own previous camera in the scene's camera history ([`EyeCameras`]): the
//!   scene's writers' entries banked per eye after each scene call, and every other lookup during
//!   an eye's scene served from that eye's bank (`monaka_core::eye_history::Bank`).
//!
//! A new history epoch starts whenever continuity breaks (a pause, another camera, a skipped frame):
//! both the camera history and the DLSS matrices then seed afresh.

use monaka_core::Aligned16;
use monaka_core::eye_history::{Bank, EyeHistory, Rules};
use monaka_core::mat4d::{Mat4d, inverse, mul};
use monaka_core::sync::Locked;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex};

#[cfg(test)]
mod tests;

/// Where a renderer's DLSS constants packet keeps what each eye's history touches (byte offsets).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PacketLayout {
    /// The packet's size.
    pub bytes: usize,
    /// The reset flag (u32).
    pub reset: usize,
    /// The render width and height (u32 each).
    pub size: [usize; 2],
    /// The projection (4x4 f32, column-major).
    pub projection: usize,
    /// The camera position (3 f32).
    pub position: usize,
    /// The camera's right, up and view-z rows (3 f32 each).
    pub basis: [usize; 3],
    /// The clip-to-previous-clip matrix and its inverse (4x4 f32, column-major).
    pub to_previous: usize,
    pub to_current: usize,
}

/// The largest packet a layout may name.
pub const PACKET_MOST: usize = 0x400;

type Matrix = Mat4d;

fn read_f32(p: &[u8], offset: usize) -> f32 {
    f32::from_le_bytes(p[offset..offset + 4].try_into().expect("4 bytes"))
}

fn read_u32(p: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(p[offset..offset + 4].try_into().expect("4 bytes"))
}

impl PacketLayout {
    /// The packet's clip transform: its projection times the view its camera rows and position make.
    #[allow(clippy::needless_range_loop)]
    fn clip(&self, packet: &[u8]) -> Option<Matrix> {
        let mut p = [0.0; 16];
        let mut v = [0.0; 16];
        v[15] = 1.0;
        for i in 0..4 {
            for j in 0..4 {
                p[i * 4 + j] = read_f32(packet, self.projection + 4 * (j * 4 + i)) as f64;
                if !p[i * 4 + j].is_finite() {
                    return None;
                }
            }
        }
        for (i, &row) in self.basis.iter().enumerate() {
            for j in 0..3 {
                let (x, pos) = (read_f32(packet, row + 4 * j) as f64, read_f32(packet, self.position + 4 * j) as f64);
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
}

/// What an eye's DLSS history carries on through: the history epoch, the eye's viewport id and the
/// render size.
type Shape = (u32, u32, u32, u32);

/// Each eye's previous clip transform, for its constants' clip-to-previous matrices
/// (`monaka_core::eye_history`).
pub struct DlssMatrices {
    history: EyeHistory<Shape, Matrix>,
}

impl Default for DlssMatrices {
    fn default() -> Self {
        Self { history: EyeHistory::new(Rules::OWN) }
    }
}

impl DlssMatrices {
    /// Writes eye `eye`'s (1 or 2) clip-to-previous and previous-to-clip into `packet` (laid out as
    /// `layout` says), and sets its reset flag when the eye's history starts over: its first frame,
    /// `reset` asked for, a new epoch, viewport or size. Whether it started over; `None` when the
    /// matrices could not be made.
    pub fn apply(&mut self, layout: &PacketLayout, packet: &mut [u8], eye: u32, epoch: u32, viewport: u32, reset: bool) -> Option<bool> {
        if !(1..=2).contains(&eye) {
            return None;
        }
        let current = layout.clip(packet)?;
        let inverse_current = inverse(&current)?;
        let (width, height) = (read_u32(packet, layout.size[0]), read_u32(packet, layout.size[1]));
        let eye = (eye - 1) as usize;
        self.history.frame(eye, (epoch, viewport, width, height), reset);
        let previous = self.history.previous(eye).copied();
        let seed = previous.is_none();
        let to_previous = mul(&previous.unwrap_or(current), &inverse_current);
        let to_current = inverse(&to_previous)?;
        PacketLayout::write(packet, layout.to_previous, &to_previous);
        PacketLayout::write(packet, layout.to_current, &to_current);
        if seed {
            packet[layout.reset..layout.reset + 4].copy_from_slice(&1u32.to_le_bytes());
        }
        self.history.keep(eye, current);
        Some(seed)
    }
}

/// The calls made per eye, the histories started over, and the calls whose matrices or own feature
/// could not be had.
pub static EYE_CALLS: AtomicU64 = AtomicU64::new(0);
pub static RESETS: AtomicU64 = AtomicU64::new(0);
pub static MATRIX_FAILURES: AtomicU64 = AtomicU64::new(0);

static MATRICES: LazyLock<Mutex<DlssMatrices>> = LazyLock::new(|| Mutex::new(DlssMatrices::default()));

/// A constants packet as the renderer reads it (aligned for SSE loads).
pub type Packet = Aligned16<[u8; PACKET_MOST]>;

/// A DLSS call (constants or evaluate) made for eye `eye` (1 or 2; 0: the game's own): the
/// renderer's viewport id at `viewport` is swapped for the eye's own feature for the call (and a
/// cached copy at `cache`, when the renderer keeps one, put back after), and a constants call gets
/// a copy of `packet` with the eye's own history matrices (at history `epoch`). `original` is the
/// renderer's own call, given the packet to pass.
///
/// # Safety
/// `viewport` (and `cache`) point at the renderer's live u32 viewport ids, `packet` at a DLSS
/// constants packet laid out as `layout` says (for a constants call), and the caller holds whatever
/// lock keeps other DLSS calls out meanwhile.
#[allow(clippy::too_many_arguments)]
pub unsafe fn for_eye(layout: &PacketLayout, eye: u32, viewport: *mut u32, cache: Option<*mut u32>, packet: usize, evaluate: bool, epoch: u32, original: impl FnOnce(usize) -> usize) -> usize {
    // SAFETY: the caller's guarantee.
    unsafe {
        let native = *viewport;
        let Some(mapped) = monaka_streamline::own_eye_viewport(native, eye) else {
            if eye != 0 {
                MATRIX_FAILURES.fetch_add(1, Ordering::Relaxed);
            }
            return original(packet);
        };
        let cached = cache.map(|c| *c);
        *viewport = mapped;
        let copy;
        let packet = if evaluate {
            EYE_CALLS.fetch_add(1, Ordering::Relaxed);
            packet
        } else {
            copy = eye_packet(layout, packet, eye, mapped, epoch);
            copy.0.as_ptr() as usize
        };
        let result = original(packet);
        *viewport = native;
        if let (Some(cache), Some(cached)) = (cache, cached) {
            *cache = cached;
        }
        result
    }
}

/// A copy of the constants packet at `packet` with eye `eye`'s history matrices, and the reset flag
/// set when this eye's history starts over.
///
/// # Safety
/// `packet` must point at a DLSS constants packet of at least `layout.bytes` bytes.
pub unsafe fn eye_packet(layout: &PacketLayout, packet: usize, eye: u32, viewport: u32, epoch: u32) -> Packet {
    let mut copy = Aligned16([0; PACKET_MOST]);
    let bytes = layout.bytes.min(PACKET_MOST);
    // SAFETY: the caller's guarantee.
    unsafe { std::ptr::copy_nonoverlapping(packet as *const u8, copy.0.as_mut_ptr(), bytes) };
    let flag = read_u32(&copy.0, layout.reset);
    let mut matrices = MATRICES.locked();
    let reset = matrices.apply(layout, &mut copy.0[..bytes], eye, epoch, viewport, flag != 0).unwrap_or_else(|| {
        MATRIX_FAILURES.fetch_add(1, Ordering::Relaxed);
        true
    });
    if reset {
        RESETS.fetch_add(1, Ordering::Relaxed);
        copy.0[layout.reset..layout.reset + 4].copy_from_slice(&1u32.to_le_bytes());
    }
    copy
}

/// The calls' counts.
pub fn report() -> String {
    format!(
        "DLSS eye calls={} resets={} matrix failures={}",
        EYE_CALLS.load(Ordering::Relaxed),
        RESETS.load(Ordering::Relaxed),
        MATRIX_FAILURES.load(Ordering::Relaxed)
    )
}

// --- Each eye's previous camera -------------------------------------------------------------------

/// Which of the scene's camera writers a call's return address `rva` is (`writers`, in order), as a
/// bit; 0 for none.
pub fn writer_bit(writers: &[usize], rva: usize) -> u32 {
    writers.iter().position(|&r| r == rva).map_or(0, |i| 1 << i)
}

/// Every writer's bit, for `count` writers.
pub const fn all_writers(count: usize) -> u32 {
    (1u32 << count) - 1
}

fn bank<const N: usize>() -> Mutex<Bank<N>> {
    Mutex::new(Bank::new(32))
}

/// Each eye's own previous camera in a scene's camera history: its entries (`ENTRY` bytes each)
/// and a pass's previous-camera block (`BLOCK` bytes), banked per eye. A game keeps one as a static
/// and calls it from its scene's hooks.
pub struct EyeCameras<const ENTRY: usize, const BLOCK: usize> {
    scene_eye: AtomicU32,
    scene_key: AtomicU32,
    epoch: AtomicU32,
    reset: AtomicBool,
    /// Every eye drawn with the game's own history (a check).
    write_tree: AtomicUsize,
    write_mask: AtomicU32,
    camera: AtomicUsize,
    counter: AtomicU32,
    entries: LazyLock<Mutex<Bank<ENTRY>>, fn() -> Mutex<Bank<ENTRY>>>,
    blocks: LazyLock<Mutex<Bank<BLOCK>>, fn() -> Mutex<Bank<BLOCK>>>,
    commits: AtomicU64,
    incomplete: AtomicU64,
    reads: AtomicU64,
    swaps: AtomicU64,
}

impl<const ENTRY: usize, const BLOCK: usize> EyeCameras<ENTRY, BLOCK> {
    pub const fn new() -> Self {
        Self {
            scene_eye: AtomicU32::new(0),
            scene_key: AtomicU32::new(0),
            epoch: AtomicU32::new(1),
            reset: AtomicBool::new(true),
            write_tree: AtomicUsize::new(0),
            write_mask: AtomicU32::new(0),
            camera: AtomicUsize::new(0),
            counter: AtomicU32::new(0),
            entries: LazyLock::new(bank::<ENTRY>),
            blocks: LazyLock::new(bank::<BLOCK>),
            commits: AtomicU64::new(0),
            incomplete: AtomicU64::new(0),
            reads: AtomicU64::new(0),
            swaps: AtomicU64::new(0),
        }
    }


    /// The history epoch now: a new one starts each eye afresh.
    pub fn epoch(&self) -> u32 {
        self.epoch.load(Ordering::Acquire)
    }

    /// Start each eye's history afresh at the next left eye (a pause, a cut).
    pub fn start_over(&self) {
        self.reset.store(true, Ordering::Release);
    }

    /// Whether a fresh start is pending.
    pub fn starting_over(&self) -> bool {
        self.reset.load(Ordering::Acquire)
    }

    /// The eye whose history the lookups serve now (1 or 2; 0 the game's own).
    pub fn eye(&self) -> u32 {
        self.scene_eye.load(Ordering::Acquire)
    }

    /// A scene call starts (`eye` 1 or 2; 0 the game's own history) for the game's frame `counter`,
    /// keyed `token`, on `camera`: a new epoch when continuity breaks. Its eye.
    pub fn begin_scene(&self, counter: u32, token: u32, eye: u32, camera: usize) -> u32 {
        if eye == 1 && self.reset.swap(false, Ordering::AcqRel) {
            self.epoch.fetch_add(1, Ordering::AcqRel);
        }
        if eye != 0 {
            if self.camera.load(Ordering::Relaxed) != camera || counter.wrapping_sub(self.counter.load(Ordering::Relaxed)) > 1 {
                self.epoch.fetch_add(1, Ordering::AcqRel);
            }
            self.camera.store(camera, Ordering::Relaxed);
            self.counter.store(counter, Ordering::Relaxed);
        } else {
            self.camera.store(0, Ordering::Relaxed);
            self.epoch.fetch_add(1, Ordering::AcqRel);
        }
        self.write_tree.store(0, Ordering::Release);
        self.write_mask.store(0, Ordering::Release);
        self.scene_key.store(token, Ordering::Release);
        self.scene_eye.store(eye, Ordering::Release);
        eye
    }

    /// A lookup in the camera history `tree` with `key` returned `original` (an entry, or 0), from
    /// the caller whose writer bit is `writer` (0: a reader): the entry to give the caller. A
    /// writer's is the game's (its tree noted, banked after the scene); a reader's during an eye's
    /// scene is that eye's own.
    ///
    /// # Safety
    /// `original`, when not 0, is a live history entry of `ENTRY` bytes.
    pub unsafe fn lookup(&self, tree: usize, key: u32, original: usize, writer: u32) -> usize {
        let eye = self.eye();
        if original == 0 || !(1..=2).contains(&eye) || key != self.scene_key.load(Ordering::Acquire) {
            return original;
        }
        if writer != 0 {
            // Native writes stay visible to every consumer; the finished camera is banked after the scene.
            let first = self.write_tree.compare_exchange(0, tree, Ordering::AcqRel, Ordering::Acquire);
            if first.is_ok() || first == Err(tree) {
                self.write_mask.fetch_or(writer, Ordering::AcqRel);
            }
            return original;
        }
        let epoch = self.epoch.load(Ordering::Acquire);
        // SAFETY: the caller's guarantee.
        let native = unsafe { &*(original as *const [u8; ENTRY]) };
        let own = self.entries.locked().get(tree, key, epoch, eye as usize - 1, native).map(|own| own.as_mut_ptr());
        match own {
            Some(own) => {
                self.reads.fetch_add(1, Ordering::Relaxed);
                own as usize
            }
            None => original,
        }
    }

    /// A pass on `object` that reads and writes only its previous-camera block (`BLOCK` bytes at
    /// `offset`), run by `run`: during an eye's scene the block is that eye's own for the pass.
    ///
    /// # Safety
    /// `object + offset` holds `BLOCK` bytes the pass alone uses while it runs.
    pub unsafe fn around_pass(&self, object: usize, offset: usize, run: impl FnOnce() -> usize) -> usize {
        let eye = self.eye();
        if !(1..=2).contains(&eye) {
            return run();
        }
        let mut bank = self.blocks.locked();
        let region = (object + offset) as *mut u8;
        // SAFETY: the caller's guarantee; the swap is undone after the pass, under the bank's lock.
        unsafe {
            let saved = bank.get(object, self.scene_key.load(Ordering::Acquire), self.epoch.load(Ordering::Acquire), eye as usize - 1, &*(region as *const [u8; BLOCK])).map(|saved| saved.as_mut_ptr());
            let mut native = [0u8; BLOCK];
            if let Some(saved) = saved {
                std::ptr::copy_nonoverlapping(region, native.as_mut_ptr(), BLOCK);
                std::ptr::copy_nonoverlapping(saved, region, BLOCK);
            }
            let result = run();
            if let Some(saved) = saved {
                std::ptr::copy_nonoverlapping(region, saved, BLOCK);
                std::ptr::copy_nonoverlapping(native.as_ptr(), region, BLOCK);
                self.swaps.fetch_add(1, Ordering::Relaxed);
            }
            result
        }
    }

    /// The scene call (eye `eye`, keyed `token`) is over: the camera its writers wrote (`all`: every
    /// writer's bit) banked as this eye's previous camera for its next frame, found with `lookup`
    /// (the game's own lookup in the tree with the key); a scene whose writers did not all write
    /// starts each eye afresh.
    pub fn end_scene(&self, eye: u32, token: u32, all: u32, lookup: impl FnOnce(usize, &u32) -> usize) {
        self.scene_eye.store(0, Ordering::Release);
        if eye == 0 {
            return;
        }
        let tree = self.write_tree.load(Ordering::Acquire);
        if tree != 0 && self.write_mask.load(Ordering::Acquire) == all {
            let current = lookup(tree, &token);
            let epoch = self.epoch.load(Ordering::Acquire);
            // SAFETY: the lookup returns a camera history entry of `ENTRY` bytes (or null, refused).
            let stored = current != 0 && self.entries.locked().store(tree, token, epoch, eye as usize - 1, unsafe { &*(current as *const [u8; ENTRY]) });
            if stored {
                self.commits.fetch_add(1, Ordering::Relaxed);
            }
        } else {
            self.incomplete.fetch_add(1, Ordering::Relaxed);
            self.reset.store(true, Ordering::Release);
        }
    }

    /// The history's counts.
    pub fn report(&self) -> String {
        format!(
            "history: camera commits={} incomplete={} own reads={} external swaps={}",
            self.commits.load(Ordering::Relaxed),
            self.incomplete.load(Ordering::Relaxed),
            self.reads.load(Ordering::Relaxed),
            self.swaps.load(Ordering::Relaxed),
        )
    }
}

impl<const ENTRY: usize, const BLOCK: usize> Default for EyeCameras<ENTRY, BLOCK> {
    fn default() -> Self {
        Self::new()
    }
}
