//! Debug drawing: what Monaka computes, drawn by the viewer over the eye it belongs to. Producer
//! code emits [`Primitive`]s (segments and points in tracking space, each in a [`Category`]) for
//! the game frame it is working on, named by a frame key (a present number, a ticket: whatever the
//! game's loop counts frames by). The [`DebugBlock`] beside the run's channel carries them to the
//! viewer, with which frame each eye of a published pair was drawn in ([`EyeKeys`]), which
//! categories the viewer wants and which the producer can draw.

use crate::protocol::{Stamp, Versioned};

#[cfg(test)]
mod tests;

pub const DEBUG_MAGIC: u32 = 0x4742_4544;
pub const DEBUG_MAJOR: u16 = 9;
/// Depth buffers listed for the viewer's pick ([`DepthCandidate`]).
pub const DEPTH_CANDIDATES: usize = 8;
/// Frames kept, newest over oldest (slot = key % frames): enough for the pair being shown and the
/// frames drawn since.
pub const DEBUG_FRAMES: usize = 8;
/// Primitives one frame holds; more are dropped (counted in [`DebugFrame::dropped`]).
pub const DEBUG_PRIMITIVES: usize = 4096;
/// Published pairs whose eye keys are kept (slot = sequence % slots).
pub const DEBUG_PAIRS: usize = 16;
/// Bytes of the game's settings line in [`GameReport`].
pub const GAME_TEXT: usize = 192;
/// The game's latest frame times kept in [`GameReport`] (a ring), for the Stats panel's graph.
pub const GAME_FRAMES: usize = 256;

/// What can be drawn or done, one bit each in a [`Categories`] set. The first ones are drawings;
/// the actions (at [`Category::FIRST_ACTION`] and up) change what the producer does instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum Category {
    /// The controllers' aim and grip axes, drawn by the viewer from its own poses.
    Controllers = 0,
    /// The skeleton's bones: the game's animated pose and the rig's placed pose.
    Skeleton = 1,
    /// Each bone's axes (x red, y green, z blue).
    BoneAxes = 2,
    /// Finger chains and their curl axes.
    Fingers = 3,
    /// Where the hand rig aimed the hands, against the controllers' palms.
    HandTargets = 4,
    /// The hand's aim ray, the converged weapon point, the head-aim target.
    Aim = 5,
    /// Swing tips: their trail, coloured by speed and swing state.
    Swings = 6,
    /// Melee reach, blade lengths, hit points.
    Melee = 7,
    /// The room-scale anchor and walk offset.
    RoomScale = 8,
    /// The over-the-shoulder menu zones, drawn by the viewer.
    MenuReach = 9,
    /// Anything a probe draws while a game is being worked out.
    Probe = 10,
    /// Action: the producer leaves its HUD lay-over out.
    HideHud = 48,
    /// Action: the hand rig leaves the arms where the game put them.
    SkipHandRig = 49,
    /// Action: the producer shares the game's depth ([`DebugDepth`]), for a depth view of a game
    /// whose pairs carry none. The viewer asks for it while such a view is shown; it is no item of
    /// the menu.
    Depth = 50,
    /// Action: the producer shares one of the game's render targets (its normals, its albedo...),
    /// for the viewer's "Game buffer" view; no item of the menu either.
    Targets = 51,
    /// Action: the viewer shows the Stats panel: the producer measures what Monaka costs each
    /// frame ([`GameReport::costs`]) and names each pair's eye frames even with nothing drawn.
    /// No item of the menu.
    Stats = 52,
    /// Action: the game's world stands still (its characters, animations and scripted scenes)
    /// while the player's view goes on, to look around a moment in the headset.
    FreezeWorld = 53,
}

impl Category {
    pub const FIRST_ACTION: u16 = 48;
    pub const ALL: [Category; 17] = [
        Category::Controllers,
        Category::Skeleton,
        Category::BoneAxes,
        Category::Fingers,
        Category::HandTargets,
        Category::Aim,
        Category::Swings,
        Category::Melee,
        Category::RoomScale,
        Category::MenuReach,
        Category::Probe,
        Category::HideHud,
        Category::SkipHandRig,
        Category::Depth,
        Category::Targets,
        Category::Stats,
        Category::FreezeWorld,
    ];

    pub const fn bit(self) -> u64 {
        1 << self as u16
    }

    pub const fn is_action(self) -> bool {
        self as u16 >= Self::FIRST_ACTION
    }

