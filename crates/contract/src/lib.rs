//! The contract between Monaka (the launcher, the loader and the viewer) and a game pack (a
//! producer DLL that links Monaka's crates into itself): everything that crosses the process
//! boundary, and nothing else. What a pack links in and runs inside the game is free to change;
//! what is here is read by a host built apart from the pack.
//!
//! - [`pack`]: the pack's exports, its manifest text and the contract version.
//! - [`host`]: the features a host has, and those a pack requires or uses.
//! - [`manifest`]: the manifest's data and the shared option vocabulary.
//! - [`status`]: what the producer's entry points answer the loader.
//! - [`protocol`]: the stereo channel's header, the blocks beside it and their names; with
//!   [`gamepad`], [`panels`], [`page`], [`console`] and [`debug_draw`] for the blocks' records.
//!
//! The rules for changing any of it are in [`pack`] (the text) and [`protocol`] (the records).
#![forbid(unsafe_code)]

pub mod console;
pub mod debug_draw;
pub mod gamepad;
pub mod host;
pub mod manifest;
pub mod pack;
pub mod page;
pub mod panels;
pub mod protocol;
pub mod status;
