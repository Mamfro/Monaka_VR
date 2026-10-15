//! A scripted stand-in for the viewer: drives a producer through its channels with a timed script
//! (head pose, the controllers' poses, the controllers' gamepad) and saves the pairs it is asked
//! for, so a producer's VR behaviour can be checked on the PC without a headset or a player.
//!
//! `cargo run -p monaka_channel --example harness -- <channel name> <script file> <output folder>`
//!
//! The script is one step per line (`#` starts a comment): the time in milliseconds from the
//! start, a command and its arguments. Everything holds until changed.
//!
//! - `head <yaw> <pitch>`: the head turned (degrees; yaw left, pitch up), at the tracking origin.
//! - `hand <left|right> <x> <y> <z> <yaw> <pitch>`: that controller's aim pose (metres from the
//!   head, x right, y up, z back; degrees as for the head); its grip sits 8 cm behind and 3 cm
//!   below, as on a real controller. `hand <left|right> off` stops locating it.
//! - `pad <buttons> <left trigger> <right trigger>`: the controllers' gamepad (buttons joined with
//!   `+`: a, b, x, y, lb, rb, start, back, ls, rs, up, down, left, right, or `none`; triggers
//!   0..=255).
//! - `stick <left x> <left y> <right x> <right y>`: the gamepad's sticks (-1..=1).
//! - `fingers <left|right> <bend>`: that hand's fingers tracked, every knuckle bent this far
//!   toward the palm (degrees; 0 an open hand, 90 a fist); `fingers <left|right> off` stops.
//! - `save <name> [full]`: the next pair goes to `<name>-left.png` and `<name>-right.png` (half
//!   size by point sampling, or `full` at the pair's own size, for sub-pixel questions),
//!   and each HUD panel the producer publishes (`<channel>-panel-<piece>`) to
//!   `<name>-panel-<piece>.png` (over grey: the panels are premultiplied RGBA).
//! - `reticle <left|right> [palm|head]` (or `reticle off`): the saved frames show the viewer's
//!   hand-aim ring for that controller, as `--reticle` and `--reticle-from` place it (10 m along
//!   the controller's aim, from its palm or from the head), drawn where it lands in each eye.
//! - `focus <exe>`: brings that process's window to the front (a game reads its pad, mouse and
//!   keys only with focus).
//! - `key <name>`: a key pressed and let go on the keyboard (esc, enter, space, tab, f1..f12,
//!   a..z, 0..9), for the window in front.
//! - `depth on` (or `off`): saved pairs also write each eye's depth the producer sent with them
//!   (`<name>-depth-left.png`, `-right`; nearer is brighter, each image spread over its own range).
//! - `unpause` / `pause`: the gamepad's Start pressed (a tenth of a second) only when it changes
//!   the game's state: the next pair shows whether a menu is up (a producer sends a menu flat, the
//!   same image to both eyes). The pad, not Esc: a key would put the game in keyboard mode.
//! - `end`: stop.
//!
//! The gamepad goes to the producer as the viewer sends it: through the controller proxy, the d-pad
//! layer and the held d-pad choices ([`monaka_core::pad_layer`]), the menus by reach
//! ([`monaka_core::menu_reach`]), the Monaka menu's Start hold ([`monaka_core::hold`]: Start held
//! half a second opens or closes it, and the game then gets an idle pad; a shorter press reaches the
//! game as a tap when let go) and the capture chord (Start and Back together, [`monaka_core::chord`]).
//! Each is set as the viewer's options set it:
//!
//! - `layer <layer|shift|rest|none>`: that layer control held (`layer`: the d-pad and Back, as the
//!   thumbrest; `shift`: the d-pad only; `rest`: the proxy's stand-in, the left grip).
//! - `act_like <touch|index|off>`: the controller proxy.
//! - `dpad_hold <directions>`: the d-pad directions a pick keeps held (`left,right`; `none`).
//! - `reach <same|other|off> [depth cm] [buttons|stick]`: the menus by reach (off until set), which
//!   pick with the same or the other hand's stick, the zone's depth behind the head (15 by default),
//!   and whether a pick presses a face button or is handed to the game as its right stick.
//! - `monaka_menu <on|off>`: the Monaka menu's Start hold (on, as the launcher runs the viewer).
//!
//! Only one reader may hold a channel: run it instead of the viewer.

