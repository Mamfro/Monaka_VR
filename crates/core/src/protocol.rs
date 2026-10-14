//! Records exchanged through named shared memory between the game producers and the viewer.
//!
//! These layouts are a contract between producers and the viewer, which are built apart (and
//! were first shared with C++ producers): field order, sizes and offsets must match MSVC x64
//! exactly, and the tests at the bottom pin them. Changing a record means bumping its version on
//! both sides.
//!
//! One channel name (for example `Local\monaka-<run>`) owns a family of named objects; see
//! [`ChannelName`] for the suffixes.

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
pub const POSE_VERSION: u32 = 2;
pub const POSE_PAIR_SLOTS: usize = 16;

/// The pose block beside a stereo channel. The viewer writes the latest head pose; the producer
/// writes the pose behind each published pair, in a ring indexed by sequence.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PoseBlock {
    pub magic: u32,
    pub version: u32,
    pub head: HeadPose,
    pub pairs: [RenderedPair; POSE_PAIR_SLOTS],
}

impl PoseBlock {
    pub fn fresh() -> Self {
        Self {
            magic: POSE_MAGIC,
            version: POSE_VERSION,
            head: HeadPose::default(),
            pairs: [RenderedPair::default(); POSE_PAIR_SLOTS],
        }
    }

    /// A block nobody has initialised yet (shared memory starts zeroed).
    pub fn is_blank(&self) -> bool {
        self.magic == 0
    }

    pub fn is_compatible(&self) -> bool {
        self.magic == POSE_MAGIC && self.version == POSE_VERSION
    }

    pub fn pair_slot(sequence: u64) -> usize {
        (sequence % POSE_PAIR_SLOTS as u64) as usize
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
pub const HANDS_VERSION: u32 = 3;
pub const LEFT_HAND: usize = 0;
pub const RIGHT_HAND: usize = 1;

pub const FINGERS_MAGIC: u32 = 0x4752_4946;
pub const FINGERS_VERSION: u32 = 1;

pub const RETICLES_MAGIC: u32 = 0x4c43_5452;
pub const RETICLES_VERSION: u32 = 1;

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
    pub magic: u32,
    pub version: u32,
    pub aims: [ReticleAim; 2],
}

impl ReticleBlock {
    pub fn fresh() -> Self {
        Self { magic: RETICLES_MAGIC, version: RETICLES_VERSION, aims: [ReticleAim::default(); 2] }
    }

    pub fn is_blank(&self) -> bool {
        self.magic == 0
    }

    pub fn is_compatible(&self) -> bool {
        self.magic == RETICLES_MAGIC && self.version == RETICLES_VERSION
    }
}

/// One hand's fingers as the viewer last tracked them ([`crate::fingers::bends`]).
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
    pub magic: u32,
    pub version: u32,
    pub hands: [FingerPose; 2],
}

impl FingerBlock {
    pub fn fresh() -> Self {
        Self { magic: FINGERS_MAGIC, version: FINGERS_VERSION, hands: [FingerPose::default(); 2] }
    }

    pub fn is_blank(&self) -> bool {
        self.magic == 0
    }

    pub fn is_compatible(&self) -> bool {
        self.magic == FINGERS_MAGIC && self.version == FINGERS_VERSION
    }
}

/// The hand block beside a stereo channel (Rust only; the C++ producers never open it). The
/// viewer writes both hands every frame it can locate them: the aim poses in `hands`, the grip
/// poses in `grips` (version 2), and the controllers' buttons, triggers and sticks as one
/// gamepad in `pad` (version 3).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HandBlock {
    pub magic: u32,
    pub version: u32,
    pub hands: [HandPose; 2],
    pub grips: [HandPose; 2],
    pub pad: PadState,
}

/// The VR controllers as one gamepad ([`crate::pad`]), when the viewer last read them.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PadState {
    pub gamepad: crate::pad::Gamepad,
    pub valid: u32,
    /// `GetTickCount64` when the viewer read it.
    pub received_tick: u64,
}

impl HandBlock {
    pub fn fresh() -> Self {
        Self { magic: HANDS_MAGIC, version: HANDS_VERSION, hands: [HandPose::default(); 2], grips: [HandPose::default(); 2], pad: PadState::default() }
    }

    pub fn is_blank(&self) -> bool {
        self.magic == 0
    }

    pub fn is_compatible(&self) -> bool {
        self.magic == HANDS_MAGIC && self.version == HANDS_VERSION
    }
}

pub const KEYED_MAGIC: u32 = 0x4756_4c44;
pub const KEYED_VERSION: u32 = 1;
/// Keyed-mutex key meaning "the writer may fill the pair".
pub const KEY_FREE: u64 = 0;
/// Keyed-mutex key meaning "an unread pair is waiting".
pub const KEY_UNREAD: u64 = 1;

