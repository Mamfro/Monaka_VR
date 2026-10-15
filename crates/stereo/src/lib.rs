//! Producer-side stereo that does not depend on the game. A game crate supplies the part only it
//! can do (making the next frame render a given eye from a given head pose); this crate turns the
//! frames into published stereo pairs.
//!
//! - [`driver`]: the alternate-eye present loop around DXGI's Present (the game's swapchain,
//!   presents counted, the next eye and pose chosen, pairs published), for any game drawing one
//!   eye per frame.
//! - [`feature`]: the present loop as a ready-made component (`present()`).
//! - [`same_frame`]: the same-frame present loop (both eyes of a pair in one simulation frame) as a
//!   ready-made component (`same_frame()`), over D3D11, D3D12 or an OpenGL frame holding both eyes.
//! - [`providers`]: what a game provides once and every ready-made feature reads: its frame labels,
//!   its frame inputs (collected by the ready-made `frame_inputs()`), its HUD layer.
//! - [`depth_layer`]: each eye's depth to the headset with its pair, its component ready-made
//!   (`depth_layer()`).
//! - [`eye_site`]: the hook where the game's camera becomes an eye: its provider (`EyeSite`), the
//!   shared recipe at a call (`draw`), its component ready-made (`eyes()`).
//! - [`eye_size`]: rendering at the headset's eye size as a ready-made component (`eye_size()`).
//! - [`foveate12`]: eye-tracked foveated rendering through variable rate shading on D3D12, its
//!   component ready-made (`foveation()`).
//! - [`upscaler`]: a game's own temporal upscaler kept per eye, its component ready-made
//!   (`per_eye()`).
//! - [`alternate`]: alternate-eye rendering, each presented frame one eye: `d3d11` and `d3d12`
//!   swapchains.
//! - [`pairs`]: same-frame pairs: `d3d11` copies each eye at its present; `d3d12` publishes any
//!   two textures as one pair, each eye with its own pose.
//! - [`afw`]: AFW (alternate frame warping), one implementation: a pair of one moment at every
//!   present, the real eye warped into the other and its gaps filled from that eye's own last real
//!   frame. The core shared by its backends (which eye is real, the poses, the gaps' rules, each
//!   eye's history, what goes out, the counts, the component's shape). Its backends: `afw::d3d11`,
//!   its component ready-made (`frame_warping()`); `afw::d3d12`, both eyes' depth with each pair,
//!   its components ready-made (`frame_warping()`, `frame_generation()`, `frame_warping_pairs()`).
//! - [`framegen12`]: per-eye FSR frame generation for alternate eyes, its component ready-made
//!   (`per_eye()`), and for same-frame pairs (`pairs::per_eye_pairs()`).
//! - [`hud_layer`]: the HUD layer's base component (`hud_layer()`): the feature, the HUD's size and
//!   distance options, on while the HUD is shown; every method's HUD component builds on it. With
//!   the lay-over of a D3D12 game's HUD layer at its present request (`hud_layer::present12`), a
//!   D3D11 game's HUD placed in front of each eye (`hud_layer::d3d11`: viewport of the draws after
//!   the scene copy), and a D3D12 game's UI layer laid back over each eye frame at the HUD's place
//!   (`hud_layer::d3d12`, on [`framepass12`], a compute pass over a swapchain frame).
//! - [`hud_panels`]: the dynamic HUD, the HUD's pieces on the hands, draw-level (D3D11, OpenGL) or
//!   by crops of a D3D12 HUD layer; each piece's D3D11 panel is `hud_panels::panel`.
//! - [`lists12`]: one hook point for a D3D12 game's command lists, from its queue, shared by the
//!   features that watch or add to them (foveation, a HUD layer from the game's draws).
//! - [`present`]: DXGI's Present hooked for a game whose swapchain is out of reach.
//! - [`queue12`]: the game's D3D12 direct queue, caught at `ExecuteCommandLists`, for a game whose
//!   engine keeps it out of reach.
//! - [`swapchain_queue`]: the D3D12 queue a swapchain presents on, read from the swapchain itself.
//!
//! Depth-based stereo from a single centre render is `monaka_warp`, with its math in
//! `monaka_core::depth`.

pub mod afw;
pub mod alternate;
pub mod debug_depth;
pub mod debug_targets;
pub mod depth_layer;
pub mod driver;
pub mod eye_site;
pub mod eye_size;
pub mod feature;
pub mod foveate12;
pub mod framegen12;
pub mod framepass12;
pub mod hud_layer;
pub mod hud_panels;
pub mod lists12;
pub mod pairs;
pub mod present;
pub mod providers;
mod publish;
pub mod queue12;
pub mod same_frame;
pub mod swapchain_queue;
pub mod upscaler;
