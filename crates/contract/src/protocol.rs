//! Records exchanged through named shared memory between the game producers and the viewer.
//!
//! These layouts are a contract between producers and the viewer, which are built apart: field
//! order, sizes and offsets are `repr(C)`, the same for a 64-bit viewer and a 32-bit producer
//! (integers and floats only, no pointers, `usize` or `bool`), and the tests pin them.
//!
//! One channel name (for example `Local\monaka-<run>`) owns a family of named objects; see
//! [`ChannelName`] for the suffixes: the stereo channel's header ([`FenceHeader`]) and the blocks
//! beside it ([`Block`]).
//!
//! # Changing a record
//!
//! Every record starts with a [`Stamp`]: what it is, its major and minor version, and its size.
//! Within a major version a record only grows: a field is appended at its end (raising the
//! minor), never moved, retyped or given another meaning, and its zero means "not sent", which is
//! what a side that does not know the field leaves there. A mapping is made at the record's fixed
//! [`Versioned::CAPACITY`], whoever makes it, so an appended field always fits. A reader then
//! takes any minor of its major: what it knows is where it expects it, and what it does not know
//! it never reads. Anything else is a new major version of that record, which comes only with a
//! new major version of the contract ([`crate::pack::CONTRACT`]).
//!
//! A new kind of data is a new block beside the channel (a new [`Block`]), never a field in the
//! stereo header, which only hands pairs of eye images over: a side that does not know the block
//! never opens it. A block a side cannot open, or finds of another major version, turns off what
//! it carries and nothing else.

/// One writer's part of a record, read without a lock (a seqlock): the writer makes `seq` odd,
/// writes `value`, then makes `seq` even again; a reader keeps its copy of `value` only when `seq`
/// was even and the same before and after it. The pose, hand, finger and reticle blocks are made of
/// these, one per writer, so no game thread waits on the other process.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Seq<T> {
    pub seq: u64,
    pub value: T,
}

/// Windows adapter LUID (`LUID { DWORD LowPart; LONG HighPart; }`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Luid {
    pub low: u32,
    pub high: i32,
}

/// Eye gaze in head-relative OpenXR VIEW coordinates: +X right, +Y up, forward -Z.
///
/// The viewer fills it in with each head pose when the headset tracks the eyes (OpenXR's eye gaze
/// interaction, or the vendor's eye tracking); otherwise it stays [`EyeGaze::NONE`]. A gaze that is
/// valid but old (by `received_tick`) is a stale sample, not a live one.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EyeGaze {
    pub direction: [f32; 3],
    pub origin: [f32; 3],
    /// OpenXR time of the sample; zero means unavailable.
    pub sample_time: i64,
    /// `GetTickCount64` when the viewer read it, for freshness checks.
    pub received_tick: u64,
    pub valid: u32,
}

impl EyeGaze {
    /// No gaze: straight ahead, not valid.
    pub const NONE: Self = Self { direction: [0.0, 0.0, -1.0], origin: [0.0; 3], sample_time: 0, received_tick: 0, valid: 0 };
}

impl Default for EyeGaze {
    fn default() -> Self {
        Self::NONE
    }
}

/// Head pose in the viewer's LOCAL space (OpenXR conventions): orientation xyzw, position in
/// metres, per-eye field of view as left/right/up/down angles in radians.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HeadPose {
    pub orientation: [f32; 4],
    pub position: [f32; 3],
    pub ipd: f32,
    pub fov: [[f32; 4]; 2],
    pub valid: u32,
    pub gaze: EyeGaze,
}

impl HeadPose {
    /// No pose yet: level, at the origin, not valid (for statics; the same as `default()`).
    pub const EMPTY: Self = Self { orientation: [0.0, 0.0, 0.0, 1.0], position: [0.0; 3], ipd: 0.0, fov: [[0.0; 4]; 2], valid: 0, gaze: EyeGaze::NONE };

    /// A headset-like field of view per eye (left, right, up, down; radians), each eye reaching
    /// further outward than inward, for synthetic heads on PC runs.
    pub const SYNTHETIC_FOV: [[f32; 4]; 2] = [[-0.87, 0.70, 0.80, -0.80], [-0.70, 0.87, 0.80, -0.80]];