/// Header of the D3D11 keyed-mutex stereo channel (one D3D11 producer, one D3D11 reader). The
/// two eye textures are named NT handles beside it.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyedHeader {
    pub magic: u32,
    pub version: u32,
    pub width: u32,
    pub height: u32,
    pub adapter: Luid,
    pub sequence: u64,
    pub tick: u64,
}

impl KeyedHeader {
    pub fn is_valid(&self) -> bool {
        self.magic == KEYED_MAGIC && self.version == KEYED_VERSION && dimensions_ok(self.width, self.height)
    }
}

pub const FENCE_MAGIC: u32 = 0x4656_4c44;
/// 2: per-slot sequences, five slots. The reader takes the newest finished pair, not only the
/// newest: with a GPU running frames behind, the newest pair is still unfinished whenever the
/// reader looks, and most pairs would never be shown.
/// 3: each eye's depth beside its image, when the producer sends it ([`FenceDepth`]).
pub const FENCE_VERSION: u32 = 3;
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

/// The depth part of the fence header: per slot a depth texture for each eye, made by the producer
/// when it first sends depth (and made anew when the depth's size or format changes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FenceDepth {
    /// Which set of textures `textures` names: 0 for none yet, one more each time they are made.
    pub generation: u32,
    /// The depth textures' typeless `DXGI_FORMAT` ([`depth_slot_format`]).
    pub format: u32,
    pub width: u32,
    pub height: u32,
    pub textures: [[u64; 2]; FENCE_SLOTS],
    pub slots: [DepthSlot; FENCE_SLOTS],
}

/// Header of the D3D12-to-D3D11 fence stereo channel: [`FENCE_SLOTS`] slots of paired eye textures
/// and two shared fences. Handle values are valid in the producer process; the reader duplicates
/// them.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FenceHeader {
    pub magic: u32,
    pub version: u32,
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
    pub depth: FenceDepth,
}

impl FenceHeader {
    pub fn is_valid(&self) -> bool {
        self.magic == FENCE_MAGIC
            && self.version == FENCE_VERSION
            && dimensions_ok(self.width, self.height)
            && fence_format_ok(self.format)
    }
}

/// Eye images are at most 8192 pixels on a side.
pub fn dimensions_ok(width: u32, height: u32) -> bool {
    width > 0 && height > 0 && width <= 8192 && height <= 8192
}

