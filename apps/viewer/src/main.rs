//! Shows a game's stereo pairs in the headset and sends the head pose back to the game.
//!
//! Command line:
//!
//! ```text
//! monaka_viewer --gpu-map <channel> --loader <openxr_loader.dll> [--runtime <manifest.json>]
//!             [--seconds N] [--pixel-aspect A] [--screen-width M] [--reticle left|right]... [--reticle-from head|palm]
//!             [--dpad-hold left,right] [--menu-reach same|other|off] [--menu-pick buttons|stick] [--menu-reach-depth CM] [--act-like touch|index]
//!             [--controller-smoothing 0-10] [--stop-event <name>] [--captures <folder>]
//! monaka_viewer --info --loader <openxr_loader.dll> [--runtime <manifest.json>]
//! monaka_viewer --gpu-map <channel> --desktop [--seconds N] [--captures <folder>] [--stop-event <name>]
//! ```
//!
//! Pairs that come with the head poses they were rendered from go out as a projection layer, so
//! the runtime reprojects them. Pairs without poses (menus, maps, loading screens) go on a flat
//! screen anchored in the room in front of where the head faced. Where each controller points
//! (its aim pose) and where it sits in the hand (its grip pose) go to the game through the hand
//! block, for hand aim and for putting the game's hand on the controller, steadied first
//! ([`smooth`]). Where the eyes look goes with each head pose when the headset tracks them
//! ([`gaze`]), and each eye's depth goes with the projection layer when the producer sends it and
//! the runtime takes it ([`depth`]). Start and Back held together save what the headset shows
//! ([`chord`], [`capture`]).
#![deny(unsafe_code)]

mod capture;
mod clipboard;
mod console;
mod console_feed;
mod debug_menu;
mod debug_panels;
mod debug_views;
mod depth;
mod desktop;
mod gaze;
mod menu_quad;
mod meter;
mod monaka_menu;
mod pad;
mod panel;
mod recorder;
mod smooth;
mod view_commands;

use monaka_channel::d3d::{self, create_texture, find_adapter, simple_texture_desc};
use monaka_channel::fingers::FingerChannel;
use monaka_channel::reticles::ReticleChannel;
use monaka_channel::hands::HandChannel;
use monaka_channel::pose::PoseChannel;
use monaka_channel::reader::{self, FenceReader};
use monaka_channel::{ChannelName, tick};
use monaka_core::chord;
use monaka_core::menu_reach as reach;
use monaka_core::math;
use monaka_core::protocol::{FingerPose, HandPose, HeadPose, LEFT_HAND, PadState, RIGHT_HAND, RenderedPair, dxgi_format};
use monaka_openxr::{Event, Extent2Df, Fovf, Instance, Layer, Loader, Posef, ProjectionView, sys};
use std::error::Error;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use windows::Win32::Graphics::Direct3D11::D3D11_BIND_SHADER_RESOURCE;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;

/// Distance of the flat menu screen in front of the head, in metres.
const SCREEN_DISTANCE: f32 = 2.0;
/// A producer that publishes nothing for this long has stopped; the viewer ends.
const PRODUCER_SILENCE_MS: u64 = 15_000;

/// The producer's channel once it is made (with its first frame), waiting up to `seconds`; a stop
/// asked for meanwhile (the game gone, the player) ends the wait.
pub(crate) fn wait_for_producer(name: &ChannelName, seconds: u64) -> Result<reader::Description, String> {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    loop {
        match reader::wait_for(name, Duration::from_millis(100)) {
            Some(Ok(description)) => return Ok(description),
            Some(Err(mismatch)) => return Err(mismatch.to_string()),
            None => {}
        }
        if console::stop_requested() {
            return Err("stopped before the game drew a frame".into());
        }
        if Instant::now() >= deadline {
            return Err("no GPU stereo producer became ready".into());
        }
    }
}

/// What the viewer changes for a [`monaka_core::manifest::LIVE`] option set in the headset's Monaka
/// menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Live {
    Smoothing,
    Reach,
    ReachDepth,
    Proxy,
}

/// The viewer's part in live option `id`, if it has one.
fn live_setting(id: &str) -> Option<Live> {
    use monaka_core::manifest::{CONTROLLER_PROXY, CONTROLLER_SMOOTHING, MENU_REACH_DEPTH, menu_reach};
    [(CONTROLLER_SMOOTHING.id, Live::Smoothing), (menu_reach("").id, Live::Reach), (MENU_REACH_DEPTH.id, Live::ReachDepth), (CONTROLLER_PROXY.id, Live::Proxy)]
        .into_iter()
        .find_map(|(live, what)| (live == id).then_some(what))
}

/// A block beside the stereo channel, opened for this run: `None` when it could not be (said in
/// the log, and the run goes on without it), an error when the game pack speaks another version of
/// it (nothing it carries would be read right).
pub(crate) fn side_channel<T>(what: &str, opened: monaka_channel::Result<T>) -> Result<Option<T>, String> {
    match opened {
        Ok(channel) => Ok(Some(channel)),
        Err(monaka_channel::Error::Version(mismatch)) => Err(mismatch.to_string()),
        Err(e) => {
            println!("No {what}: {e}");
            Ok(None)
        }
    }
}

struct Args {
    channel: Option<ChannelName>,
    info: bool,
    seconds: u64,
    /// How long to wait for the producer's first frame (`--producer-wait`): longer for a game
    /// joined as it starts, which reaches its first frame later.
    producer_wait: u64,
    pixel_aspect: f32,
    screen_width: f32,
    loader: PathBuf,
    runtime: Option<PathBuf>,
    /// The hands (0 left, 1 right) whose aim the game follows: a reticle shows where each points.
    /// `--reticle` once per hand (Dying Light 1: the right hand's weapon, the left hand's tool).
    reticles: [bool; 2],
    /// The reticle sits where a weapon held in the palm points ([`math::converged`]) instead of on
    /// the ray from the head: the game aims there when its hand is put on the controller.
    reticle_palm: bool,
    panel_nearest: f32,
    /// The d-pad directions held for the player once pushed through the d-pad layer (`pad`).
    dpad_hold: u16,
    /// Those menus also open by reaching over a shoulder, picked with this stick ([`reach`]); `None`:
    /// not by reach.
    menu_reach: Option<reach::Pick>,
    /// How a pick from those menus reaches the game: a face button, or the game's right stick.
    menu_pick: reach::PickWith,
    /// How far behind the head a hand reaches to open one (metres; `--menu-reach-depth` in cm).
    menu_reach_depth: f32,
    /// The controllers act like another kind (`--act-like touch|index`, a layout test).
    proxy: pad::Proxy,
    /// How hard the controllers' poses are steadied ([`smooth`]; 0 off, 10 strongest).
    controller_smoothing: f32,
    /// Where captures of what the headset shows go ([`capture`]; the run's folder).
    captures: Option<PathBuf>,
    /// Back held opens the debug menu (`--debug-menu`); without it Back is a plain button.
    debug_menu: bool,
    /// The options the Monaka menu (Start held) offers, `id=value,id=value` (`--live`).
    live: String,
    /// The 2D window instead of the headset (`--desktop`, [`desktop`]).
    desktop: bool,
}