use monaka_channel::d3d::{copy_texture, create_device, create_texture, find_adapter, read_image, simple_texture_desc};
use monaka_channel::fingers::FingerChannel;
use monaka_channel::hands::HandChannel;
use monaka_channel::pose::PoseChannel;
use monaka_channel::reader::{self, FenceReader};
use monaka_channel::{ChannelName, tick};
use monaka_channel::reticles::ReticleChannel;
use monaka_core::chord::Chord;
use monaka_core::hold::HoldToggle;
use monaka_core::menu_reach::{self, Pick, PickWith, Reach};
use monaka_core::pad::{Gamepad, button};
use monaka_core::pad_layer::{Held, Layers, Proxy, dpad_directions};
use monaka_core::png;
use monaka_core::protocol::{FingerPose, HandPose, HeadPose, PadState, RenderedPair};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;

/// Left, right, up, down half-angles in radians (a headset's lopsided eyes).
const LEFT_FOV: [f32; 4] = [-0.90, 0.75, 0.85, -0.90];
const RIGHT_FOV: [f32; 4] = [-0.75, 0.90, 0.85, -0.90];

#[derive(Clone, Debug)]
enum Step {
    Head { yaw: f32, pitch: f32 },
    Hand { side: usize, pose: Option<([f32; 3], f32, f32)> },
    Pad { buttons: u16, triggers: [u8; 2] },
    Stick([f32; 4]),
    Fingers { side: usize, bend: Option<f32> },
    Save(String, bool),
    Reticle(Option<(usize, bool)>),
    Focus(String),
    Key(u16),
    /// Esc pressed only if the next pair is not what this asks: a menu up (true) or not (false).
    Menu(bool),
    Depth(bool),
    Layer(Held),
    ActLike(Proxy),
    DpadHold(u16),
    Reach(Option<(Pick, f32, PickWith)>),
    MonakaMenu(bool),
    End,
}

/// A key's scan code (set 1).
fn scan_code(name: &str) -> Option<u16> {
    const LETTERS: [u16; 26] = [
        0x1e, 0x30, 0x2e, 0x20, 0x12, 0x21, 0x22, 0x23, 0x17, 0x24, 0x25, 0x26, 0x32, 0x31, 0x18, 0x19, 0x10, 0x13, 0x1f, 0x14, 0x16, 0x2f, 0x11,
        0x2d, 0x15, 0x2c,
    ];
    const FUNCTION: [u16; 12] = [0x3b, 0x3c, 0x3d, 0x3e, 0x3f, 0x40, 0x41, 0x42, 0x43, 0x44, 0x57, 0x58];
    match name {
        "esc" => Some(0x01),
        "enter" => Some(0x1c),
        "space" => Some(0x39),
        "tab" => Some(0x0f),
        "0" => Some(0x0b),
        n if n.len() == 1 && n.as_bytes()[0].is_ascii_digit() => Some(0x02 + u16::from(n.as_bytes()[0] - b'1')),
        n if n.len() == 1 && n.as_bytes()[0].is_ascii_lowercase() => Some(LETTERS[usize::from(n.as_bytes()[0] - b'a')]),
        n if n.starts_with('f') => n[1..].parse::<usize>().ok().filter(|f| (1..=12).contains(f)).map(|f| FUNCTION[f - 1]),
        _ => None,
    }
}

/// Presses and lets go of a key by scan code (what DirectInput games read).
/// A saved pair: its name, the eyes, its pose record, the ring's point (tracking space) and its
/// sequence.
type SavedPair = (String, bool, [ID3D11Texture2D; 2], Option<RenderedPair>, Option<[f32; 3]>, u64);

fn press_key(scan: u16) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, SendInput};
    let event = |up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wScan: scan, dwFlags: if up { KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP } else { KEYEVENTF_SCANCODE }, ..Default::default() } },
    };
    // SAFETY: plain input events of our own.
    unsafe { SendInput(&[event(false)], size_of::<INPUT>() as i32) };
    std::thread::sleep(Duration::from_millis(80));
    // SAFETY: as above.
    unsafe { SendInput(&[event(true)], size_of::<INPUT>() as i32) };
}