/// `DXGI_FORMAT` values the channels use, so this crate needs no Windows bindings.
pub mod dxgi_format {
    pub const R10G10B10A2_UNORM: u32 = 24;
    pub const R8G8B8A8_UNORM: u32 = 28;
    pub const R8G8B8A8_UNORM_SRGB: u32 = 29;
    pub const B8G8R8A8_UNORM: u32 = 87;
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

/// Formats a D3D12 producer may publish through the fence channel (the game's back buffer format).
pub fn fence_format_ok(format: u32) -> bool {
    use dxgi_format::*;
    matches!(format, R8G8B8A8_UNORM | B8G8R8A8_UNORM | R10G10B10A2_UNORM)
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

    /// Named NT handle of one eye texture (keyed-mutex channel).
    pub fn eye_texture(&self, eye: usize) -> String {
        format!("{}-{}", self.0, if eye == 0 { "left" } else { "right" })
    }

    pub fn pose(&self) -> String {
        format!("{}-pose", self.0)
    }

    pub fn pose_lock(&self) -> String {
        format!("{}-pose-lock", self.0)
    }

    pub fn hands(&self) -> String {
        format!("{}-hands", self.0)
    }

    pub fn hands_lock(&self) -> String {
        format!("{}-hands-lock", self.0)
    }

    pub fn fingers(&self) -> String {
        format!("{}-fingers", self.0)
    }

    pub fn fingers_lock(&self) -> String {
        format!("{}-fingers-lock", self.0)
    }

    pub fn reticles(&self) -> String {
        format!("{}-reticles", self.0)
    }

    pub fn reticles_lock(&self) -> String {
        format!("{}-reticles-lock", self.0)
    }

    /// The debug block ([`crate::debug_draw::DebugBlock`]).
    pub fn debug(&self) -> String {
        format!("{}-debug", self.0)
    }

    pub fn debug_lock(&self) -> String {
        format!("{}-debug-lock", self.0)
    }

    /// The game's depth shared for the debug views ([`crate::debug_draw::DebugDepth`]).
    pub fn debug_depth(&self) -> String {
        format!("{}-debug-depth", self.0)
    }

    /// One of the game's render targets shared for the debug views.
    pub fn debug_target(&self) -> String {
        format!("{}-debug-target", self.0)
    }

    /// The game page ([`crate::page::PageBlock`]).
    pub fn page(&self) -> String {
        format!("{}-page", self.0)
    }

    pub fn page_lock(&self) -> String {
        format!("{}-page-lock", self.0)
    }

    pub fn console(&self) -> String {
        format!("{}-console", self.0)
    }

    pub fn console_lock(&self) -> String {
        format!("{}-console-lock", self.0)
    }

    /// The viewer's own console (the 2D window's command line), beside the producer's.
    pub fn view_console(&self) -> String {
        format!("{}-view-console", self.0)
    }

    pub fn view_console_lock(&self) -> String {
        format!("{}-view-console-lock", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::{align_of, offset_of, size_of};

    // The C++ headers' sizes and offsets under MSVC x64 (sizeof/offsetof).
    #[test]
    fn eye_gaze_matches_cpp() {
        assert_eq!(size_of::<EyeGaze>(), 48);
        assert_eq!(align_of::<EyeGaze>(), 8);
        assert_eq!(offset_of!(EyeGaze, sample_time), 24);
        assert_eq!(offset_of!(EyeGaze, received_tick), 32);
        assert_eq!(offset_of!(EyeGaze, valid), 40);
    }

    #[test]
    fn head_pose_matches_cpp() {
        assert_eq!(size_of::<HeadPose>(), 120);
        assert_eq!(offset_of!(HeadPose, ipd), 28);
        assert_eq!(offset_of!(HeadPose, fov), 32);
        assert_eq!(offset_of!(HeadPose, valid), 64);
        assert_eq!(offset_of!(HeadPose, gaze), 72);
        assert_eq!(size_of::<RenderedPair>(), 248);
        assert_eq!(offset_of!(RenderedPair, eyes), 8);
    }

    #[test]
    fn pose_block_matches_cpp() {
        assert_eq!(size_of::<PoseBlock>(), 4096);
        assert_eq!(offset_of!(PoseBlock, head), 8);
        assert_eq!(offset_of!(PoseBlock, pairs), 128);
    }

    #[test]
    fn synthetic_pose_and_ipd() {
        let fov = [[-0.9, 0.8, 0.8, -0.9]; 2];
        let pose = HeadPose::synthetic(30.0, 0.0, fov, 0.064);
        assert!((crate::math::yaw(pose.orientation) - 30f32.to_radians()).abs() < 1e-5);
        let (yaw, pitch) = crate::camera::head_yaw_pitch(HeadPose::synthetic(30.0, 20.0, fov, 0.064).orientation);
        assert!((yaw - 30f32.to_radians()).abs() < 1e-5 && (pitch - 20f32.to_radians()).abs() < 1e-5);
        assert_eq!(pose.ipd_or(0.07), 0.064);
        assert_eq!(HeadPose { ipd: 0.2, ..pose }.ipd_or(0.07), 0.07);
        assert_eq!(HeadPose { valid: 0, ..pose }.ipd_or(0.07), 0.07);
    }

    #[test]
    fn finger_block_layout() {
        assert_eq!(size_of::<FingerPose>(), 96);
        assert_eq!(offset_of!(FingerPose, fingers), 16);
        assert_eq!(size_of::<FingerBlock>(), 200);
        assert_eq!(offset_of!(FingerBlock, hands), 8);
        assert!(FingerBlock::fresh().is_compatible() && !FingerBlock::fresh().is_blank());
    }

    #[test]
    fn hand_block_layout() {
        assert_eq!(size_of::<HandPose>(), 40);
        assert_eq!(offset_of!(HandPose, received_tick), 32);
        assert_eq!(size_of::<HandBlock>(), 192);
        assert_eq!(offset_of!(HandBlock, hands), 8);
        assert_eq!(offset_of!(HandBlock, grips), 88);
        assert_eq!(offset_of!(HandBlock, pad), 168);
        assert_eq!(size_of::<PadState>(), 24);
        assert!(HandBlock::fresh().is_compatible() && !HandBlock::fresh().is_blank());
    }

    #[test]
    fn keyed_header_matches_cpp() {
        assert_eq!(size_of::<KeyedHeader>(), 40);
        assert_eq!(offset_of!(KeyedHeader, adapter), 16);
        assert_eq!(offset_of!(KeyedHeader, sequence), 24);
        assert_eq!(offset_of!(KeyedHeader, tick), 32);
    }

    /// Version 3's layout: version 2's 240 bytes, then the depth part (version 1, the C++
    /// producer's, was 128 bytes with three slots; older channels are refused by the version check).
    #[test]
    fn fence_header_layout() {
        assert_eq!(size_of::<FenceHeader>(), 616);
        assert_eq!(offset_of!(FenceHeader, adapter), 24);
        assert_eq!(offset_of!(FenceHeader, textures), 32);
        assert_eq!(offset_of!(FenceHeader, produced), 112);
        assert_eq!(offset_of!(FenceHeader, consumed), 120);
        assert_eq!(offset_of!(FenceHeader, sequence), 128);
        assert_eq!(offset_of!(FenceHeader, tick), 136);
        assert_eq!(offset_of!(FenceHeader, latest_slot), 144);
        assert_eq!(offset_of!(FenceHeader, reader_slot), 148);
        assert_eq!(offset_of!(FenceHeader, reader_claim), 152);
        assert_eq!(offset_of!(FenceHeader, slot_sequences), 160);
        assert_eq!(offset_of!(FenceHeader, slot_ticks), 200);
        assert_eq!(offset_of!(FenceHeader, depth), 240);
        assert_eq!(size_of::<FenceDepth>(), 376);
        assert_eq!(offset_of!(FenceDepth, textures), 16);
        assert_eq!(offset_of!(FenceDepth, slots), 96);
        assert_eq!(size_of::<DepthSlot>(), 56);
        assert_eq!(offset_of!(DepthSlot, params), 8);
        assert_eq!(offset_of!(DepthSlot, rects), 24);
        assert_eq!(size_of::<DepthParams>(), 16);
        assert_eq!(offset_of!(DepthParams, reversed), 8);
    }

    #[test]
    fn depth_params_give_the_distances_at_0_and_1() {
        let standard = DepthParams::new(0.1, 1000.0, false);
        assert!(standard.is_valid());
        assert_eq!(standard.distances_at_0_and_1(), (0.1, 1000.0));
        // Reversed depth is 1 at the near plane: the far plane is at 0, even when infinite.
        let reversed = DepthParams::new(0.05, f32::INFINITY, true);
        assert!(reversed.is_valid());
        assert_eq!(reversed.distances_at_0_and_1(), (f32::INFINITY, 0.05));
        assert!(!DepthParams::new(0.0, 100.0, false).is_valid(), "no near plane");
        assert!(!DepthParams::new(10.0, 1.0, false).is_valid(), "far before near");
        assert!(!DepthParams::new(f32::NAN, 100.0, false).is_valid());
        assert!(!DepthParams::default().is_valid(), "zeroed shared memory is no depth");
    }

    #[test]
    fn depth_slot_formats() {
        use dxgi_format::*;
        for format in [D32_FLOAT, R32_TYPELESS, R32_FLOAT, D32_FLOAT_S8X24_UINT, R32G8X24_TYPELESS, R32_FLOAT_X8X24_TYPELESS] {
            assert_eq!(depth_slot_format(format), Some(R32_TYPELESS), "{format}");
        }
        for format in [D16_UNORM, R16_TYPELESS, R16_UNORM] {
            assert_eq!(depth_slot_format(format), Some(R16_TYPELESS), "{format}");
        }
        assert_eq!(depth_slot_format(D24_UNORM_S8_UINT), None);
        assert_eq!(depth_slot_format(R8G8B8A8_UNORM), None);
    }

    #[test]
    fn channel_names() {
        let name = ChannelName::new(r"Local\monaka-1").unwrap();
        assert_eq!(name.header_lock(), r"Local\monaka-1-lock");
        assert_eq!(name.reader_lease(), r"Local\monaka-1-reader");
        assert_eq!(name.eye_texture(0), r"Local\monaka-1-left");
        assert_eq!(name.eye_texture(1), r"Local\monaka-1-right");
        assert_eq!(name.pose(), r"Local\monaka-1-pose");
        assert_eq!(name.pose_lock(), r"Local\monaka-1-pose-lock");
        assert_eq!(name.hands(), r"Local\monaka-1-hands");
        assert_eq!(name.hands_lock(), r"Local\monaka-1-hands-lock");
        assert_eq!(name.debug(), r"Local\monaka-1-debug");
        assert_eq!(name.debug_lock(), r"Local\monaka-1-debug-lock");
        assert!(ChannelName::new("").is_none());
        assert!(ChannelName::new("x".repeat(201)).is_none());
    }

    #[test]
    fn pose_ring_wraps_by_sequence() {
        assert_eq!(PoseBlock::pair_slot(17), 1);
        assert_eq!(PoseBlock::pair_slot(16), 0);
        assert!(PoseBlock::fresh().is_compatible());
    }

    #[test]
    fn fence_formats() {
        assert!(fence_format_ok(dxgi_format::R10G10B10A2_UNORM));
        assert!(!fence_format_ok(dxgi_format::R8G8B8A8_UNORM_SRGB));
    }
}