    /// The IPDs (metres) a headset report is believed within; anything else is a bad sample.
    pub const PLAUSIBLE_IPD: core::ops::RangeInclusive<f32> = 0.04..=0.09;

    /// The reported IPD if plausible, else `fallback` (a configured eye separation).
    pub fn ipd_or(&self, fallback: f32) -> f32 {
        if self.valid != 0 && Self::PLAUSIBLE_IPD.contains(&self.ipd) { self.ipd } else { fallback }
    }

    /// A headset-like pose for PC runs: turned `yaw` degrees left, then pitched `pitch` degrees
    /// up, at the origin, with field of view `fov` per eye and eye distance `ipd`.
    pub fn synthetic(yaw: f32, pitch: f32, fov: [[f32; 4]; 2], ipd: f32) -> Self {
        let (sy, cy) = (yaw.to_radians() * 0.5).sin_cos();
        let (sp, cp) = (pitch.to_radians() * 0.5).sin_cos();
        Self { orientation: [cy * sp, sy * cp, -sy * sp, cy * cp], position: [0.0; 3], ipd, fov, valid: 1, gaze: EyeGaze::default() }
    }
}

impl Default for HeadPose {
    fn default() -> Self {
        Self::EMPTY
    }
}

/// The head pose each eye image of one published stereo pair was rendered with.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RenderedPair {
    pub sequence: u64,
    pub eyes: [HeadPose; 2],
}

pub const POSE_MAGIC: u32 = 0x534f_5044;
/// Majors continue the versions records had before their stamps had a minor, so an older side
/// never takes a newer record for its own.
pub const POSE_MAJOR: u16 = 4;
pub const POSE_PAIR_SLOTS: usize = 16;

/// The pose block beside a stereo channel. The viewer writes the latest head pose; the producer
/// writes the pose behind each published pair, in a ring indexed by sequence (each a [`Seq`]).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PoseBlock {
    pub stamp: Stamp,
    pub head: Seq<HeadPose>,
    pub pairs: [Seq<RenderedPair>; POSE_PAIR_SLOTS],
    /// The headset's refresh as the viewer sees it (minor 1; zero: not sent).
    pub display: Seq<DisplayTiming>,
}

/// The headset's refresh, written by the viewer each time its frame wait returns: how long a
/// headset frame is, and the moment that wait returned (both in nanoseconds of the Windows
/// performance counter, which every process reads alike: `monaka_channel::pose::now_ns`). A
/// producer can start its frames on that grid, so each headset frame gets one new pair (or one
/// every few) instead of drifting against it.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DisplayTiming {
    pub period_ns: u64,
    pub woke_ns: u64,
}

impl PoseBlock {
    pub fn fresh() -> Self {
        Self {
            stamp: Stamp::of::<Self>(),
            head: Seq::default(),
            pairs: [Seq::default(); POSE_PAIR_SLOTS],
            display: Seq::default(),
        }
    }

    pub fn pair_slot(sequence: u64) -> usize {
        (sequence % POSE_PAIR_SLOTS as u64) as usize
    }
}

impl Versioned for PoseBlock {
    const MAGIC: u32 = POSE_MAGIC;
    const MAJOR: u16 = POSE_MAJOR;
    const MINOR: u16 = 1;
    const WHAT: &'static str = "head pose block";
    const CAPACITY: usize = 16 * 1024;

    fn stamp(&self) -> Stamp {
        self.stamp
    }

    fn start(&mut self) {
        *self = Self::fresh();
    }
}

/// One tracked hand pose in the same LOCAL space as [`HeadPose`]: OpenXR's aim pose (where a
/// controller points, forward -Z) or its grip pose (origin in the palm, -Z along the straightened
/// index finger as when holding a pistol).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HandPose {
    pub orientation: [f32; 4],
    pub position: [f32; 3],
    pub valid: u32,
    /// `GetTickCount64` when the viewer located it, for freshness checks (controllers sleep).
    pub received_tick: u64,
}