/// The launcher passes `--controller-smoothing` only away from Monaka's default, so that is ours too.
fn smoothing_default() -> f32 {
    match monaka_core::manifest::CONTROLLER_SMOOTHING.kind {
        monaka_core::manifest::Kind::Number { default, .. } => default as f32,
        _ => 0.0,
    }
}

fn parse_args() -> Result<Args, String> {
    let mut args =
        Args { channel: None, info: false, seconds: 30, producer_wait: 30, pixel_aspect: 1.0, screen_width: 2.4, loader: PathBuf::new(), runtime: None, reticles: [false; 2], reticle_palm: false, panel_nearest: 0.0, dpad_hold: 0, menu_reach: Some(reach::Pick::Same), menu_pick: reach::PickWith::Buttons, menu_reach_depth: reach::DEPTH, proxy: pad::Proxy::None, controller_smoothing: smoothing_default(), captures: None, debug_menu: false, live: String::new(), desktop: false };
    let mut words = std::env::args().skip(1);
    while let Some(flag) = words.next() {
        if flag == "--info" {
            args.info = true;
            continue;
        }
        if flag == monaka_core::manifest::DESKTOP_ARG {
            args.desktop = true;
            continue;
        }
        if flag == monaka_core::manifest::DEBUG_MENU_ARG {
            args.debug_menu = true;
            continue;
        }
        let value = words.next().ok_or_else(|| format!("{flag} needs a value"))?;
        let number = |what: &str| value.parse::<f64>().map_err(|_| format!("{what} must be a number"));
        match flag.as_str() {
            "--gpu-map" => args.channel = Some(ChannelName::new(value.clone()).ok_or("bad channel name")?),
            "--seconds" => args.seconds = number("--seconds")? as u64,
            "--producer-wait" => args.producer_wait = number("--producer-wait")? as u64,
            "--pixel-aspect" => args.pixel_aspect = number("--pixel-aspect")? as f32,
            "--screen-width" => args.screen_width = number("--screen-width")? as f32,
            // The hand panels sit no nearer the head than this (metres; 0: on the hand as it is).
            "--panel-nearest" => args.panel_nearest = number("--panel-nearest")? as f32,
            // Directions of a choice held open with the d-pad (`left,right`).
            "--dpad-hold" => args.dpad_hold = pad::dpad_directions(&value)?,
            // Those menus by reaching over a shoulder: picked with the same hand's stick, the
            // other's, or off.
            "--menu-reach" => args.menu_reach = reach::parse(&value)?,
            "--menu-pick" => args.menu_pick = reach::parse_with(&value)?,
            "--menu-reach-depth" => {
                let cm = number("--menu-reach-depth")?;
                if !(5.0..=40.0).contains(&cm) {
                    return Err("--menu-reach-depth is 5 to 40 (cm)".into());
                }
                args.menu_reach_depth = cm as f32 / 100.0;
            }
            "--controller-smoothing" => {
                let strength = number("--controller-smoothing")?;
                if !(0.0..=10.0).contains(&strength) {
                    return Err("--controller-smoothing is 0 to 10".into());
                }
                args.controller_smoothing = strength as f32;
            }
            // A layout test: the controllers act like Quest's or Index's.
            "--act-like" => args.proxy = pad::Proxy::parse(&value)?,
            // The options the Monaka menu offers, with their values.
            monaka_core::manifest::LIVE_ARG => args.live = value,
            "--loader" => args.loader = PathBuf::from(&value),
            "--runtime" => args.runtime = Some(PathBuf::from(&value)),
            "--reticle" => {
                let hand = match value.as_str() {
                    "left" => LEFT_HAND,
                    "right" => RIGHT_HAND,
                    _ => return Err("--reticle must be left or right".into()),
                };
                args.reticles[hand] = true;
            }
            "--reticle-from" => {
                args.reticle_palm = match value.as_str() {
                    "head" => false,
                    "palm" => true,
                    _ => return Err("--reticle-from must be head or palm".into()),
                }
            }
            // A launcher without a console stops the viewer by setting this event.
            "--stop-event" => console::watch_stop_event(&value)?,
            "--captures" => args.captures = Some(PathBuf::from(&value)),
            // Accepted and ignored for launch scripts that pass it: the gaze goes to the game with
            // every head pose when the headset tracks the eyes.
            "--gaze-log" => {}
            _ => return Err(format!("unknown argument {flag}")),
        }
    }
    if !(1..=86_400).contains(&args.seconds) {
        return Err("--seconds must be 1..86400".into());
    }
    if !(0.25..=4.0).contains(&args.pixel_aspect) {
        return Err("--pixel-aspect must be 0.25..4".into());
    }
    if !args.desktop && !args.loader.is_absolute() {
        return Err("provide an absolute --loader path".into());
    }
    if let Some(runtime) = &args.runtime
        && (!runtime.is_absolute() || !runtime.is_file())
    {
        return Err("--runtime must be an existing absolute path".into());
    }
    if args.info == args.channel.is_some() {
        return Err("give either --info or --gpu-map".into());
    }
    Ok(args)
}

