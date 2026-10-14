//! The 2D window's own command line (`monaka_play view "<command>"`), answered on its own console
//! channel ([`monaka_channel::console::ConsoleChannel::open_view`]): what the window shows, set
//! and seen without the mouse. The fake head and hands, the overlays and the panels, as the menu
//! and the Fake VR panel set them (remembered the same way); and a capture of the window as shown
//! (or of the game's picture alone, overlays on it) saved as PNG, its path the answer.

use crate::debug_views::DebugViews;
use monaka_core::debug_draw::Category;
use monaka_overlay::fake_vr::{FakeVr, Hand, Preset};
use monaka_overlay::panels::PanelId;
use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_B8G8R8A8_UNORM_SRGB};

const HELP: &str = "\
help                                  this list
status                                the fake head and hands, the overlays and panels on
capture                               the window as shown (game, overlays, menu, panels) as PNG
capture eye                           the game's picture alone with its overlays, full size, as PNG
head <turn> <tilt>                    the fake head (degrees)
hands <off|forward|aiming|sides>      the fake hands' pose
hand <left|right> <x> <y> <z> [yaw] [pitch]
                                      one fake hand: metres from the head (x right, y up, z back), degrees
overlay <name|list> [on|off]          an overlay (skeleton, bone_axes, fingers, hand_targets, ...)
panel <stats|console|fake_vr> [on|off]";

/// What a command asks of the window.
pub enum Asked {
    /// Done; the answer.
    Answer(String),
    /// A capture of the window (else of the game's picture), taken once this frame is drawn.
    Capture { window: bool },
}

/// Runs `command` on the window's state.
pub fn run(command: &str, fake: &mut FakeVr, views: &mut DebugViews, now: Instant) -> Asked {
    let words: Vec<&str> = command.split_whitespace().collect();
    let answer = match words.as_slice() {
        [] | ["help"] => Ok(HELP.to_owned()),
        ["status"] => Ok(status(fake, views)),
        ["capture"] => return Asked::Capture { window: true },
        ["capture", "eye"] => return Asked::Capture { window: false },
        ["head", turn, tilt] => number(turn).and_then(|turn| Ok([turn, number(tilt)?])).map(|head| {
            fake.head = head;
            format!("head turned {:.0}, tilted {:.0}", head[0], head[1])
        }),
        ["hands", preset] => preset_named(preset).map(|preset| {
            fake.preset = preset;
            fake.hands = preset.hands();
            format!("hands {}", preset.label())
        }),
        ["hand", side, x, y, z, rest @ ..] if rest.len() <= 2 => hand(fake, side, [x, y, z], rest),
        ["overlay", "list"] => Ok(Category::ALL.iter().map(|c| format!("{} ({})", c.id(), c.label())).collect::<Vec<_>>().join("\n")),
        ["overlay", name] => category(name).map(|c| format!("{}: {}", c.label(), on_off(views.settings().wanted.has(c)))),
        ["overlay", name, state] => category(name).and_then(|c| Ok((c, switch(state)?))).map(|(c, on)| {
            views.change_settings(|s| s.wanted = s.wanted.with(c, on), now);
            format!("{}: {}", c.label(), on_off(on))
        }),
        ["panel", name] => panel(name).map(|p| format!("{}: {}", p.label(), on_off(views.settings().panels.has(p)))),
        ["panel", name, state] => panel(name).and_then(|p| Ok((p, switch(state)?))).map(|(p, on)| {
            views.change_settings(|s| s.panels = s.panels.with(p, on), now);
            format!("{}: {}", p.label(), on_off(on))
        }),
        _ => Err(format!("unknown command {command:?}; help lists them")),
    };
    Asked::Answer(answer.unwrap_or_else(|e| format!("error: {e}")))
}

fn number(text: &str) -> Result<f32, String> {
    text.parse().map_err(|_| format!("{text}: not a number"))
}

fn switch(text: &str) -> Result<bool, String> {
    match text {
        "on" => Ok(true),
        "off" => Ok(false),
        _ => Err(format!("{text}: on or off")),
    }
}

fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

fn preset_named(name: &str) -> Result<Preset, String> {
    match name {
        "off" => Ok(Preset::Off),
        "forward" => Ok(Preset::Forward),
        "aiming" => Ok(Preset::Aiming),
        "sides" | "at_sides" => Ok(Preset::AtSides),
        _ => Err(format!("{name}: off, forward, aiming or sides")),
    }
}

fn category(name: &str) -> Result<Category, String> {
    Category::from_id(name).ok_or_else(|| format!("{name}: no such overlay (overlay list)"))
}

fn panel(name: &str) -> Result<PanelId, String> {
    PanelId::ALL.into_iter().find(|p| p.label().to_ascii_lowercase().replace(' ', "_") == name.to_ascii_lowercase()).ok_or_else(|| format!("{name}: stats, console or fake_vr"))
}

/// One fake hand placed (the others kept; hands that were off start from the forward pose).
fn hand(fake: &mut FakeVr, side: &str, at: [&&str; 3], turn: &[&str]) -> Result<String, String> {
    let index = match side {
        "left" => 0,
        "right" => 1,
        _ => return Err(format!("{side}: left or right")),
    };
    let at = [number(at[0])?, number(at[1])?, number(at[2])?];
    let yaw = turn.first().map_or(Ok(0.0), |t| number(t))?;
    let pitch = turn.get(1).map_or(Ok(0.0), |t| number(t))?;
    let mut hands = fake.hands.or(Preset::Forward.hands()).ok_or("no hands")?;
    hands[index] = Hand { at, yaw, pitch };
    fake.hands = Some(hands);
    Ok(format!("{side} hand at {at:?}, yaw {yaw:.0}, pitch {pitch:.0}"))
}

fn status(fake: &FakeVr, views: &DebugViews) -> String {
    let settings = views.settings();
    let mut out = format!("head turned {:.0}, tilted {:.0}; hands {}\n", fake.head[0], fake.head[1], fake.preset.label());
    for (hand, name) in fake.hands.iter().flatten().zip(["left", "right"]) {
        let _ = writeln!(out, "  {name}: at {:?}, yaw {:.0}, pitch {:.0}", hand.at, hand.yaw, hand.pitch);
    }
    let overlays: Vec<&str> = Category::ALL.iter().filter(|c| settings.wanted.has(**c)).map(|c| c.id()).collect();
    let panels: Vec<&str> = settings.panels.iter().map(PanelId::label).collect();
    let _ = writeln!(out, "overlays on: {}", if overlays.is_empty() { "none".to_owned() } else { overlays.join(", ") });
    let _ = write!(out, "panels on: {}", if panels.is_empty() { "none".to_owned() } else { panels.join(", ") });
    out
}

/// `texture` saved as a PNG in `folder` (BGRA and 10-bit colour turned to RGBA8, the alpha made
/// opaque, as shown); its path.
pub fn save_png(device: &ID3D11Device, context: &ID3D11DeviceContext, texture: &ID3D11Texture2D, folder: &Path, name: &str) -> Result<PathBuf, String> {
    let (width, height, mut pixels) = monaka_channel::d3d::read_image(device, context, texture).map_err(|e| format!("the picture could not be read: {e}"))?;
    let desc = monaka_channel::d3d::texture_desc(texture);
    let bgra = desc.Format == DXGI_FORMAT_B8G8R8A8_UNORM || desc.Format == DXGI_FORMAT_B8G8R8A8_UNORM_SRGB;
    for pixel in pixels.as_chunks_mut::<4>().0 {
        match monaka_core::bmp::dxgi_rgba8(desc.Format.0 as u32, *pixel) {
            Some(rgba) => *pixel = rgba,
            None if bgra => pixel.swap(0, 2),
            None => {}
        }
        pixel[3] = 255;
    }
    std::fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    let path = folder.join(name);
    std::fs::write(&path, monaka_core::png::rgba(width, height, &pixels)).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}