impl HandPose {
    /// The palm: the grip pose's position (the controller's handle, the middle of the palm) with
    /// the aim pose's direction (where a relaxed hand points; the grip's own forward runs along a
    /// straightened index finger, which bent the wrist to aim level). Either alone stands in for
    /// both.
    pub fn palm(aim: Option<HandPose>, grip: Option<HandPose>) -> Option<HandPose> {
        match (aim, grip) {
            (Some(aim), Some(grip)) => Some(HandPose { position: grip.position, ..aim }),
            (aim, grip) => aim.or(grip),
        }
    }
}

impl Default for HandPose {
    fn default() -> Self {
        Self { orientation: [0.0, 0.0, 0.0, 1.0], position: [0.0; 3], valid: 0, received_tick: 0 }
    }
}

pub const HANDS_MAGIC: u32 = 0x444e_4148;
pub const HANDS_MAJOR: u16 = 5;
pub const LEFT_HAND: usize = 0;
pub const RIGHT_HAND: usize = 1;

pub const FINGERS_MAGIC: u32 = 0x4752_4946;
pub const FINGERS_MAJOR: u16 = 3;

pub const RETICLES_MAGIC: u32 = 0x4c43_5452;
pub const RETICLES_MAJOR: u16 = 3;

/// Where a hand's reticle points instead of the palm's forward, as the producer last said: a
/// direction in the palm's frame ([`HandPose::palm`]: forward -Z), e.g. along a bow held in it.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ReticleAim {
    pub valid: u32,
    pub _reserved: u32,
    pub direction: [f32; 3],
    pub _pad: f32,
    /// `GetTickCount64` when the producer wrote it.
    pub written_tick: u64,
}

/// The reticle block beside a stereo channel: a producer writes where each hand's reticle points
/// when it is not along the palm; the viewer reads it. Either side may create it; a producer that
/// never writes it leaves the reticles on the palms.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ReticleBlock {
    pub stamp: Stamp,
    /// Each hand's, written on its own ([`Seq`]).
    pub aims: [Seq<ReticleAim>; 2],
}

impl ReticleBlock {
    pub fn fresh() -> Self {
        Self { stamp: Stamp::of::<Self>(), aims: [Seq::default(); 2] }
    }
}

impl Versioned for ReticleBlock {
    const MAGIC: u32 = RETICLES_MAGIC;
    const MAJOR: u16 = RETICLES_MAJOR;
    const MINOR: u16 = 0;
    const WHAT: &'static str = "reticle block";
    const CAPACITY: usize = 4 * 1024;

    fn stamp(&self) -> Stamp {
        self.stamp
    }

    fn start(&mut self) {
        *self = Self::fresh();
    }
}

/// One hand's fingers as the viewer last tracked them (`monaka_core::fingers::bends`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FingerPose {
    pub valid: u32,
    pub _reserved: u32,
    /// `GetTickCount64` when the viewer located them.
    pub received_tick: u64,
    /// Per finger (thumb, index, middle, ring, little): the bends of its three knuckles toward
    /// the palm, from the hand out, and the spread of the first toward the thumb (radians).
    pub fingers: [[f32; 4]; 5],
}

/// The finger block beside a stereo channel, next to the hand block (its own block, so the hand
/// block's layout stays as it is): both hands' fingers when the headset tracks them.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FingerBlock {
    pub stamp: Stamp,
    /// Both hands', written together ([`Seq`]).
    pub hands: Seq<[FingerPose; 2]>,
}

impl FingerBlock {
    pub fn fresh() -> Self {
        Self { stamp: Stamp::of::<Self>(), hands: Seq::default() }
    }
}

impl Versioned for FingerBlock {
    const MAGIC: u32 = FINGERS_MAGIC;
    const MAJOR: u16 = FINGERS_MAJOR;
    const MINOR: u16 = 0;
    const WHAT: &'static str = "finger block";
    const CAPACITY: usize = 4 * 1024;

    fn stamp(&self) -> Stamp {
        self.stamp
    }

    fn start(&mut self) {
        *self = Self::fresh();
    }
}

/// The hand block beside a stereo channel. The viewer writes both hands every frame it can locate
/// them (`poses`), and the controllers' buttons, triggers and sticks as one gamepad (`pad`), each a
/// [`Seq`].
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HandBlock {
    pub stamp: Stamp,
    pub poses: Seq<HandPoses>,
    pub pad: Seq<PadState>,
}