/// Brings the biggest visible window of the process named `exe` to the front.
fn focus(exe: &str) -> Result<(), String> {
    use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT};
    use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS};
    use windows::Win32::UI::WindowsAndMessaging::{BringWindowToTop, EnumWindows, GetWindowRect, GetWindowThreadProcessId, IsWindowVisible, SetForegroundWindow};
    use windows::core::BOOL;
    // SAFETY: a process snapshot of our own, closed below.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }.map_err(|e| e.to_string())?;
    let mut entry = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
    let mut pid = None;
    // SAFETY: walks the snapshot with a sized entry.
    let mut more = unsafe { Process32FirstW(snapshot, &mut entry) }.is_ok();
    while more {
        let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
        if String::from_utf16_lossy(&entry.szExeFile[..len]).eq_ignore_ascii_case(exe) {
            pid = Some(entry.th32ProcessID);
            break;
        }
        // SAFETY: as above.
        more = unsafe { Process32NextW(snapshot, &mut entry) }.is_ok();
    }
    // SAFETY: our snapshot.
    let _ = unsafe { CloseHandle(snapshot) };
    let pid = pid.ok_or_else(|| format!("no process {exe}"))?;
    struct Search {
        pid: u32,
        best: Option<(HWND, i64)>,
    }
    unsafe extern "system" fn visit(window: HWND, data: LPARAM) -> BOOL {
        // SAFETY: `data` is the search below, alive for the enumeration.
        let search = unsafe { &mut *(data.0 as *mut Search) };
        let mut owner = 0;
        // SAFETY: plain queries on a window handed to us.
        unsafe {
            GetWindowThreadProcessId(window, Some(&mut owner));
            if owner == search.pid && IsWindowVisible(window).as_bool() {
                let mut rect = RECT::default();
                if GetWindowRect(window, &mut rect).is_ok() {
                    let area = i64::from(rect.right - rect.left) * i64::from(rect.bottom - rect.top);
                    if search.best.is_none_or(|(_, a)| area > a) {
                        search.best = Some((window, area));
                    }
                }
            }
        }
        BOOL(1)
    }
    let mut search = Search { pid, best: None };
    // SAFETY: the callback only touches `search` during the call.
    let _ = unsafe { EnumWindows(Some(visit), LPARAM(&mut search as *mut Search as isize)) };
    let (window, _) = search.best.ok_or_else(|| format!("{exe} has no visible window"))?;
    // A process may take the front only right after input: an Alt tap first.
    use windows::Win32::UI::Input::KeyboardAndMouse::{INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VK_MENU};
    let alt = |up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VK_MENU, dwFlags: if up { KEYEVENTF_KEYUP } else { Default::default() }, ..Default::default() } },
    };
    // SAFETY: input events of our own and a window found above.
    unsafe {
        SendInput(&[alt(false), alt(true)], size_of::<INPUT>() as i32);
        let _ = BringWindowToTop(window);
        if !SetForegroundWindow(window).as_bool() {
            return Err(format!("{exe}'s window would not come to the front"));
        }
    }
    Ok(())
}

fn parse_buttons(text: &str) -> Result<u16, String> {
    if text == "none" {
        return Ok(0);
    }
    text.split('+')
        .map(|b| match b {
            "a" => Ok(button::A),
            "b" => Ok(button::B),
            "x" => Ok(button::X),
            "y" => Ok(button::Y),
            "lb" => Ok(button::LEFT_SHOULDER),
            "rb" => Ok(button::RIGHT_SHOULDER),
            "start" => Ok(button::START),
            "back" => Ok(button::BACK),
            "ls" => Ok(button::LEFT_THUMB),
            "rs" => Ok(button::RIGHT_THUMB),
            "up" => Ok(button::DPAD_UP),
            "down" => Ok(button::DPAD_DOWN),
            "left" => Ok(button::DPAD_LEFT),
            "right" => Ok(button::DPAD_RIGHT),
            other => Err(format!("unknown button {other}")),
        })
        .try_fold(0, |all, b| b.map(|b| all | b))
}