    /// Drawn by the viewer itself, whatever the producer offers.
    pub const fn viewer_drawn(self) -> bool {
        matches!(self, Category::Controllers | Category::MenuReach)
    }

    pub fn from_index(index: u16) -> Option<Category> {
        Self::ALL.into_iter().find(|c| *c as u16 == index)
    }

    /// A name that stays the same, for files that remember choices.
    pub const fn id(self) -> &'static str {
        match self {
            Category::Controllers => "controllers",
            Category::Skeleton => "skeleton",
            Category::BoneAxes => "bone_axes",
            Category::Fingers => "fingers",
            Category::HandTargets => "hand_targets",
            Category::Aim => "aim",
            Category::Swings => "swings",
            Category::Melee => "melee",
            Category::RoomScale => "room_scale",
            Category::MenuReach => "menu_reach",
            Category::Probe => "probes",
            Category::HideHud => "hide_hud",
            Category::SkipHandRig => "skip_hand_rig",
            Category::Depth => "depth",
            Category::Targets => "targets",
            Category::Stats => "stats",
            Category::FreezeWorld => "freeze_world",
        }
    }

    pub fn from_id(id: &str) -> Option<Category> {
        Self::ALL.into_iter().find(|c| c.id() == id)
    }

    pub const fn label(self) -> &'static str {
        match self {
            Category::Controllers => "Controllers",
            Category::Skeleton => "Skeleton",
            Category::BoneAxes => "Bone axes",
            Category::Fingers => "Fingers",
            Category::HandTargets => "Hand targets",
            Category::Aim => "Aim",
            Category::Swings => "Swings",
            Category::Melee => "Melee",
            Category::RoomScale => "Room-scale",
            Category::MenuReach => "Menu reach",
            Category::Probe => "Probes",
            Category::HideHud => "Hide the HUD",
            Category::SkipHandRig => "Game's own arms",
            Category::Depth => "Depth",
            Category::Targets => "Game buffers",
            Category::Stats => "Stats",
            Category::FreezeWorld => "Freeze the world",
        }
    }
}

/// A set of [`Category`]s.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Categories(pub u64);

impl Categories {
    pub const NONE: Self = Self(0);

    pub const fn has(self, category: Category) -> bool {
        self.0 & category.bit() != 0
    }

    pub const fn with(self, category: Category, on: bool) -> Self {
        if on {
            Self(self.0 | category.bit())
        } else {
            Self(self.0 & !category.bit())
        }
    }

    /// Whether any drawing (not an action) is in the set.
    pub const fn draws(self) -> bool {
        self.0 & ((1 << Category::FIRST_ACTION) - 1) != 0
    }
}

/// What a primitive is.
pub const KIND_SEGMENT: u16 = 0;
/// A point at `a`, drawn as a small square facing the eye (`b` unused).
pub const KIND_POINT: u16 = 1;

/// One segment or point in tracking space (metres, the headset's local space).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Primitive {
    pub a: [f32; 3],
    pub b: [f32; 3],
    /// Straight (not premultiplied) RGBA, red in the low byte ([`rgba`]).
    pub colour: u32,
    pub category: u16,
    pub kind: u16,
}

impl Primitive {
    pub const fn segment(category: Category, a: [f32; 3], b: [f32; 3], colour: u32) -> Self {
        Self {
            a,
            b,
            colour,
            category: category as u16,
            kind: KIND_SEGMENT,
        }
    }

    pub const fn point(category: Category, at: [f32; 3], colour: u32) -> Self {
        Self {
            a: at,
            b: at,
            colour,
            category: category as u16,
            kind: KIND_POINT,
        }
    }
}

/// A colour for [`Primitive::colour`] from straight RGBA in 0..=1.
pub fn rgba(r: f32, g: f32, b: f32, a: f32) -> u32 {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
    byte(r) | byte(g) << 8 | byte(b) << 16 | byte(a) << 24
}

pub const RED: u32 = 0xff30_30ff;
pub const GREEN: u32 = 0xff30_ff30;
pub const BLUE: u32 = 0xffff_6030;
pub const WHITE: u32 = 0xffff_ffff;
pub const YELLOW: u32 = 0xff30_e0ff;
pub const ORANGE: u32 = 0xff20_80ff;
pub const CYAN: u32 = 0xffff_e030;
pub const MAGENTA: u32 = 0xffff_40ff;

