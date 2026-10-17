//! Game-independent logic shared by the viewer and the in-game producers.
//!
//! Everything here is plain data and arithmetic, so it is testable without a GPU, a headset or a
//! running game. Code that touches Windows, Direct3D or OpenXR lives in the edge crates.
#![forbid(unsafe_code)]

pub mod aim;
pub mod alternate;
pub mod bmp;
pub mod camera;
pub mod chord;
pub mod convention;
pub mod depth;
pub mod eye_history;
pub mod feature;
pub mod fence_rules;
pub mod fingers;
pub mod gesture;
pub mod half;
pub mod hold;
pub mod home;
pub mod hud;
pub mod mat4d;
pub mod math;
pub mod menu_reach;
pub mod options;
pub mod pad;
pub mod pad_layer;
pub mod pe;
pub mod png;
pub mod reach;
pub mod rendering;
pub mod run;
pub mod scan;
pub mod scrub;
pub mod sha256;
pub mod situation;
pub mod stream;
pub mod swing;
pub mod sync;
pub mod ticket;
pub mod trail;
pub mod x64;

/// A value on a 16-byte boundary: engine matrix code reads with aligned SSE loads, and an unaligned
/// matrix faults (in Dying Light 2, an access violation reading 0xffffffffffffffff).
#[repr(C, align(16))]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Aligned16<T>(pub T);