/// Both hands' poses, written together: the aim poses in `hands`, the grip poses in `grips`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HandPoses {
    pub hands: [HandPose; 2],
    pub grips: [HandPose; 2],
}

/// The VR controllers as one gamepad ([`crate::gamepad`]), when the viewer last read them.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PadState {
    pub gamepad: crate::gamepad::Gamepad,
    pub valid: u32,
    /// `GetTickCount64` when the viewer read it.
    pub received_tick: u64,
}

impl HandBlock {
    pub fn fresh() -> Self {
        Self { stamp: Stamp::of::<Self>(), poses: Seq::default(), pad: Seq::default() }
    }
}

impl Versioned for HandBlock {
    const MAGIC: u32 = HANDS_MAGIC;
    const MAJOR: u16 = HANDS_MAJOR;
    const MINOR: u16 = 0;
    const WHAT: &'static str = "hand block";
    const CAPACITY: usize = 4 * 1024;

    fn stamp(&self) -> Stamp {
        self.stamp
    }

    fn start(&mut self) {
        *self = Self::fresh();
    }
}

/// The first 16 bytes of every channel header and block: which record it is, which layout, and how
/// much of it its writer's layout fills. Read alone, it tells a record of another layout from no
/// record at all, whatever that layout's size. (Before contract 4 a stamp was `magic` and a 32-bit
/// `version`; a record's major continues that version, so the two never read alike.)
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stamp {
    pub magic: u32,
    /// Changes when the record changes in place: a reader of another major cannot read it.
    pub major: u16,
    /// Rises with each field appended within the major ("Changing a record").
    pub minor: u16,
    /// The bytes the writer's layout fills (`size_of` there).
    pub size: u32,
    pub _reserved: u32,
}

impl Stamp {
    /// The stamp of a fresh `T` written by this side.
    pub const fn of<T: Versioned>() -> Self {
        Self { magic: T::MAGIC, major: T::MAJOR, minor: T::MINOR, size: size_of::<T>() as u32, _reserved: 0 }
    }
}

/// A shared-memory record that starts with its [`Stamp`]: which record it is and which layout.
pub trait Versioned: Copy {
    const MAGIC: u32;
    const MAJOR: u16;
    const MINOR: u16;
    /// What it is, for a version mismatch ("finger block").
    const WHAT: &'static str;
    /// The bytes its mapping has, whoever makes it: fixed for its major version and larger than
    /// the record, so the fields a later minor appends fit in a mapping an earlier side made.
    const CAPACITY: usize;

    /// Its stamp as written.
    fn stamp(&self) -> Stamp;

    /// Makes a blank record (all zeros, as a new mapping is) a fresh one, in place.
    fn start(&mut self);

    /// All zeros: nobody has written it yet.
    fn is_blank(&self) -> bool {
        self.stamp().magic == 0
    }

    /// Written in a layout this side reads: this record at this major, any minor.
    fn is_compatible(&self) -> bool {
        let stamp = self.stamp();
        stamp.magic == Self::MAGIC && stamp.major == Self::MAJOR
    }
}

/// A record whose kind this side knows but whose layout it does not: the game pack and this
/// Monaka were built for different major versions of the contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mismatch {
    pub what: &'static str,
    pub found: u16,
    pub expected: u16,
}

impl Mismatch {
    /// The mismatch `stamp` shows for a record of `magic` at major `major`, if it is that record at
    /// another major (no record, another kind of record, or another minor is no mismatch).
    pub fn of(stamp: Stamp, what: &'static str, magic: u32, major: u16) -> Option<Self> {
        (stamp.magic == magic && stamp.major != major).then_some(Self { what, found: stamp.major, expected: major })
    }
}

impl core::fmt::Display for Mismatch {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "the game's {} is version {}, this Monaka reads version {}: the game pack was built for a different Monaka (update the pack or Monaka)",
            self.what, self.found, self.expected
        )
    }
}

pub const FENCE_MAGIC: u32 = 0x4656_4c44;
/// Before 4: 2 gave each slot its own sequence (the reader takes the newest finished pair, not
/// only the newest), 3 put each eye's depth in the header, which 4 moved to its own block
/// ([`DepthBlock`]).
pub const FENCE_MAJOR: u16 = 4;
pub const FENCE_SLOTS: usize = 5;
pub const NO_SLOT: u32 = u32::MAX;