fn parse_script(text: &str) -> Result<Vec<(u64, Step)>, String> {
    let mut steps = Vec::new();
    for (number, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let words: Vec<&str> = line.split_whitespace().collect();
        let bad = |why: &str| format!("line {}: {why}: {line}", number + 1);
        let at: u64 = words[0].parse().map_err(|_| bad("no time"))?;
        let number = |i: usize| -> Result<f32, String> { words.get(i).ok_or_else(|| bad("too few numbers"))?.parse::<f32>().map_err(|_| bad("not a number")) };
        let step = match words.get(1).copied() {
            Some("head") => Step::Head { yaw: number(2)?, pitch: number(3)? },
            Some("hand") => {
                let side = match words.get(2).copied() {
                    Some("left") => 0,
                    Some("right") => 1,
                    _ => return Err(bad("hand takes left or right")),
                };
                let pose = if words.get(3) == Some(&"off") { None } else { Some(([number(3)?, number(4)?, number(5)?], number(6)?, number(7)?)) };
                Step::Hand { side, pose }
            }
            Some("fingers") => {
                let side = match words.get(2).copied() {
                    Some("left") => 0,
                    Some("right") => 1,
                    _ => return Err(bad("fingers takes left or right")),
                };
                let bend = if words.get(3) == Some(&"off") { None } else { Some(number(3)?.to_radians()) };
                Step::Fingers { side, bend }
            }
            Some("pad") => Step::Pad {
                buttons: parse_buttons(words.get(2).ok_or_else(|| bad("pad takes buttons"))?).map_err(|e| bad(&e))?,
                triggers: [number(3)?.clamp(0.0, 255.0) as u8, number(4)?.clamp(0.0, 255.0) as u8],
            },
            Some("stick") => Step::Stick([number(2)?, number(3)?, number(4)?, number(5)?].map(|v| v.clamp(-1.0, 1.0))),
            Some("save") => Step::Save(words.get(2).ok_or_else(|| bad("save takes a name"))?.to_string(), words.get(3) == Some(&"full")),
            Some("reticle") => Step::Reticle(match words.get(2).copied() {
                Some("off") => None,
                Some("left") => Some((0, words.get(3) != Some(&"head"))),
                Some("right") => Some((1, words.get(3) != Some(&"head"))),
                _ => return Err(bad("reticle takes left, right or off")),
            }),
            Some("focus") => Step::Focus(words.get(2).ok_or_else(|| bad("focus takes a program"))?.to_string()),
            Some("key") => Step::Key(words.get(2).and_then(|k| scan_code(k)).ok_or_else(|| bad("unknown key"))?),
            Some("depth") => Step::Depth(words.get(2) != Some(&"off")),
            Some("unpause") => Step::Menu(false),
            Some("pause") => Step::Menu(true),
            Some("layer") => Step::Layer(match words.get(2).copied() {
                Some("layer") => Held { layer: true, ..Held::default() },
                Some("shift") => Held { shift: true, ..Held::default() },
                Some("rest") => Held { rest: true, ..Held::default() },
                Some("none") => Held::default(),
                _ => return Err(bad("layer takes layer, shift, rest or none")),
            }),
            Some("act_like") => Step::ActLike(Proxy::parse(words.get(2).copied().unwrap_or("")).map_err(|e| bad(&e))?),
            Some("dpad_hold") => Step::DpadHold(match words.get(2).copied() {
                Some("none") => 0,
                Some(list) => dpad_directions(list).map_err(|e| bad(&e))?,
                None => return Err(bad("dpad_hold takes directions or none")),
            }),
            Some("reach") => Step::Reach(match menu_reach::parse(words.get(2).copied().unwrap_or("")).map_err(|e| bad(&e))? {
                None => None,
                Some(pick) => {
                    let depth = words.get(3).map(|_| number(3)).transpose()?.map_or(menu_reach::DEPTH, |cm| cm / 100.0);
                    let with = words.get(4).map(|w| menu_reach::parse_with(w)).transpose().map_err(|e| bad(&e))?.unwrap_or(PickWith::Buttons);
                    Some((pick, depth, with))
                }
            }),
            Some("monaka_menu") => Step::MonakaMenu(words.get(2) != Some(&"off")),
            Some("end") => Step::End,
            _ => return Err(bad("unknown command")),
        };
        steps.push((at, step));
    }
    steps.sort_by_key(|(at, _)| *at);
    Ok(steps)
}

/// Yaw about +Y, then pitch about the turned +X (degrees).
fn turn(yaw: f32, pitch: f32) -> [f32; 4] {
    let (sy, cy) = (yaw.to_radians() * 0.5).sin_cos();
    let (sp, cp) = (pitch.to_radians() * 0.5).sin_cos();
    [cy * sp, sy * cp, -sy * sp, cy * cp]
}

/// The viewer's hand-aim ring: this wide (`apps/viewer`), where `math::reticle_point` places it.
const RETICLE_SIZE: f32 = 0.21;
/// A reticle direction the producer said is followed for this long after (ms), as the viewer does.
const RETICLE_SAID_MS: u64 = 250;

/// The pad's way from the script to the producer, as the viewer's from the controllers (see the
/// module's notes): what each stage keeps between reads.
struct PadChain {
    layers: Layers,
    held: Held,
    reach: Option<Reach>,
    /// The Monaka menu's Start hold, and whether the menu is open (none: no menu).
    menu: Option<(HoldToggle, bool)>,
    chord: Chord,
}

impl PadChain {
    fn new() -> Self {
        Self { layers: Layers::new(0, Proxy::None), held: Held::default(), reach: None, menu: Some((HoldToggle::new(button::START), false)), chord: Chord::default() }
    }