/// The three axes of a matrix (row-major 3x4, columns x, y, z, position, as `monaka_core::camera::Mat34`) as segments
/// `size` long from its position: x red, y green, z blue.
pub fn axes(category: Category, matrix: &[f32; 12], size: f32) -> [Primitive; 3] {
    let at = [matrix[3], matrix[7], matrix[11]];
    let tip = |column: usize| [0, 1, 2].map(|row| at[row] + matrix[row * 4 + column] * size);
    [
        Primitive::segment(category, at, tip(0), RED),
        Primitive::segment(category, at, tip(1), GREEN),
        Primitive::segment(category, at, tip(2), BLUE),
    ]
}

/// Which game frame each eye of published pair `sequence` was drawn in.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EyeKeys {
    pub sequence: u64,
    pub keys: [u64; 2],
}

/// The game's depth as the producer shares it ([`Category::Depth`]): a 32-bit float texture by
/// name (`ChannelName::debug_depth`) behind a keyed mutex, made again when `generation` changes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DebugDepth {
    /// 0 until the texture is made; one more each time it is made again (a new size).
    pub generation: u32,
    /// 1: depth 1 is near (reversed depth), 0: depth 0 is near, 2: not known (the game never
    /// clears it, which would say).
    pub reversed: u32,
    pub width: u32,
    pub height: u32,
    /// The frame whose depth the texture holds.
    pub key: u64,
    /// `GetTickCount64` when it was written.
    pub tick: u64,
    /// How many times the game cleared this depth in the frame, and what the texture holds: the
    /// depth just before clear `pass` (1 the first: what the frame drew before reusing it), or at
    /// the present (0).
    pub passes: u32,
    pub pass: u32,
    /// The eye whose frame it is (0 left, 1 right; 2 not known: a game drawing both eyes at once,
    /// or a loop that does not say).
    pub eye: u32,
    pub _pad: u32,
}

/// A depth buffer the game drew with, listed for the viewer's pick of which to share.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DepthCandidate {
    pub width: u32,
    pub height: u32,
    /// Its `DXGI_FORMAT`.
    pub format: u32,
    /// Draws a frame with it, on average lately.
    pub draws: u32,
    pub samples: u16,
    pub layers: u16,
    /// 1: it can be shared (single-sample, one layer, a depth format).
    pub usable: u32,
}

impl DepthCandidate {
    /// A render target's format's short name: RGBA8, RGBA16F, R10G10B10A2...
    pub fn colour_format_name(&self) -> &'static str {
        match self.format {
            1..=4 => "RGBA32F",
            10 => "RGBA16F",
            9 | 11..=14 => "RGBA16",
            15..=18 => "RG32F",
            23..=25 => "R10G10B10A2",
            26 => "R11G11B10F",
            27..=32 => "RGBA8",
            34 => "RG16F",
            33 | 35..=38 => "RG16",
            39 | 41 => "R32F",
            48..=52 => "RG8",
            54 => "R16F",
            53 | 56..=58 => "R16",
            60..=65 => "R8",
            87..=93 => "BGRA8",
            _ => "?",
        }
    }

    /// A depth format's short name: D24, D32, D32S8, D16.
    pub fn format_name(&self) -> &'static str {
        match self.format {
            44..=47 => "D24",
            39..=41 => "D32",
            19..=22 => "D32S8",
            53..=56 => "D16",
            _ => "?",
        }
    }
}

/// One game frame's primitives.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DebugFrame {
    /// The frame's key; 0 for a slot never written.
    pub key: u64,
    /// `GetTickCount64` when the frame was written.
    pub written_tick: u64,
    pub count: u32,
    /// Primitives emitted past [`DEBUG_PRIMITIVES`].
    pub dropped: u32,
    pub primitives: [Primitive; DEBUG_PRIMITIVES],
}

/// The game side's timing and settings, for the viewer's Stats panel ([`DebugBlock::game`]).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GameReport {
    /// `GetTickCount64` when the timing was last written (0: never).
    pub tick: u64,
    /// The game's presents a second, over the last measured stretch.
    pub fps: f32,
    /// Its frame time over that stretch: the mean and the longest (ms).
    pub frame_ms: f32,
    pub worst_frame_ms: f32,
    /// Its swapchain's size (0: unknown).
    pub width: u32,
    pub height: u32,
    pub text_len: u32,
    /// The game's frames measured since the run's start: frame `n`'s time (ms) is in
    /// `frame_times[n % GAME_FRAMES]`, the newest `frames - 1`.
    pub frames: u64,
    pub frame_times: [f32; GAME_FRAMES],
    /// What Monaka's own work cost the game's threads (CPU) each frame over the stretch, by
    /// [`Cost`], while the viewer shows the Stats panel ([`Category::Stats`]).
    pub costs: [CostTime; COSTS],
    /// The run's rendering settings, one `Label: value` per line (UTF-8).
    pub text: [u8; GAME_TEXT],
}