/// How a depth image maps to distance: the near and far planes' distances in metres (`far` may be
/// infinite), and whether depth runs from 1 at the near plane to 0 at the far one (reversed) rather
/// than from 0 to 1.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DepthParams {
    pub near: f32,
    pub far: f32,
    pub reversed: u32,
    pub _reserved: u32,
}

impl DepthParams {
    pub fn new(near: f32, far: f32, reversed: bool) -> Self {
        Self { near, far, reversed: reversed as u32, _reserved: 0 }
    }

    /// A positive, finite near plane in front of a farther (or infinite) far plane.
    pub fn is_valid(&self) -> bool {
        self.near > 0.0 && self.near.is_finite() && self.far > self.near
    }

    /// The distances at depth 0 and at depth 1 (OpenXR's `nearZ` and `farZ` for a depth range of
    /// 0 to 1): reversed depth swaps them, so an infinite far plane is the distance at 0.
    pub fn distances_at_0_and_1(&self) -> (f32, f32) {
        if self.reversed != 0 { (self.far, self.near) } else { (self.near, self.far) }
    }
}

/// The depth one fence slot holds beside its eye images.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DepthSlot {
    /// The pair this depth belongs to: when it is not the slot's pair, the pair came without depth.
    pub sequence: u64,
    pub params: DepthParams,
    /// Per eye, the part of the depth texture that lines up with the eye image: left, top, width,
    /// height, in texels.
    pub rects: [[u32; 4]; 2],
}

pub const DEPTH_MAGIC: u32 = 0x4854_5044;
pub const DEPTH_MAJOR: u16 = 1;

/// The depth block beside a stereo channel: per slot of the channel a depth texture for each eye,
/// made by the producer when it first sends depth (and made anew when the depth's size or format
/// changes), and which pair each slot's depth belongs to. Either side may create it; a viewer that
/// never opens it shows the images without depth.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DepthBlock {
    pub stamp: Stamp,
    /// Which set of textures `textures` names: 0 for none yet, one more each time they are made.
    pub generation: u32,
    /// The depth textures' typeless `DXGI_FORMAT` ([`depth_slot_format`]).
    pub format: u32,
    pub width: u32,
    pub height: u32,
    pub textures: [[u64; 2]; FENCE_SLOTS],
    /// Per slot of the channel, the depth of the pair it holds (when `sequence` is that pair's).
    pub slots: [DepthSlot; FENCE_SLOTS],
}

impl DepthBlock {
    pub fn fresh() -> Self {
        Self { stamp: Stamp::of::<Self>(), ..Self::default() }
    }
}

impl Versioned for DepthBlock {
    const MAGIC: u32 = DEPTH_MAGIC;
    const MAJOR: u16 = DEPTH_MAJOR;
    const MINOR: u16 = 0;
    const WHAT: &'static str = "depth block";
    const CAPACITY: usize = 4 * 1024;

    fn stamp(&self) -> Stamp {
        self.stamp
    }

    fn start(&mut self) {
        *self = Self::fresh();
    }
}

/// Header of the fence stereo channel (a D3D12 or D3D11 producer, a D3D11 reader):
/// [`FENCE_SLOTS`] slots of paired eye textures and two shared fences. Handle values are valid in
/// the producer process; the reader duplicates them. It only hands pairs of eye images over and
/// does not grow: what comes with a pair is a block beside it (the module's "Changing a record").
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FenceHeader {
    pub stamp: Stamp,
    pub width: u32,
    pub height: u32,
    /// A `DXGI_FORMAT` value; see [`fence_format_ok`].
    pub format: u32,
    pub producer_pid: u32,
    pub adapter: Luid,
    pub textures: [[u64; 2]; FENCE_SLOTS],
    pub produced: u64,
    pub consumed: u64,
    pub sequence: u64,
    pub tick: u64,
    pub latest_slot: u32,
    pub reader_slot: u32,
    pub reader_claim: u64,
    /// The pair each slot holds or is being written with (0: none yet), and its tick.
    pub slot_sequences: [u64; FENCE_SLOTS],
    pub slot_ticks: [u64; FENCE_SLOTS],
}