    /// The pad the producer gets for the script's `pad` this read, with the head and the hands' grip
    /// positions; says what the viewer would have done on the way.
    fn send(&mut self, mut pad: Gamepad, head: &HeadPose, hands: [Option<[f32; 3]>; 2], at: Instant, ms: u64) -> Gamepad {
        self.layers.apply(&mut pad, self.held, at);
        if let Some(reach) = self.reach.as_mut() {
            for (hand, _, _) in reach.update(&mut pad, Some((head.orientation, head.position)), hands, at) {
                println!("{ms:>6} ms: reach: buzz on the {} hand", ["left", "right"][hand]);
            }
        }
        if let Some((toggle, open)) = self.menu.as_mut() {
            if toggle.toggled(&mut pad, at) {
                *open = !*open;
                println!("{ms:>6} ms: Monaka menu {}", if *open { "opened" } else { "closed" });
            }
            if *open {
                pad = Gamepad::default();
            }
        }
        if self.chord.step(&mut pad, at) {
            println!("{ms:>6} ms: capture chord");
        }
        pad
    }
}

/// A 10-bit (R10G10B10A2) texture's texels rewritten as 8-bit RGBA in place, so the BMP writer
/// reads them as it does every other eye (Control's swapchain is 10-bit).
fn unpack_10_bit(texture: &ID3D11Texture2D, texels: &mut [u8]) {
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R10G10B10A2_TYPELESS, DXGI_FORMAT_R10G10B10A2_UNORM};
    let mut desc = Default::default();
    // SAFETY: fills a descriptor from a live texture.
    unsafe { texture.GetDesc(&mut desc) };
    if desc.Format != DXGI_FORMAT_R10G10B10A2_UNORM && desc.Format != DXGI_FORMAT_R10G10B10A2_TYPELESS {
        return;
    }
    for texel in texels.chunks_exact_mut(4) {
        let packed = u32::from_le_bytes([texel[0], texel[1], texel[2], texel[3]]);
        texel[0] = ((packed & 0x3ff) >> 2) as u8;
        texel[1] = (((packed >> 10) & 0x3ff) >> 2) as u8;
        texel[2] = (((packed >> 20) & 0x3ff) >> 2) as u8;
        texel[3] = 255;
    }
}

/// PNG from tightly packed RGBA rows, half size by point sampling (or `full`), with the ring at
/// `ring` (centre and radius in full-size pixels) drawn over it.
fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8], ring: Option<([f32; 2], f32)>, full: bool) -> std::io::Result<()> {
    let step = if full { 1 } else { 2 };
    let (w, h) = (width / step, height / step);
    let mut pixels: Vec<u8> = (0..h)
        .flat_map(|y| (0..w).map(move |x| ((y * step * width + x * step) * 4) as usize))
        .flat_map(|p| [rgba[p], rgba[p + 1], rgba[p + 2], 255])
        .collect();
    if let Some(([cx, cy], radius)) = ring {
        let scale = 1.0 / step as f32;
        draw_ring(&mut pixels, w, h, [cx * scale, cy * scale], radius * scale);
    }
    std::fs::write(path, png::rgba(w, h, &pixels))
}

/// The viewer's ring image (a white ring with a dark edge and a centre dot, on a 64-unit square)
/// blended over `bgra` at `centre`, `radius` pixels to the square's edge.
fn draw_ring(bgra: &mut [u8], w: u32, h: u32, centre: [f32; 2], radius: f32) {
    let unit = (radius / 32.0).max(0.3);
    let reach = (28.0 * unit).ceil() as i64 + 1;
    let (x0, y0) = (centre[0].round() as i64, centre[1].round() as i64);
    for y in (y0 - reach).max(0)..(y0 + reach).min(i64::from(h)) {
        for x in (x0 - reach).max(0)..(x0 + reach).min(i64::from(w)) {
            let r = ((x as f32 - centre[0]).powi(2) + (y as f32 - centre[1]).powi(2)).sqrt() / unit;
            let band = |inner: f32, outer: f32| ((r - inner + 0.5).clamp(0.0, 1.0)).min((outer - r + 0.5).clamp(0.0, 1.0));
            let white = band(22.0, 26.0).max(band(0.0, 2.5));
            let alpha = white.max(band(20.5, 27.5).max(band(0.0, 4.0)) * 0.6);
            let at = ((y * i64::from(w) + x) * 4) as usize;
            for c in 0..3 {
                bgra[at + c] = (white * 255.0 + (1.0 - alpha) * f32::from(bgra[at + c])) as u8;
            }
        }
    }
}