/// Where Monaka's own work in the game is timed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Cost {
    /// All of it around the game's present, the present itself left out.
    AtPresent = 0,
    /// Of that, before the present: the frame's eye kept, the pair made and published (a warp
    /// included).
    BeforePresent = 1,
    /// Of that, after the present: the next eye chosen.
    AfterPresent = 2,
    /// The hand rig's posing of the arms (wherever the game runs it).
    HandRig = 3,
}

pub const COSTS: usize = 4;

impl Cost {
    pub const ALL: [Cost; COSTS] = [Cost::AtPresent, Cost::BeforePresent, Cost::AfterPresent, Cost::HandRig];

    pub const fn label(self) -> &'static str {
        match self {
            Cost::AtPresent => "At the present",
            Cost::BeforePresent => "Before it",
            Cost::AfterPresent => "After it",
            Cost::HandRig => "Hand rig",
        }
    }
}

/// One [`Cost`]'s time a frame (ms): the mean over the frames measured, and the longest single
/// one.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CostTime {
    pub mean_ms: f32,
    pub worst_ms: f32,
}

impl GameReport {
    pub const EMPTY: Self = Self {
        tick: 0,
        fps: 0.0,
        frame_ms: 0.0,
        worst_frame_ms: 0.0,
        width: 0,
        height: 0,
        text_len: 0,
        frames: 0,
        frame_times: [0.0; GAME_FRAMES],
        costs: [CostTime { mean_ms: 0.0, worst_ms: 0.0 }; COSTS],
        text: [0; GAME_TEXT],
    };

    /// Adds frame times measured after the last ones written, oldest first.
    pub fn push_frames(&mut self, times: &[f32]) {
        for time in times {
            self.frame_times[(self.frames % GAME_FRAMES as u64) as usize] = *time;
            self.frames += 1;
        }
    }

    /// The frame times after frame `since` (at most the ring's), oldest first, and the count now.
    pub fn frames_since(&self, since: u64) -> (Vec<f32>, u64) {
        let first = since.max(self.frames.saturating_sub(GAME_FRAMES as u64));
        let times = (first..self.frames).map(|n| self.frame_times[(n % GAME_FRAMES as u64) as usize]).collect();
        (times, self.frames)
    }

    /// The settings lines, cut at a character boundary where `text` was too long.
    pub fn text(&self) -> &str {
        let bytes = &self.text[..(self.text_len as usize).min(GAME_TEXT)];
        match std::str::from_utf8(bytes) {
            Ok(text) => text,
            Err(e) => std::str::from_utf8(&bytes[..e.valid_up_to()]).unwrap_or_default(),
        }
    }

    pub fn set_text(&mut self, text: &str) {
        let mut len = text.len().min(GAME_TEXT);
        while !text.is_char_boundary(len) {
            len -= 1;
        }
        self.text[..len].copy_from_slice(&text.as_bytes()[..len]);
        self.text_len = len as u32;
    }
}

/// The debug block beside a stereo channel. Either side may create it; it is written in place
/// (it is about a megabyte: never copied whole).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DebugBlock {
    pub stamp: Stamp,
    /// The categories the viewer wants (its menu), and when it last said so.
    pub wanted: u64,
    pub wanted_tick: u64,
    /// The categories the producer can draw or do, and when it last said so.
    pub offered: u64,
    pub offered_tick: u64,
    /// The depth pass the viewer wants shared ([`DebugDepth::pass`]).
    pub wanted_pass: u32,
    /// The depth buffer the viewer wants shared: 0 the producer's own pick, else that candidate
    /// (1 the first).
    pub wanted_buffer: u32,
    pub depth: DebugDepth,
    /// The depth buffers the producer saw drawn with, and which it shares (1 the first, 0 none).
    pub candidates: [DepthCandidate; DEPTH_CANDIDATES],
    pub candidate_count: u32,
    pub chosen: u32,
    /// The same for the game's render targets ([`Category::Targets`]): which one the viewer wants
    /// shared (0 the producer's own pick), the one shared (its record: `reversed` and the passes
    /// unused), those listed, and which is shared.
    pub wanted_target: u32,
    pub _target_pad: u32,
    pub target: DebugDepth,
    pub targets: [DepthCandidate; DEPTH_CANDIDATES],
    pub target_count: u32,
    pub target_chosen: u32,
    /// The game's timing and settings, for the Stats panel.
    pub game: GameReport,
    pub pairs: [EyeKeys; DEBUG_PAIRS],
    pub frames: [DebugFrame; DEBUG_FRAMES],
}