/// The stereo channel `name` opened on `device`, tried for five seconds: the producer may still be
/// making its textures when it already describes the channel.
pub(crate) fn open_channel(name: &ChannelName, device: &windows::Win32::Graphics::Direct3D11::ID3D11Device) -> Result<FenceReader, String> {
    let open_by = Instant::now() + Duration::from_secs(5);
    loop {
        match FenceReader::open(name, device) {
            Ok(input) => return Ok(input),
            Err(e) if Instant::now() >= open_by => return Err(format!("cannot open the GPU channel: {e}")),
            Err(_) => std::thread::sleep(Duration::from_millis(3)),
        }
    }
}

/// Gives each of `buzzes` on its hand, when the controllers can vibrate.
fn buzz(session: &monaka_openxr::Session<'_>, haptic: Option<&monaka_openxr::Action<'_>>, hand_paths: &[sys::Path; 2], buzzes: impl IntoIterator<Item = debug_menu::Buzz>) {
    let Some(haptic) = haptic else { return };
    for (hand, amplitude, seconds) in buzzes {
        let _ = session.vibrate(haptic, hand_paths[hand], amplitude, seconds);
    }
}

/// What the producer publishes, learned before anything is opened.
type Producer = reader::Description;

/// Swapchain images are filled by copy, so they must be in the producer's format family.
fn swapchain_format(offered: &[i64], producer: &Producer) -> Option<u32> {
    use dxgi_format::*;
    let has = |f: u32| offered.contains(&(f as i64));
    if producer.format != R8G8B8A8_UNORM {
        let srgb = if producer.format == B8G8R8A8_UNORM { B8G8R8A8_UNORM_SRGB } else { producer.format };
        return [producer.format, srgb].into_iter().find(|&f| has(f));
    }
    [R8G8B8A8_UNORM, R8G8B8A8_UNORM_SRGB].into_iter().find(|&f| has(f))
}

fn fov_angles(f: Fovf) -> [f32; 4] {
    [f.angleLeft, f.angleRight, f.angleUp, f.angleDown]
}

/// What the runtime offers for eye tracking, through either extension.
fn eye_tracking_report(instance: &Instance, system: monaka_openxr::SystemId) -> [String; 2] {
    let report = |name: &std::ffi::CStr, supported: bool| {
        let state = if supported {
            "supported"
        } else if instance.has_extension(name) {
            "offered, but this headset reports no eye tracking"
        } else {
            "not offered"
        };
        format!("Eye tracking ({}): {state}", name.to_string_lossy())
    };
    [report(sys::EXT_EYE_GAZE_INTERACTION, instance.eye_gaze(system)), report(sys::FB_EYE_TRACKING_SOCIAL, instance.eye_tracking_fb(system))]
}

#[derive(Default)]
struct Stats {
    frames: u64,
    visible: u64,
    pairs: u64,
    poses_written: u64,
    gazes: u64,
    hands_written: u64,
    fingers_written: u64,
    projected: u64,
    with_depth: u64,
}

/// Controller profiles whose aim and grip poses the viewer suggests; the runtime uses whichever
/// matches.
const AIM_PROFILES: [&str; 6] = [
    "/interaction_profiles/valve/frame_controller",
    "/interaction_profiles/oculus/touch_controller",
    "/interaction_profiles/valve/index_controller",
    "/interaction_profiles/htc/vive_controller",
    "/interaction_profiles/microsoft/motion_controller",
    "/interaction_profiles/khr/simple_controller",
];
const LEFT_AIM: &str = "/user/hand/left/input/aim/pose";
const RIGHT_AIM: &str = "/user/hand/right/input/aim/pose";
const LEFT_GRIP: &str = "/user/hand/left/input/grip/pose";
const RIGHT_GRIP: &str = "/user/hand/right/input/grip/pose";

/// A reticle direction the producer said is followed for this long after (ms).
const RETICLE_SAID_MS: u64 = 250;
/// The hand-aim reticle: this wide, at its distance (`math::reticle_point`, 1.2 degrees).
const RETICLE_SIZE: f32 = 0.21;
const RETICLE_PIXELS: u32 = 64;

/// A white ring with a dark edge and a centre dot, premultiplied RGBA, `size` pixels square.
fn reticle_image(size: u32) -> Vec<u8> {
    let centre = (size as f32 - 1.0) * 0.5;
    let unit = size as f32 / 64.0;
    let mut pixels = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let r = ((x as f32 - centre).powi(2) + (y as f32 - centre).powi(2)).sqrt() / unit;
            // Coverage of a band [inner, outer] with a one-pixel soft edge.
            let band = |inner: f32, outer: f32| ((r - inner + 0.5).clamp(0.0, 1.0)).min((outer - r + 0.5).clamp(0.0, 1.0));
            let white = band(22.0, 26.0).max(band(0.0, 2.5));
            let edge = band(20.5, 27.5).max(band(0.0, 4.0));
            let alpha = white.max(edge * 0.6);
            let level = (white * 255.0) as u8;
            pixels.extend_from_slice(&[level, level, level, (alpha * 255.0) as u8]);
        }
    }
    pixels
}