/// Where a point (tracking space) lands in eye `eye`'s image of `pair` (pixels from the top left
/// of a `width` x `height` image), and how many pixels a metre across there is; `None` behind.
fn project(pair: &RenderedPair, eye: usize, point: [f32; 3], width: u32, height: u32) -> Option<([f32; 2], f32)> {
    let e = &pair.eyes[eye];
    let from = monaka_core::math::eye_position(e, eye);
    let [x, y, z, w] = e.orientation;
    let local = monaka_core::math::rotate([-x, -y, -z, w], [0, 1, 2].map(|k| point[k] - from[k]));
    if local[2] >= -1e-3 {
        return None;
    }
    let [left, right, up, down] = e.fov[eye].map(f32::tan);
    let (tx, ty) = (local[0] / -local[2], local[1] / -local[2]);
    let pixel = [(tx - left) / (right - left) * width as f32, (up - ty) / (up - down) * height as f32];
    let per_metre = width as f32 / (right - left) / -local[2];
    Some((pixel, per_metre))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let usage = "usage: harness <channel name> <script file> <output folder>";
    let mut args = std::env::args().skip(1);
    let name = ChannelName::new(args.next().ok_or(usage)?).ok_or("bad channel name")?;
    let script = parse_script(&std::fs::read_to_string(args.next().ok_or(usage)?)?)?;
    let folder = PathBuf::from(args.next().ok_or(usage)?);
    std::fs::create_dir_all(&folder)?;

    let producer = reader::wait_for(&name, Duration::from_secs(10)).ok_or("no producer on that channel")?.map_err(|mismatch| mismatch.to_string())?;
    let (width, height, format) = (producer.width, producer.height, DXGI_FORMAT(producer.format as i32));
    println!("channel {width}x{height}; {} steps", script.len());
    let (device, context, _) = create_device(Some(&find_adapter(producer.adapter)?))?;
    // The producer can still be creating its textures when it already describes the channel
    // ("no valid fence header"): retry for a few seconds, as the viewer does.
    let opened = std::time::Instant::now();
    let mut reader = loop {
        match FenceReader::open(&name, &device) {
            Ok(reader) => break reader,
            Err(e) if opened.elapsed() < Duration::from_secs(5) => {
                let _ = e;
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(e.into()),
        }
    };
    let mut poses = PoseChannel::open(&name)?;
    let mut hands = HandChannel::open(&name)?;
    let mut fingers = FingerChannel::open(&name)?;
    let texture = || create_texture(&device, &simple_texture_desc(width, height, format));
    let targets: [ID3D11Texture2D; 2] = [texture()?, texture()?];
    let targets = [&targets[0], &targets[1]];

    let mut head = HeadPose { valid: 1, ipd: 0.064, position: [0.0; 3], fov: [LEFT_FOV, RIGHT_FOV], ..HeadPose::default() };
    let mut hand_poses: [Option<([f32; 3], f32, f32)>; 2] = [None, None];
    let mut finger_bends: [Option<f32>; 2] = [None, None];
    let mut gamepad = Gamepad::default();
    let mut chain = PadChain::new();
    let mut reach_set: Option<(Pick, f32, PickWith)> = None;
    let mut reticle_aims = ReticleChannel::open(&name).ok();
    let mut wanted: Vec<(String, bool)> = Vec::new();
    let mut reticle: Option<(usize, bool)> = None;
    let mut menu_wanted: Option<bool> = None;
    let mut start_until: Option<Instant> = None;
    let mut save_depth = false;
    let mut saved_depths: Vec<(String, [ID3D11Texture2D; 2])> = Vec::new();
    let mut saved: Vec<SavedPair> = Vec::new();
    // The HUD panels' readers (opened as their channels appear) and the panels saved with a pair.
    let mut panels: Vec<(&'static str, Option<(FenceReader, ID3D11Texture2D, windows::Win32::Graphics::Direct3D11::D3D11_TEXTURE2D_DESC)>)> = PANELS.iter().map(|p| (*p, None)).collect();
    let mut saved_panels: Vec<(String, ID3D11Texture2D)> = Vec::new();
    let mut next = 0;
    let started = Instant::now();
    let (mut seen, mut pairs) = (0u64, 0u64);
    let mut ended = false;
    loop {
        let now = started.elapsed().as_millis() as u64;
        while next < script.len() && script[next].0 <= now {
            println!("{now:>6} ms: {:?}", script[next].1);
            match &script[next].1 {
                Step::Head { yaw, pitch } => head.orientation = turn(*yaw, *pitch),
                Step::Hand { side, pose } => hand_poses[*side] = *pose,
                Step::Fingers { side, bend } => finger_bends[*side] = *bend,
                Step::Pad { buttons, triggers } => {
                    gamepad.buttons = *buttons;
                    gamepad.left_trigger = triggers[0];
                    gamepad.right_trigger = triggers[1];
                }
                Step::Stick(s) => {
                    let axis = |v: f32| (v * i16::MAX as f32) as i16;
                    (gamepad.thumb_lx, gamepad.thumb_ly, gamepad.thumb_rx, gamepad.thumb_ry) = (axis(s[0]), axis(s[1]), axis(s[2]), axis(s[3]));
                }
                Step::Save(name, full) => wanted.push((name.clone(), *full)),
                Step::Reticle(r) => reticle = *r,
                Step::Focus(exe) => {
                    if let Err(why) = focus(exe) {
                        println!("focus: {why}");
                    }
                }
                Step::Key(scan) => press_key(*scan),
                Step::Menu(open) => menu_wanted = Some(*open),
                Step::Depth(on) => save_depth = *on,
                Step::Layer(held) => chain.held = *held,
                Step::ActLike(proxy) => chain.layers.proxy = *proxy,
                Step::DpadHold(directions) => chain.layers.hold = *directions,
                Step::Reach(set) => reach_set = *set,
                Step::MonakaMenu(on) => chain.menu = on.then(|| (HoldToggle::new(button::START), false)),
                Step::End => ended = true,
            }
            if matches!(script[next].1, Step::Reach(_) | Step::DpadHold(_)) {
                // As the viewer makes it: with the held directions, and only when it can open anything.
                chain.reach = reach_set.map(|(pick, depth, with)| Reach::new(pick, chain.layers.hold, depth).picking_with(with)).filter(Reach::active);
            }
            next += 1;
        }
        // The script done and every frame asked for saved.
        if (ended || next >= script.len()) && wanted.is_empty() && menu_wanted.is_none() && start_until.is_none() {
            break;
        }
        poses.write_head(&head);
        let located = hand_poses.map(|pose| {
            pose.map(|(position, yaw, pitch)| {
                let orientation = turn(yaw, pitch);
                let aim = HandPose { orientation, position, valid: 1, received_tick: tick() };
                let palm = monaka_core::math::rotate(orientation, [0.0, -0.03, 0.08]);
                let grip = HandPose { position: [0, 1, 2].map(|k| position[k] + palm[k]), ..aim };
                (aim, grip)
            })
        });
        let aims = located.map(|h| h.map_or(HandPose::default(), |h| h.0));
        let grips = located.map(|h| h.map_or(HandPose::default(), |h| h.1));
        hands.write(&aims, &grips);
        // Start held a tenth of a second for `unpause` / `pause`, over the script's buttons.
        let mut sent = gamepad;
        if start_until.is_some_and(|until| Instant::now() < until) {
            sent.buttons |= button::START;
        } else {
            start_until = None;
        }
        let held_at = located.map(|h| h.map(|(_, grip)| grip.position));
        let sent = chain.send(sent, &head, held_at, Instant::now(), now);
        hands.write_pad(&PadState { gamepad: sent, valid: 1, received_tick: tick() });
        let fingers_now = finger_bends.map(|bend| match bend {
            Some(b) => FingerPose { valid: 1, received_tick: tick(), fingers: [[b, b, b, 0.0]; 5], ..FingerPose::default() },
            None => FingerPose::default(),
        });
        fingers.write(&fingers_now);
        let read = if save_depth { reader.read_with_depth(&context, targets, seen)?.map(|(pair, depth)| (pair, depth.is_some())) } else { reader.read(&context, targets, seen)?.map(|pair| (pair, false)) };
        if let Some((pair, with_depth)) = read {
            seen = pair.sequence;
            pairs += 1;
            if let Some(open) = menu_wanted.take() {
                // A menu goes out flat: the same image to both eyes.
                let copies = [texture()?, texture()?];
                copy_texture(&context, &copies[0], targets[0]);
                copy_texture(&context, &copies[1], targets[1]);
                let flat = read_image(&device, &context, &copies[0])?.2 == read_image(&device, &context, &copies[1])?.2;
                if flat != open {
                    start_until = Some(Instant::now() + Duration::from_millis(100));
                }
                println!("{:>6} ms: {} (pair {}), {}", started.elapsed().as_millis(), if flat { "a menu is up" } else { "no menu" }, pair.sequence, if flat != open { "Start pressed" } else { "left as it is" });
            }
            // The ring as the viewer places it now: from the palm (the grip's place), along the
            // direction the producer says or the controller's aim, or along the aim from the head.
            let ring = reticle.and_then(|(side, palm)| {
                let (aim, grip) = located[side]?;
                let said = reticle_aims.as_mut().and_then(|r| r.read(side, RETICLE_SAID_MS));
                Some(monaka_core::math::reticle_point(head.position, aim.orientation, grip.position, palm, said))
            });
            for (save, full) in wanted.drain(..) {
                let copies = [texture()?, texture()?];
                copy_texture(&context, &copies[0], targets[0]);
                copy_texture(&context, &copies[1], targets[1]);
                saved.push((save.clone(), full, copies, poses.read_pair(pair.sequence), ring, pair.sequence));
                if with_depth && let Some(depths) = reader.depth_textures() {
                    let desc = monaka_channel::d3d::texture_desc(depths[0]);
                    let copies = [create_texture(&device, &desc)?, create_texture(&device, &desc)?];
                    copy_texture(&context, &copies[0], depths[0]);
                    copy_texture(&context, &copies[1], depths[1]);
                    saved_depths.push((save.clone(), copies));
                }
                for (piece, slot) in &mut panels {
                    if slot.is_none()
                        && let Some(panel_name) = name.panel(piece)
                        && let Some(Ok(description)) = reader::describe_any(&panel_name)
                        && let Ok(reader) = FenceReader::open(&panel_name, &device)
                    {
                        let desc = simple_texture_desc(description.width, description.height, DXGI_FORMAT(description.format as i32));
                        if let Ok(panel) = create_texture(&device, &desc) {
                            println!("HUD panel {piece}: {}x{}", description.width, description.height);
                            *slot = Some((reader, panel, desc));
                        }
                    }
                    if let Some((reader, panel, desc)) = slot
                        && let Ok(Some(_)) = reader.read(&context, [panel, panel], 0)
                    {
                        let copy = create_texture(&device, desc)?;
                        copy_texture(&context, &copy, panel);
                        saved_panels.push((format!("{save}-panel-{piece}"), copy));
                    }
                }
            }
        }
        if now > 600_000 {
            return Err("script ran past ten minutes".into());
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    println!("pairs read {pairs}");
    for (name, full, eyes, record, ring, sequence) in &saved {
        for (index, (eye, side)) in eyes.iter().zip(["left", "right"]).enumerate() {
            let (w, h, mut rgba) = read_image(&device, &context, eye)?;
            unpack_10_bit(eye, &mut rgba);
            let drawn = match (record, ring) {
                (Some(record), Some(point)) => project(record, index, *point, w, h).map(|(at, per_metre)| (at, per_metre * RETICLE_SIZE * 0.5)),
                _ => None,
            };
            write_png(&folder.join(format!("{name}-{side}.png")), w, h, &rgba, drawn, *full)?;
        }
        println!("saved {name} (pair {sequence}){}", if ring.is_some() && record.is_none() { " (no pose record: ring not drawn)" } else { "" });
    }
    for (name, depths) in &saved_depths {
        for (depth, side) in depths.iter().zip(["left", "right"]) {
            let Ok((w, h, texels)) = read_image(&device, &context, depth) else {
                println!("depth of {name}: not 4-byte texels");
                break;
            };
            let values: Vec<f32> = texels.chunks_exact(4).map(|t| f32::from_le_bytes([t[0], t[1], t[2], t[3]])).collect();
            let finite = values.iter().copied().filter(|v| v.is_finite());
            let (low, high) = finite.fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)));
            let spread = (high - low).max(1e-12);
            // Reversed or standard depth: the brighter end is whichever holds less of the image (the near hands).
            let near_high = values.iter().filter(|&&v| v > (low + high) * 0.5).count() < values.len() / 2;
            let rgba: Vec<u8> = values
                .iter()
                .flat_map(|&v| {
                    let t = ((v - low) / spread).clamp(0.0, 1.0);
                    let g = ((if near_high { t } else { 1.0 - t }).powf(0.35) * 255.0) as u8;
                    [g, g, g, 255]
                })
                .collect();
            write_png(&folder.join(format!("{name}-depth-{side}.png")), w, h, &rgba, None, false)?;
        }
    }
    for (name, panel) in &saved_panels {
        let (w, h, mut rgba) = read_image(&device, &context, panel)?;
        // Premultiplied over mid grey, so the panel's shape shows.
        for px in rgba.chunks_exact_mut(4) {
            let a = u32::from(px[3]);
            for c in &mut px[..3] {
                *c = (u32::from(*c) + 128 * (255 - a) / 255).min(255) as u8;
            }
        }
        write_png(&folder.join(format!("{name}.png")), w, h, &rgba, None, false)?;
        println!("saved {name}");
    }
    Ok(())
}

/// The HUD pieces a producer may publish beside its channel (the viewer's panels).
const PANELS: [&str; 7] = ["minimap", "quests", "weapon", "health", "tool", "weapon_left", "layer"];