impl DebugBlock {
    /// All zeros, as a new mapping is.
    pub const EMPTY: Self = Self {
        stamp: Stamp { magic: 0, major: 0, minor: 0, size: 0, _reserved: 0 },
        wanted: 0,
        wanted_tick: 0,
        offered: 0,
        offered_tick: 0,
        wanted_pass: 0,
        wanted_buffer: 0,
        depth: DebugDepth { generation: 0, reversed: 0, width: 0, height: 0, key: 0, tick: 0, passes: 0, pass: 0, eye: 2, _pad: 0 },
        candidates: [DepthCandidate { width: 0, height: 0, format: 0, draws: 0, samples: 0, layers: 0, usable: 0 }; DEPTH_CANDIDATES],
        candidate_count: 0,
        chosen: 0,
        wanted_target: 0,
        _target_pad: 0,
        target: DebugDepth { generation: 0, reversed: 0, width: 0, height: 0, key: 0, tick: 0, passes: 0, pass: 0, eye: 2, _pad: 0 },
        targets: [DepthCandidate { width: 0, height: 0, format: 0, draws: 0, samples: 0, layers: 0, usable: 0 }; DEPTH_CANDIDATES],
        target_count: 0,
        target_chosen: 0,
        game: GameReport::EMPTY,
        pairs: [EyeKeys {
            sequence: 0,
            keys: [0; 2],
        }; DEBUG_PAIRS],
        frames: [DebugFrame::EMPTY; DEBUG_FRAMES],
    };

    pub const fn frame_slot(key: u64) -> usize {
        (key % DEBUG_FRAMES as u64) as usize
    }

    pub const fn pair_slot(sequence: u64) -> usize {
        (sequence % DEBUG_PAIRS as u64) as usize
    }

    /// The eye keys of pair `sequence`, if its publisher wrote them.
    pub fn eye_keys(&self, sequence: u64) -> Option<[u64; 2]> {
        let pair = &self.pairs[Self::pair_slot(sequence)];
        (pair.sequence == sequence && sequence != 0).then_some(pair.keys)
    }

    /// The frame with key `key`, if it is still kept.
    pub fn frame(&self, key: u64) -> Option<&DebugFrame> {
        let frame = &self.frames[Self::frame_slot(key)];
        (frame.key == key && key != 0).then_some(frame)
    }

    /// The newest frame written.
    pub fn newest(&self) -> Option<&DebugFrame> {
        self.frames
            .iter()
            .filter(|f| f.key != 0)
            .max_by_key(|f| (f.written_tick, f.key))
    }

    /// Writes frame `key`'s primitives over the slot it falls in.
    pub fn write_frame(&mut self, key: u64, primitives: &[Primitive], tick: u64) {
        let frame = &mut self.frames[Self::frame_slot(key)];
        let count = primitives.len().min(DEBUG_PRIMITIVES);
        frame.primitives[..count].copy_from_slice(&primitives[..count]);
        frame.count = count as u32;
        frame.dropped = (primitives.len() - count) as u32;
        frame.written_tick = tick;
        frame.key = key;
    }

    pub fn write_eye_keys(&mut self, sequence: u64, keys: [u64; 2]) {
        self.pairs[Self::pair_slot(sequence)] = EyeKeys { sequence, keys };
    }
}

impl Versioned for DebugBlock {
    const MAGIC: u32 = DEBUG_MAGIC;
    const MAJOR: u16 = DEBUG_MAJOR;
    const MINOR: u16 = 0;
    const WHAT: &'static str = "debug block";
    const CAPACITY: usize = 2 * 1024 * 1024;

    fn stamp(&self) -> Stamp {
        self.stamp
    }

    fn start(&mut self) {
        self.stamp = Stamp::of::<Self>();
    }
}

impl DebugFrame {
    pub const EMPTY: Self = Self {
        key: 0,
        written_tick: 0,
        count: 0,
        dropped: 0,
        primitives: [Primitive {
            a: [0.0; 3],
            b: [0.0; 3],
            colour: 0,
            category: 0,
            kind: 0,
        }; DEBUG_PRIMITIVES],
    };

    pub fn primitives(&self) -> &[Primitive] {
        &self.primitives[..(self.count as usize).min(DEBUG_PRIMITIVES)]
    }
}