impl FenceHeader {
    pub fn is_valid(&self) -> bool {
        self.is_compatible() && dimensions_ok(self.width, self.height) && fence_format_ok(self.format)
    }
}

impl Versioned for FenceHeader {
    const MAGIC: u32 = FENCE_MAGIC;
    const MAJOR: u16 = FENCE_MAJOR;
    const MINOR: u16 = 0;
    const WHAT: &'static str = "stereo channel";
    const CAPACITY: usize = 4 * 1024;

    fn stamp(&self) -> Stamp {
        self.stamp
    }

    fn start(&mut self) {
        *self = Self { stamp: Stamp::of::<Self>(), latest_slot: NO_SLOT, reader_slot: NO_SLOT, ..Self::default() };
    }
}

/// Eye images are at most 8192 pixels on a side.
pub fn dimensions_ok(width: u32, height: u32) -> bool {
    width > 0 && height > 0 && width <= 8192 && height <= 8192
}

/// `DXGI_FORMAT` values the channels use, so this crate needs no Windows bindings.
pub mod dxgi_format {
    pub const R10G10B10A2_TYPELESS: u32 = 23;
    pub const R10G10B10A2_UNORM: u32 = 24;
    pub const R8G8B8A8_TYPELESS: u32 = 27;
    pub const R8G8B8A8_UNORM: u32 = 28;
    pub const R8G8B8A8_UNORM_SRGB: u32 = 29;
    pub const B8G8R8A8_UNORM: u32 = 87;
    pub const B8G8R8A8_TYPELESS: u32 = 90;
    pub const B8G8R8A8_UNORM_SRGB: u32 = 91;
    pub const R32G8X24_TYPELESS: u32 = 19;
    pub const D32_FLOAT_S8X24_UINT: u32 = 20;
    pub const R32_FLOAT_X8X24_TYPELESS: u32 = 21;
    pub const R32_TYPELESS: u32 = 39;
    pub const D32_FLOAT: u32 = 40;
    pub const R32_FLOAT: u32 = 41;
    pub const D24_UNORM_S8_UINT: u32 = 45;
    pub const R16_TYPELESS: u32 = 53;
    pub const D16_UNORM: u32 = 55;
    pub const R16_UNORM: u32 = 56;
}

/// Formats a producer may publish through the fence channel (the game's back buffer format).
pub fn fence_format_ok(format: u32) -> bool {
    use dxgi_format::*;
    matches!(format, R8G8B8A8_UNORM | B8G8R8A8_UNORM | R10G10B10A2_UNORM)
}

/// The fence channel format that carries images of `format`: its family's plain UNORM member, so
/// typeless and sRGB images are copied in as stored. `None`: a family the channel does not carry.
pub fn fence_format_of(format: u32) -> Option<u32> {
    use dxgi_format::*;
    match format {
        R8G8B8A8_TYPELESS | R8G8B8A8_UNORM | R8G8B8A8_UNORM_SRGB => Some(R8G8B8A8_UNORM),
        B8G8R8A8_TYPELESS | B8G8R8A8_UNORM | B8G8R8A8_UNORM_SRGB => Some(B8G8R8A8_UNORM),
        R10G10B10A2_TYPELESS | R10G10B10A2_UNORM => Some(R10G10B10A2_UNORM),
        _ => None,
    }
}

/// The typeless format of the fence channel's depth textures for a game depth texture of `format`:
/// its depth plane is copied as it is, so 32-bit float depth (with or without stencil) travels as
/// R32 and 16-bit depth as R16. 24-bit depth is not copyable that way; `None`.
pub fn depth_slot_format(format: u32) -> Option<u32> {
    use dxgi_format::*;
    match format {
        R32_TYPELESS | D32_FLOAT | R32_FLOAT | R32G8X24_TYPELESS | D32_FLOAT_S8X24_UINT | R32_FLOAT_X8X24_TYPELESS => Some(R32_TYPELESS),
        R16_TYPELESS | D16_UNORM | R16_UNORM => Some(R16_TYPELESS),
        _ => None,
    }
}