fn run(args: Args) -> Result<i32, Box<dyn Error>> {
    if args.desktop {
        let name = args.channel.as_ref().ok_or("the 2D window needs --gpu-map")?;
        return desktop::run(name, args.seconds, args.producer_wait, args.captures.clone());
    }
    let producer = match &args.channel {
        // A producer may first switch the game to the headset's resolution.
        Some(name) => Some(wait_for_producer(name, args.producer_wait)?),
        None => None,
    };
    if let Some(p) = &producer {
        println!("GPU stereo input: {}x{} format {} producer adapter={}:{}", p.width, p.height, p.format, p.adapter.high, p.adapter.low);
    }
    if let Some(runtime) = &args.runtime {
        console::set_runtime_override(runtime);
    }

    let instance = Instance::create(Loader::load(&args.loader)?, "Monaka VR viewer")?;
    println!("Runtime: {}", instance.runtime_name()?);
    let system = instance.headset().map_err(|e| format!("{e} (connect the headset and start SteamVR)"))?;
    let headset = instance.system_info(system)?;
    println!("Headset: {}", headset.name);
    let (runtime_luid, min_feature_level) = instance.d3d11_requirements(system)?;
    let runtime_adapter = d3d::luid(runtime_luid);
    println!("Required adapter LUID: {}:{} minimum feature level={min_feature_level:#x}", runtime_adapter.high, runtime_adapter.low);
    let views = instance.stereo_views(system)?;
    for (eye, v) in views.iter().enumerate() {
        println!(
            "Recommended eye image {eye}: {}x{} (max {}x{})",
            v.recommendedImageRectWidth, v.recommendedImageRectHeight, v.maxImageRectWidth, v.maxImageRectHeight
        );
    }
    let depth_state = if instance.depth_layer() { "offered" } else { "not offered" };
    println!("Depth layer ({}): {depth_state}", sys::KHR_COMPOSITION_LAYER_DEPTH.to_string_lossy());
    for line in eye_tracking_report(&instance, system) {
        println!("{line}");
    }
    let Some(producer) = producer else {
        // --info: the DL2 launcher renders the game at this size.
        if let Some(width) = views.iter().map(|v| v.recommendedImageRectWidth).max() {
            let height = views.iter().map(|v| v.recommendedImageRectHeight).max().unwrap_or(0);
            println!("Eye image size: {width} {height}");
        }
        return Ok(0);
    };
    let name = args.channel.as_ref().expect("checked by parse_args");

    if headset.max_layers < 2 {
        return Err("runtime cannot submit two eye-specific layers".into());
    }
    if producer.width > headset.max_image_width || producer.height > headset.max_image_height {
        return Err("eye images exceed the runtime's image limit".into());
    }
    if !instance.supports_opaque_stereo(system)? {
        return Err("runtime has no opaque stereo mode".into());
    }
    if producer.adapter != runtime_adapter {
        return Err("the game renders on a different GPU than the headset runtime".into());
    }
    let adapter = find_adapter(runtime_adapter)?;
    println!("GPU: {}", d3d::describe_adapter(&adapter));
    let (device, context, level) = d3d::create_device(Some(&adapter))?;
    if level.0 < min_feature_level {
        return Err("GPU feature level is below the runtime's minimum".into());
    }

    // Controllers: where each one points, for the game's hand aim. Created before the session
    // (the action set must outlive it). Optional: without them the viewer still shows the game.
    let hand_paths = [instance.path("/user/hand/left")?, instance.path("/user/hand/right")?];
    let actions = instance.action_set("monaka", "Monaka VR").map_err(|e| println!("No controller input: {e}")).ok();
    let pose_action = |name: &str, label: &str| actions.as_ref().and_then(|set| set.pose_action(name, label, &hand_paths).map_err(|e| println!("No controller input: {e}")).ok());
    let (aim, grip) = (pose_action("aim", "Aim"), pose_action("grip", "Grip"));
    // The controllers' buttons, triggers and sticks, as a gamepad for the game.
    if args.proxy != pad::Proxy::None {
        println!("Controller test: acting like {:?} controllers (the d-pad they lack is hidden; the left grip stands in for their d-pad layer)", args.proxy);
    }
    let pad_actions = actions.as_ref().and_then(|set| pad::PadActions::create(set, args.dpad_hold, args.proxy).map_err(|e| println!("No controller buttons: {e}")).ok());
    // The held menus by reaching over a shoulder, and the buzz that says one opened or picked.
    // Made again when the Monaka menu changes how it picks or how deep the reach is.
    let make_reach = |pick: Option<reach::Pick>, depth: f32| {
        pick.map(|pick| reach::Reach::new(pick, args.dpad_hold, depth).picking_with(args.menu_pick)).filter(|r| r.active() && pad_actions.is_some())
    };
    let (mut reach_pick, mut reach_depth) = (args.menu_reach, args.menu_reach_depth);
    let mut reach = make_reach(reach_pick, reach_depth);
    if let (Some(pick), true) = (args.menu_reach, reach.is_some()) {
        println!("Menus by reach: rest a hand over its shoulder to open its menu; flick {} stick to pick", if pick == reach::Pick::Same { "that hand's" } else { "the other hand's" });
    }
    let haptic = actions.as_ref().and_then(|set| set.vibration_action("haptic", "Vibration", &hand_paths).map_err(|e| println!("No controller vibration: {e}")).ok());
    // Start and Back held together: a capture of what the headset shows.
    let mut chord = chord::Chord::default();
    let mut capture = args.captures.clone().map(capture::Capture::new);
    if capture.is_some() && pad_actions.is_some() {
        println!("Capture: hold Start and Back together to save what the headset shows");
    }
    if let (Some(aim), Some(grip)) = (&aim, &grip) {
        // One suggestion per profile carries all of its bindings (a second call replaces the first).
        let poses = [(aim, LEFT_AIM), (aim, RIGHT_AIM), (grip, LEFT_GRIP), (grip, RIGHT_GRIP)];
        let mut profiles = Vec::new();
        for profile in AIM_PROFILES {
            // The gamepad's buttons ride along; a profile that refuses one of them still gets its
            // poses (hand aim and the hand), and its buttons can be bound in the runtime's screen.
            let buttons: Vec<(&str, String)> = pad_actions.as_ref().map(|_| pad::default_bindings(profile)).unwrap_or_default();
            let mut all: Vec<(&monaka_openxr::Action<'_>, &str)> = poses.to_vec();
            if let Some(pad) = &pad_actions {
                all.extend(buttons.iter().filter_map(|(name, path)| Some((pad.action(name)?, path.as_str()))));
            }
            // Each set of extras with the defaults, then the defaults alone.
            let mut extras = if pad_actions.is_some() { pad::extra_bindings(profile) } else { Vec::new() };
            extras.push(Vec::new());
            // Each with the vibration first, then without (a profile may refuse it).
            let accepted = extras.iter().flat_map(|extra| [(extra, true), (extra, false)]).find_map(|(extra, buzz)| {
                let mut with = all.clone();
                with.extend(extra.iter().filter_map(|(name, path)| Some((pad_actions.as_ref()?.action(name)?, path.as_str()))));
                let buttons = with.len() - poses.len();
                if buzz && let Some(haptic) = &haptic {
                    with.extend([(haptic, "/user/hand/left/output/haptic"), (haptic, "/user/hand/right/output/haptic")]);
                }
                instance.suggest_bindings(profile, &with).is_ok().then(|| (buttons, extra.iter().map(|(_, p)| p.as_str()).collect::<Vec<_>>()))
            });
            if let Some((count, extra)) = accepted {
                let extra = if extra.is_empty() { String::new() } else { format!("; also {}", extra.join(", ")) };
                profiles.push(format!("{profile} ({count} buttons{extra})"));
            } else if instance.suggest_bindings(profile, &poses).is_ok() {
                profiles.push(format!("{profile} (poses only: a default button binding was refused)"));
            }
        }
        println!("Controller profiles: {}", if profiles.is_empty() { "none accepted".into() } else { profiles.join(", ") });
    }
    // Eye gaze as a pose action in the same set, bound through its own interaction profile.
    let gaze_action = actions.as_ref().filter(|_| instance.eye_gaze(system)).and_then(|set| {
        let action = set.pose_action("gaze", "Eye gaze", &[]).map_err(|e| println!("No eye gaze action: {e}")).ok()?;
        instance.suggest_bindings(gaze::PROFILE, &[(&action, gaze::BINDING)]).map_err(|e| println!("No eye gaze binding: {e}")).ok()?;
        Some(action)
    });

    let session = instance.create_session(system, &device)?;
    let local = session.reference_space(sys::REFERENCE_SPACE_TYPE_LOCAL)?;
    let view_space = session.reference_space(sys::REFERENCE_SPACE_TYPE_VIEW)?;
    // The action set is attached once, with all its actions: the controllers' and the gaze's.
    let attached = actions
        .as_ref()
        .filter(|_| (aim.is_some() && grip.is_some()) || gaze_action.is_some())
        .filter(|set| session.attach(&[set]).map_err(|e| println!("No controller input: {e}")).is_ok());
    let controllers = match (attached, &aim, &grip) {
        (Some(_), Some(aim), Some(grip)) => (|| -> monaka_openxr::Result<_> {
            let spaces = |action| -> monaka_openxr::Result<_> { Ok([session.action_space(action, hand_paths[0])?, session.action_space(action, hand_paths[1])?]) };
            Ok([(aim, spaces(aim)?), (grip, spaces(grip)?)])
        })()
        .map_err(|e| println!("No controller input: {e}"))
        .ok(),
        _ => None,
    };
    // Where the eyes look, from the gaze action, else from Meta's eye tracker.
    let gaze_source = match (attached, &gaze_action) {
        (Some(_), Some(action)) => session.action_space(action, sys::NULL_HANDLE).map(|space| gaze::Source::Action(action, space)).map_err(|e| println!("No eye gaze space: {e}")).ok(),
        _ => None,
    }
    .or_else(|| instance.eye_tracking_fb(system).then(|| session.eye_tracker().map(gaze::Source::Tracker).map_err(|e| println!("No eye tracker: {e}")).ok()).flatten());
    println!(
        "{}",
        match &gaze_source {
            Some(gaze::Source::Action(..)) => "Eye gaze on (XR_EXT_eye_gaze_interaction), sent with each head pose",
            Some(gaze::Source::Tracker(_)) => "Eye gaze on (XR_FB_eye_tracking_social), sent with each head pose",
            None => "No eye gaze (the headset or runtime offers none)",
        }
    );
    let mut hands_out = match controllers {
        Some(_) => side_channel("hand channel", HandChannel::open(name))?,
        None => None,
    };
    // Fingers, where the headset tracks them (from the controllers' finger sensing, or cameras).
    let finger_trackers: Option<[_; 2]> = instance
        .hand_tracking(system)
        .then(|| match (session.hand_tracker(0), session.hand_tracker(1)) {
            (Ok(left), Ok(right)) => Some([left, right]),
            (Err(e), _) | (_, Err(e)) => {
                println!("No finger tracking: {e}");
                None
            }
        })
        .flatten();
    let mut fingers_out = match finger_trackers {
        Some(_) => side_channel("finger channel", FingerChannel::open(name))?,
        None => None,
    };
    println!("{}", if fingers_out.is_some() { "Finger tracking on" } else { "No finger tracking (the headset or runtime offers none)" });
    let mut fingers_seen = [false; 2];
    let mut hands_seen = [false; 2];
    let mut smoother = smooth::Smoother::new(args.controller_smoothing);
    if controllers.is_some() {
        println!("Controller smoothing {}", args.controller_smoothing);
    }
    let mut poses = side_channel("head pose channel (no head tracking)", PoseChannel::open(name))?;
    // Where the producer says a reticle points, when not along the palm (a bow at an angle in it).
    let mut reticle_aims = if args.reticles.contains(&true) { side_channel("reticle channel", ReticleChannel::open(name))? } else { None };
    println!("Forward follows the headset: use its centre/recentre control.");

    let offered_formats = session.swapchain_formats()?;
    let format = swapchain_format(&offered_formats, &producer).ok_or("runtime has no image format matching the game's")?;
    let swapchains = [
        debug_views::eye_swapchain(&session, format as i64, producer.width, producer.height)?,
        debug_views::eye_swapchain(&session, format as i64, producer.width, producer.height)?,
    ];
    // The hand-aim reticles, one swapchain per hand: filled once; a layer keeps showing the last
    // image released.
    let mut reticles = [None, None];
    if args.reticles.contains(&true) {
        let image = d3d::create_texture_with(&device, &simple_texture_desc(RETICLE_PIXELS, RETICLE_PIXELS, DXGI_FORMAT(format as i32)), &reticle_image(RETICLE_PIXELS))?;
        for (reticle, _) in reticles.iter_mut().zip(args.reticles).filter(|(_, on)| *on) {
            let swapchain = session.create_swapchain(format as i64, RETICLE_PIXELS, RETICLE_PIXELS)?;
            swapchain.fill(|target| {
                d3d::copy_texture(&context, target, &image);
                d3d::flush(&context);
            })?;
            *reticle = Some(swapchain);
        }
    }
    // HUD pieces the game publishes on their own, on the hands where it says.
    let declared = side_channel("HUD panels block", monaka_channel::panels::PanelsChannel::open(name))?;
    let mut hand_panels = panel::HandPanels::new(name, format, declared);
    let mut source_desc = simple_texture_desc(producer.width, producer.height, DXGI_FORMAT(producer.format as i32));
    source_desc.BindFlags = D3D11_BIND_SHADER_RESOURCE.0 as u32;
    let sources = [create_texture(&device, &source_desc)?, create_texture(&device, &source_desc)?];
    let eyes = [&sources[0], &sources[1]];
    let mut debug_views = debug_views::DebugViews::new(name, DXGI_FORMAT(format as i32), args.captures.clone());
    let mut console_feed = console_feed::ConsoleFeed::new(name);
    let mut monaka_menu = monaka_menu::MonakaMenu::new(DXGI_FORMAT(format as i32), &args.live);
    if !args.live.is_empty() && pad_actions.is_some() {
        println!("Monaka menu: hold Start");
    }

    let mut input = open_channel(name, &device).map_err(|e| format!("{e} on the runtime adapter"))?;
    let ready_by = Instant::now() + Duration::from_secs(10);
    let mut seen = loop {
        if let Some(pair) = input.read(&context, eyes, 0)? {
            break pair.sequence;
        }
        if Instant::now() >= ready_by {
            return Err("the producer has not published a stereo pair".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    };

    console::catch_ctrl_c();
    let mut stats = Stats { pairs: 1, ..Stats::default() };
    let mut meter = meter::Meter::new("Headset", [producer.width, producer.height], Instant::now());
    let (mut running, mut exit, mut exit_requested) = (false, false, false);
    let deadline = Instant::now() + Duration::from_secs(args.seconds);
    let mut exit_deadline = deadline + Duration::from_secs(5);
    let mut last_pair_at = tick();
    let mut rendered: Option<RenderedPair> = None;
    let mut last_head: Option<HeadPose> = None;
    let mut screen: Option<Posef> = None;
    // The newest pair's depth, read only when the runtime takes depth, and its swapchains.
    let depth_layer = instance.depth_layer();
    let mut pair_depth: Option<monaka_channel::fence::ReadDepth> = None;
    let mut depth_chains: Option<depth::DepthChains<'_>> = None;
    let image_aspect = producer.height as f32 / (producer.width as f32 * args.pixel_aspect);
    println!("Live stereo; update cadence set by the producer. Ctrl+C exits.");

    while !exit {
        while let Some(event) = instance.poll_event()? {
            match event {
                Event::SessionState(sys::SESSION_STATE_READY) if !running && !exit_requested => {
                    session.begin()?;
                    running = true;
                }
                Event::SessionState(sys::SESSION_STATE_STOPPING) => {
                    if running {
                        session.end()?;
                    }
                    running = false;
                    exit |= exit_requested;
                }
                Event::SessionState(sys::SESSION_STATE_EXITING | sys::SESSION_STATE_LOSS_PENDING) | Event::InstanceLossPending => exit = true,
                _ => {}
            }
        }
        let now = Instant::now();
        if !exit_requested && (console::stop_requested() || now >= deadline) {
            exit_requested = true;
            if running {
                session.request_exit()?;
                exit_deadline = now + Duration::from_secs(5);
            } else {
                exit = true;
            }
        }
        if exit || (exit_requested && now >= exit_deadline) {
            break;
        }
        if !running {
            std::thread::sleep(Duration::from_millis(10));
            continue;
        }

        let frame = session.wait_frame()?;
        session.begin_frame()?;
        // The actions are read once a frame, before the head pose: the gaze goes out with it.
        let synced = attached.is_some_and(|set| session.sync(set).is_ok());
        if let Some(located) = session.locate_views(&local, frame.display_time)? {
            let positions = [located[0].pose.position.xyz(), located[1].pose.position.xyz()];
            let fov = [fov_angles(located[0].fov), fov_angles(located[1].fov)];
            let mut head = math::head_from_views(located[0].pose.orientation.xyzw(), positions, fov);
            let gaze_readable = |source: &gaze::Source<'_>| synced || matches!(source, gaze::Source::Tracker(_));
            if let Some(source) = gaze_source.as_ref().filter(|s| gaze_readable(s))
                && let Some(gaze) = source.read(&session, &view_space, frame.display_time, tick())
            {
                head.gaze = gaze;
                stats.gazes += 1;
                meter.gaze();
            }
            if poses.as_mut().is_some_and(|p| p.write_head(&head)) {
                stats.poses_written += 1;
                meter.head(true);
            }
            last_head = Some(head);
        }
        let mut aiming: [Option<(HandPose, HandPose)>; 2] = [None, None];
        let mut grip_poses = [HandPose::default(); 2];
        if let Some(poses_of) = &controllers
            && synced
        {
            // Aim poses, then grip poses, of both hands.
            let mut located = [[HandPose::default(); 2]; 2];
            for ((action, spaces), out) in poses_of.iter().zip(located.iter_mut()) {
                for (hand, (space, path)) in spaces.iter().zip(hand_paths).enumerate() {
                    let pose = session.pose_active(action, path).unwrap_or(false).then(|| session.locate(space, &local, frame.display_time).ok().flatten()).flatten();
                    if let Some(p) = pose {
                        out[hand] = HandPose { orientation: p.orientation.xyzw(), position: p.position.xyz(), valid: 1, received_tick: tick() };
                    }
                }
            }
            let [mut hands, mut grips] = located;
            for hand in 0..2 {
                (hands[hand], grips[hand]) = smoother.smooth(hand, frame.display_time, hands[hand], grips[hand]);
            }
            grip_poses = grips;
            debug_views.controllers(last_head.as_ref(), &hands, &grips);
            for (hand, seen) in hands_seen.iter_mut().enumerate() {
                let tracked = hands[hand].valid != 0;
                if tracked != *seen {
                    *seen = tracked;
                    println!("{} controller {}", ["Left", "Right"][hand], if tracked { "tracked" } else { "lost" });
                }
            }
            if hands_out.as_mut().is_some_and(|h| h.write(&hands, &grips)) {
                stats.hands_written += 1;
                meter.hands([hands[0].valid != 0, hands[1].valid != 0]);
            }
            if let (Some(trackers), Some(out)) = (&finger_trackers, fingers_out.as_mut()) {
                let mut fingers = [FingerPose::default(); 2];
                for (hand, tracker) in trackers.iter().enumerate() {
                    if let Ok(Some(joints)) = tracker.orientations(&local, frame.display_time) {
                        fingers[hand] = FingerPose { valid: 1, received_tick: tick(), fingers: monaka_core::fingers::bends(hand, &joints), ..FingerPose::default() };
                    }
                    let tracked = fingers[hand].valid != 0;
                    if tracked != fingers_seen[hand] {
                        fingers_seen[hand] = tracked;
                        println!("{} fingers {}", ["Left", "Right"][hand], if tracked { "tracked" } else { "lost" });
                    }
                }
                if out.write(&fingers) {
                    stats.fingers_written += 1;
                    meter.fingers([fingers[0].valid != 0, fingers[1].valid != 0]);
                }
            }
            if let (Some(out), Some(pad)) = (hands_out.as_mut(), &pad_actions) {
                let mut gamepad = pad.read(&session);
                if let Some(reach) = reach.as_mut() {
                    let head = last_head.as_ref().map(|h| (h.orientation, h.position));
                    let at = [0, 1].map(|h| [grips[h], hands[h]].into_iter().find(|p| p.valid != 0).map(|p| p.position));
                    buzz(&session, haptic.as_ref(), &hand_paths, reach.update(&mut gamepad, head, at, Instant::now()));
                }
                let head = last_head.as_ref().map(|h| (h.orientation, h.position));
                // Start held: the Monaka menu, unless the debug menu has the pad; what it changes
                // applies at once (the launcher saves it from the line it prints).
                let asked = monaka_menu.pad(&mut gamepad, head, Instant::now(), debug_views.menu_open());
                buzz(&session, haptic.as_ref(), &hand_paths, asked.buzzes);
                match asked.changed.and_then(|(id, value)| Some((live_setting(id)?, value))) {
                    Some((Live::Smoothing, value)) => smoother = smooth::Smoother::new(value.parse().unwrap_or(args.controller_smoothing)),
                    Some((Live::Reach, value)) => {
                        reach_pick = reach::parse(&value).unwrap_or(reach_pick);
                        reach = make_reach(reach_pick, reach_depth);
                    }
                    Some((Live::ReachDepth, value)) => {
                        reach_depth = value.parse::<f32>().map_or(reach_depth, |cm| cm / 100.0);
                        reach = make_reach(reach_pick, reach_depth);
                    }
                    Some((Live::Proxy, value)) => pad.set_proxy(pad::Proxy::parse(&value).unwrap_or_default()),
                    None => {}
                }
                // Select held: the debug menu (its Save capture: one with the debug drawing on it).
                // Only with the debug menu on (`--debug-menu`): otherwise Select reaches the game.
                let (buzzes, capture_drawn) = if args.debug_menu { debug_views.pad(&mut gamepad, head, Instant::now()) } else { (Vec::new(), false) };
                // A console command picked on the console panel.
                if let Some(monaka_overlay::panels::Asked::Console(command)) = debug_views.take_asked() {
                    console_feed.ask(&command);
                }
                buzz(&session, haptic.as_ref(), &hand_paths, buzzes);
                if capture_drawn {
                    capture::start(capture.as_mut());
                }
                if chord.step(&mut gamepad, Instant::now()) && capture::start(capture.as_mut()) {
                    buzz(&session, haptic.as_ref(), &hand_paths, [LEFT_HAND, RIGHT_HAND].map(|hand| (hand, 0.6, 0.15)));
                }
                out.write_pad(&PadState { gamepad, valid: 1, received_tick: tick() });
            }
            // The palm: the grip pose's position, the aim pose's direction.
            aiming = [0, 1].map(|hand| Some((hands[hand], grips[hand])).filter(|(aim, _)| args.reticles[hand] && aim.valid != 0));
        }

        if !frame.should_render {
            last_pair_at = tick(); // headset idle: pairs are not being read
        }
        if tick().saturating_sub(last_pair_at) > PRODUCER_SILENCE_MS && !console::stop_requested() {
            println!("No stereo pairs for 15 seconds; stopping.");
            console::request_stop();
        }
        let mut layers = Vec::new();
        if frame.should_render {
            // A frozen frame stays: no pairs are read, and the silence check is told they need not be.
            let read = if debug_views.frozen() {
                last_pair_at = tick();
                None
            } else if depth_layer || debug_views.wants_depth() {
                input.read_with_depth(&context, eyes, seen)?
            } else {
                input.read(&context, eyes, seen)?.map(|pair| (pair, None))
            };
            let arrived = read.is_some();
            let now = Instant::now();
            meter.frame(now, read.as_ref().map(|(pair, _)| (pair.sequence, pair.tick)), tick());
            if let Some(keys) = read.as_ref().and_then(|(pair, _)| debug_views.eye_keys(pair.sequence)) {
                meter.eyes(keys);
            }
            if let Some((pair, depth)) = read {
                seen = pair.sequence;
                stats.pairs += 1;
                last_pair_at = tick();
                rendered = poses.as_mut().and_then(|p| p.read_pair(seen)).filter(|p| p.eyes.iter().all(|e| e.valid != 0));
                if let Some(capture) = capture.as_mut().filter(|_| !debug_views.captures_drawn()) {
                    capture.arrived(&device, &context, eyes, seen, rendered.as_ref());
                }
                pair_depth = depth;
                if let Some(depth) = pair_depth.as_ref().filter(|_| depth_layer)
                    && !depth_chains.as_ref().is_some_and(|chains| chains.fits(depth))
                {
                    depth_chains = Some(depth::DepthChains::new(&session, &offered_formats, depth));
                }
            }
            hand_panels.update(&session, &device, &context);
            let game = debug_views.game();
            let content = monaka_overlay::panels::Content { stats: meter.view(now, game), console: console_feed.view(now), fake_vr: &monaka_overlay::fake_vr::FakeVr::HEADSET };
            debug_views.show_in_headset(&session, &device, &context, &content, now);
            monaka_menu.show(&session, &device, &context);
            for (eye, (swapchain, source)) in swapchains.iter().zip(&sources).enumerate() {
                swapchain.fill(|image| {
                    d3d::copy_texture(&context, image, source);
                    debug_views.draw_eye(&device, &context, image, eye, rendered.as_ref(), seen, eyes, pair_depth.as_ref().zip(input.depth_textures()));
                    d3d::flush(&context);
                })?;
            }
            // Captures that include the debug drawings take the eyes as drawn.
            if arrived && let (Some(capture), Some(drawn)) = (capture.as_mut(), debug_views.drawn_eyes()) {
                capture.arrived(&device, &context, drawn, seen, rendered.as_ref());
            }
            if let Some(pair) = &rendered {
                screen = None;
                // Each eye's depth, when the pair came with it and it has swapchains.
                let depth_views = match (&pair_depth, &depth_chains, input.depth_textures()) {
                    (Some(depth), Some(chains), Some(sources)) => chains.fill(&context, sources, depth),
                    _ => None,
                };
                let mut depth_views = depth_views.map(|[left, right]| [Some(left), Some(right)]).unwrap_or([None, None]);
                if depth_views[0].is_some() {
                    stats.with_depth += 1;
                }
                let mut view = |eye: usize| {
                    let e = &pair.eyes[eye];
                    let [left, right, up, down] = e.fov[eye];
                    ProjectionView {
                        pose: Posef::new(e.orientation, math::eye_position(e, eye)),
                        fov: Fovf { angleLeft: left, angleRight: right, angleUp: up, angleDown: down },
                        swapchain: &swapchains[eye],
                        depth: depth_views[eye].take(),
                    }
                };
                layers.push(Layer::Projection { space: &local, views: [view(0), view(1)] });
                stats.projected += 1;
                // In play, where the game's aim ray goes: from the head along the controller, or
                // where a weapon in the palm points.
                for (side, (reticle, aimed)) in reticles.iter().zip(aiming).enumerate() {
                    let (Some(swapchain), Some((hand, grip)), Some(head)) = (reticle, aimed, &last_head) else { continue };
                    let palm = if grip.valid != 0 { grip.position } else { hand.position };
                    let said = reticle_aims.as_mut().and_then(|r| r.read(side, RETICLE_SAID_MS));
                    let position = math::reticle_point(head.position, hand.orientation, palm, args.reticle_palm, said);
                    layers.push(Layer::Overlay {
                        space: &local,
                        swapchain,
                        pose: Posef::new(hand.orientation, position),
                        size: Extent2Df { width: RETICLE_SIZE, height: RETICLE_SIZE },
                    });
                }
                if controllers.is_some() {
                    layers.extend(hand_panels.layers(&local, &grip_poses, last_head.as_ref(), args.panel_nearest));
                }
            } else {
                let (space, quad_pose, width) = match &last_head {
                    Some(head) => {
                        let anchored = *screen.get_or_insert_with(|| {
                            let (orientation, position) = math::ahead_of(head.orientation, head.position, SCREEN_DISTANCE);
                            Posef::new(orientation, position)
                        });
                        (&local, anchored, args.screen_width)
                    }
                    None => (&view_space, Posef::new(math::IDENTITY, [0.0, 0.0, -SCREEN_DISTANCE]), 4.0),
                };
                for (eye, swapchain) in swapchains.iter().enumerate() {
                    let size = Extent2Df { width, height: width * image_aspect };
                    layers.push(Layer::Quad { space, eye, swapchain, pose: quad_pose, size });
                }
            }
            stats.visible += 1;
        }
        layers.extend(debug_views.layers(&local));
        layers.extend(monaka_menu.layer(&local));
        session.end_frame(frame.display_time, &layers)?;
        stats.frames += 1;
        // A capture's images read back after the frame is out, one per frame.
        if let Some(capture) = capture.as_mut() {
            capture.step(&device, &context);
        }
    }

    println!(
        "Frame cycles: {}; stereo image submissions: {}; pairs received: {}; head poses sent: {} ({} with eye gaze); hand records sent: {}; finger records sent: {}; projection-layer frames: {} ({} with depth)",
        stats.frames, stats.visible, stats.pairs, stats.poses_written, stats.gazes, stats.hands_written, stats.fingers_written, stats.projected, stats.with_depth
    );
    Ok(if stats.visible > 0 { 0 } else { 2 })
}

fn main() {
    let code = match parse_args() {
        Err(message) => {
            eprintln!("{message}");
            1
        }
        Ok(args) => run(args).unwrap_or_else(|error| {
            eprintln!("{error}");
            1
        }),
    };
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;
    use monaka_core::protocol::Luid;

    #[test]
    fn every_live_option_has_its_part_in_the_viewer() {
        for opt in monaka_core::manifest::LIVE {
            assert!(live_setting(opt.id).is_some(), "{} changes nothing in the viewer", opt.id);
        }
        assert_eq!(live_setting("no_such_option"), None);
    }

    fn producer(format: u32) -> Producer {
        Producer { width: 1, height: 1, adapter: Luid::default(), format }
    }

    #[test]
    fn reticle_is_a_premultiplied_ring() {
        let image = reticle_image(64);
        assert_eq!(image.len(), 64 * 64 * 4);
        let at = |x: usize, y: usize| &image[(y * 64 + x) * 4..(y * 64 + x) * 4 + 4];
        assert!(image.chunks(4).all(|p| p[0] <= p[3]), "colour never exceeds alpha");
        assert_eq!(at(0, 0), [0, 0, 0, 0], "transparent corner");
        assert_eq!(at(32, 32)[3], 255, "centre dot");
        assert_eq!(at(32 + 24, 32)[0], 255, "white ring");
        assert_eq!(at(32 + 12, 32)[3], 0, "clear between dot and ring");
    }

    #[test]
    fn swapchain_format_follows_the_game() {
        use dxgi_format::*;
        let offered = [R8G8B8A8_UNORM_SRGB as i64, B8G8R8A8_UNORM_SRGB as i64, R10G10B10A2_UNORM as i64];
        assert_eq!(swapchain_format(&offered, &producer(R8G8B8A8_UNORM)), Some(R8G8B8A8_UNORM_SRGB));
        assert_eq!(swapchain_format(&offered, &producer(B8G8R8A8_UNORM)), Some(B8G8R8A8_UNORM_SRGB));
        assert_eq!(swapchain_format(&offered, &producer(R10G10B10A2_UNORM)), Some(R10G10B10A2_UNORM));
        assert_eq!(swapchain_format(&[R8G8B8A8_UNORM as i64], &producer(R10G10B10A2_UNORM)), None);
    }
}