/// The run folder's file naming the run's channel (a `Local\monaka-` name), which the launcher
/// writes before the producer starts.
pub const LIVE_CHANNEL_FILE: &str = "live-channel.txt";

/// A validated channel name and the names of the objects that belong to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelName(String);

impl ChannelName {
    /// Longest accepted name; leaves room for the suffixes inside Windows' object name limit.
    pub const MAX_LEN: usize = 200;

    pub fn new(name: impl Into<String>) -> Option<Self> {
        let name = name.into();
        (!name.is_empty() && name.chars().count() <= Self::MAX_LEN).then_some(Self(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The header mapping of a GPU channel.
    pub fn header(&self) -> String {
        self.0.clone()
    }

    /// Mutex guarding the header.
    pub fn header_lock(&self) -> String {
        format!("{}-lock", self.0)
    }

    /// Mutex a reader holds for its lifetime; a second reader is refused.
    pub fn reader_lease(&self) -> String {
        format!("{}-reader", self.0)
    }

    /// The channel of one HUD piece's panel (`<channel>-panel-<piece>`; the viewer places it
    /// on a hand), if the name fits.
    pub fn panel(&self, piece: &str) -> Option<Self> {
        Self::new(format!("{}-panel-{piece}", self.0))
    }

    /// The game's depth shared for the debug views ([`crate::debug_draw::DebugDepth`]).
    pub fn debug_depth(&self) -> String {
        format!("{}-debug-depth", self.0)
    }

    /// One of the game's render targets shared for the debug views.
    pub fn debug_target(&self) -> String {
        format!("{}-debug-target", self.0)
    }

    /// Block `block`'s mapping (`<channel>-<block>`) and the mutex guarding it (`...-lock`).
    pub fn block(&self, block: Block) -> (String, String) {
        let name = format!("{}-{}", self.0, block.suffix());
        let lock = format!("{name}-lock");
        (name, lock)
    }
}

/// A channel's shared-memory blocks besides its stereo header, each beside the channel's name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Block {
    /// Head poses in, rendered-pair poses out ([`PoseBlock`]).
    Pose,
    Hands,
    Fingers,
    Reticles,
    /// The debug block ([`crate::debug_draw::DebugBlock`]).
    Debug,
    /// The game page ([`crate::page::PageBlock`]).
    Page,
    Console,
    /// The viewer's own console (the 2D window's command line), beside the producer's.
    ViewConsole,
    /// The HUD pieces the producer puts on the hands ([`crate::panels::PanelsBlock`]).
    Panels,
    /// Each eye's depth beside its image ([`DepthBlock`]).
    Depth,
}

impl Block {
    const fn suffix(self) -> &'static str {
        match self {
            Block::Pose => "pose",
            Block::Hands => "hands",
            Block::Fingers => "fingers",
            Block::Reticles => "reticles",
            Block::Debug => "debug",
            Block::Page => "page",
            Block::Console => "console",
            Block::ViewConsole => "view-console",
            Block::Panels => "panels",
            Block::Depth => "depth",
        }
    }
}

// Every record fits the mapping made for it.
const _: () = {
    assert!(size_of::<PoseBlock>() <= PoseBlock::CAPACITY);
    assert!(size_of::<HandBlock>() <= HandBlock::CAPACITY);
    assert!(size_of::<FingerBlock>() <= FingerBlock::CAPACITY);
    assert!(size_of::<ReticleBlock>() <= ReticleBlock::CAPACITY);
    assert!(size_of::<DepthBlock>() <= DepthBlock::CAPACITY);
    assert!(size_of::<FenceHeader>() <= FenceHeader::CAPACITY);
    assert!(size_of::<crate::panels::PanelsBlock>() <= crate::panels::PanelsBlock::CAPACITY);
    assert!(size_of::<crate::page::PageBlock>() <= crate::page::PageBlock::CAPACITY);
    assert!(size_of::<crate::console::ConsoleBlock>() <= crate::console::ConsoleBlock::CAPACITY);
    assert!(size_of::<crate::debug_draw::DebugBlock>() <= crate::debug_draw::DebugBlock::CAPACITY);
};

#[cfg(test)]
mod tests;
