//! Shows a game's stereo pairs in the headset and sends the head pose back to the game.
//!
//! Command line:
//!
//! ```text
//! monaka_viewer --gpu-map <channel> --loader <openxr_loader.dll> [--runtime <manifest.json>]
//!             [--seconds N] [--pixel-aspect A] [--screen-width M] [--reticle left|right]... [--reticle-from head|palm]
//!             [--dpad-hold left,right] [--menu-reach same|other|off] [--menu-reach-depth CM] [--act-like touch|index]
//!             [--stop-event <name>]
//! monaka_viewer --info --loader <openxr_loader.dll> [--runtime <manifest.json>]
//! ```
//!
//! Pairs that come with the head poses they were rendered from go out as a projection layer, so
//! the runtime reprojects them. Pairs without poses (menus, maps, loading screens) go on a flat
//! screen anchored in the room in front of where the head faced. Where each controller points
//! (its aim pose) and where it sits in the hand (its grip pose) go to the game through the hand
//! block, for hand aim and for putting the game's hand on the controller.
#![deny(unsafe_code)]

mod console;
mod pad;
mod panel;
mod reach;

use monaka_channel::d3d::{self, create_texture, find_adapter, simple_texture_desc};
use monaka_channel::fingers::FingerChannel;
use monaka_channel::hands::HandChannel;
use monaka_channel::pose::PoseChannel;
use monaka_channel::reader::{self, AnyReader};
use monaka_channel::{ChannelName, tick};
use monaka_core::math::{self, Quat, Vec3};
use monaka_core::protocol::{FingerPose, HandPose, HeadPose, LEFT_HAND, PadState, RIGHT_HAND, RenderedPair, dxgi_format};
use monaka_openxr::{Event, Extent2Df, Fovf, Instance, Layer, Loader, Posef, ProjectionView, Quaternionf, Vector3f, sys};
use std::error::Error;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use windows::Win32::Graphics::Direct3D11::D3D11_BIND_SHADER_RESOURCE;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;

/// Distance of the flat menu screen in front of the head, in metres.
const SCREEN_DISTANCE: f32 = 2.0;
/// A producer that publishes nothing for this long has stopped; the viewer ends.
const PRODUCER_SILENCE_MS: u64 = 15_000;

struct Args {
    channel: Option<ChannelName>,
    info: bool,
    seconds: u64,
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
    /// How far behind the head a hand reaches to open one (metres; `--menu-reach-depth` in cm).
    menu_reach_depth: f32,
    /// The controllers act like another kind (`--act-like touch|index`, a layout test).
    proxy: pad::Proxy,
}

fn parse_args() -> Result<Args, String> {
    let mut args =
        Args { channel: None, info: false, seconds: 30, pixel_aspect: 1.0, screen_width: 2.4, loader: PathBuf::new(), runtime: None, reticles: [false; 2], reticle_palm: false, panel_nearest: 0.0, dpad_hold: 0, menu_reach: Some(reach::Pick::Same), menu_reach_depth: reach::DEPTH, proxy: pad::Proxy::None };
    let mut words = std::env::args().skip(1);
    while let Some(flag) = words.next() {
        if flag == "--info" {
            args.info = true;
            continue;
        }
        let value = words.next().ok_or_else(|| format!("{flag} needs a value"))?;
        let number = |what: &str| value.parse::<f64>().map_err(|_| format!("{what} must be a number"));
        match flag.as_str() {
            "--gpu-map" => args.channel = Some(ChannelName::new(value.clone()).ok_or("bad channel name")?),
            "--seconds" => args.seconds = number("--seconds")? as u64,
            "--pixel-aspect" => args.pixel_aspect = number("--pixel-aspect")? as f32,
            "--screen-width" => args.screen_width = number("--screen-width")? as f32,
            // The hand panels sit no nearer the head than this (metres; 0: on the hand as it is).
            "--panel-nearest" => args.panel_nearest = number("--panel-nearest")? as f32,
            // Directions of a choice held open with the d-pad (`left,right`).
            "--dpad-hold" => args.dpad_hold = pad::dpad_directions(&value)?,
            // Those menus by reaching over a shoulder: picked with the same hand's stick, the
            // other's, or off.
            "--menu-reach" => args.menu_reach = reach::parse(&value)?,
            "--menu-reach-depth" => {
                let cm = number("--menu-reach-depth")?;
                if !(5.0..=40.0).contains(&cm) {
                    return Err("--menu-reach-depth is 5 to 40 (cm)".into());
                }
                args.menu_reach_depth = cm as f32 / 100.0;
            }
            // A layout test: the controllers act like Quest's or Index's.
            "--act-like" => args.proxy = pad::Proxy::parse(&value)?,
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
            // Eye tracking is not implemented; the flag is accepted and ignored for launch scripts
            // that pass it.
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
    if !args.loader.is_absolute() {
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

/// What the producer publishes, learned before anything is opened.
type Producer = reader::Description;

/// Swapchain images are filled by copy, so they must be in the producer's format family.
fn swapchain_format(offered: &[i64], producer: &Producer) -> Option<u32> {
    use dxgi_format::*;
    let has = |f: u32| offered.contains(&(f as i64));
    if producer.fence && producer.format != R8G8B8A8_UNORM {
        let srgb = if producer.format == B8G8R8A8_UNORM { B8G8R8A8_UNORM_SRGB } else { producer.format };
        return [producer.format, srgb].into_iter().find(|&f| has(f));
    }
    [R8G8B8A8_UNORM, R8G8B8A8_UNORM_SRGB].into_iter().find(|&f| has(f))
}

fn quat(q: Quaternionf) -> Quat {
    [q.x, q.y, q.z, q.w]
}

fn vec3(v: Vector3f) -> Vec3 {
    [v.x, v.y, v.z]
}

fn fov_angles(f: Fovf) -> [f32; 4] {
    [f.angleLeft, f.angleRight, f.angleUp, f.angleDown]
}

fn pose(orientation: Quat, position: Vec3) -> Posef {
    let [x, y, z, w] = orientation;
    let [px, py, pz] = position;
    Posef { orientation: Quaternionf { x, y, z, w }, position: Vector3f { x: px, y: py, z: pz } }
}

#[derive(Default)]
struct Stats {
    frames: u64,
    visible: u64,
    pairs: u64,
    poses_written: u64,
    hands_written: u64,
    fingers_written: u64,
    projected: u64,
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

/// The hand-aim reticle: this far along the aim ray, this wide (1.2 degrees).
const RETICLE_DISTANCE: f32 = 10.0;
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
    let producer = match &args.channel {
        // A producer may first switch the game to the headset's resolution.
        Some(name) => Some(reader::wait_for(name, Duration::from_secs(30)).ok_or("no GPU stereo producer became ready")?),
        None => None,
    };
    if let Some(p) = &producer {
        let transport = if p.fence { "D3D12 fence channel" } else { "D3D11 keyed channel" };
        println!("GPU stereo input: {}x{} format {} producer adapter={}:{} ({transport})", p.width, p.height, p.format, p.adapter.high, p.adapter.low);
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
    let (device, context, level) = d3d::create_device(Some(&find_adapter(runtime_adapter)?))?;
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
    let mut reach = args.menu_reach.map(|pick| reach::Reach::new(pick, args.dpad_hold, args.menu_reach_depth)).filter(|r| r.active() && pad_actions.is_some());
    if let (Some(pick), true) = (args.menu_reach, reach.is_some()) {
        println!("Menus by reach: rest a hand over its shoulder to open its menu; flick {} stick to pick", if pick == reach::Pick::Same { "that hand's" } else { "the other hand's" });
    }
    let haptic = actions.as_ref().and_then(|set| set.vibration_action("haptic", "Vibration", &hand_paths).map_err(|e| println!("No controller vibration: {e}")).ok());
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

    let session = instance.create_session(system, &device)?;
    let local = session.reference_space(sys::REFERENCE_SPACE_TYPE_LOCAL)?;
    let view_space = session.reference_space(sys::REFERENCE_SPACE_TYPE_VIEW)?;
    let controllers = match (&actions, &aim, &grip) {
        (Some(set), Some(aim), Some(grip)) => (|| -> monaka_openxr::Result<_> {
            session.attach(&[set])?;
            let spaces = |action| -> monaka_openxr::Result<_> { Ok([session.action_space(action, hand_paths[0])?, session.action_space(action, hand_paths[1])?]) };
            Ok((set, [(aim, spaces(aim)?), (grip, spaces(grip)?)]))
        })()
        .map_err(|e| println!("No controller input: {e}"))
        .ok(),
        _ => None,
    };
    let mut hands_out = controllers.as_ref().and_then(|_| HandChannel::open(name).ok());
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
    let mut fingers_out = finger_trackers.as_ref().and_then(|_| FingerChannel::open(name).ok());
    println!("{}", if fingers_out.is_some() { "Finger tracking on" } else { "No finger tracking (the headset or runtime offers none)" });
    let mut fingers_seen = [false; 2];
    let mut hands_seen = [false; 2];
    let mut poses = PoseChannel::open(name).ok();
    println!("Forward follows the headset: use its centre/recentre control.");

    let format = swapchain_format(&session.swapchain_formats()?, &producer).ok_or("runtime has no image format matching the game's")?;
    let swapchains = [
        session.create_swapchain(format as i64, producer.width, producer.height)?,
        session.create_swapchain(format as i64, producer.width, producer.height)?,
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
    // HUD pieces the game publishes on their own, on the right hand.
    let mut hand_panels = [
        panel::HandPanel::new(name, format, "minimap", RIGHT_HAND, panel::Side::BackOfHand, panel::HAND_WIDTH, false),
        panel::HandPanel::new(name, format, "quests", RIGHT_HAND, panel::Side::BackOfHand, panel::QUESTS_WIDTH, true),
        panel::HandPanel::new(name, format, "weapon", RIGHT_HAND, panel::Side::Palm, panel::HAND_WIDTH, false),
        panel::HandPanel::new(name, format, "health", LEFT_HAND, panel::Side::BackOfHand, panel::HAND_WIDTH, false),
        panel::HandPanel::new(name, format, "tool", LEFT_HAND, panel::Side::Palm, panel::ICON_WIDTH, false),
    ];
    let source_format = if producer.fence { producer.format } else { format };
    let mut source_desc = simple_texture_desc(producer.width, producer.height, DXGI_FORMAT(source_format as i32));
    source_desc.BindFlags = D3D11_BIND_SHADER_RESOURCE.0 as u32;
    let sources = [create_texture(&device, &source_desc)?, create_texture(&device, &source_desc)?];
    let eyes = [&sources[0], &sources[1]];

    let open_by = Instant::now() + Duration::from_secs(5);
    let mut input = loop {
        match AnyReader::open(name, &device, producer.fence) {
            Ok(input) => break input,
            Err(e) if Instant::now() >= open_by => return Err(format!("cannot open the GPU channel on the runtime adapter: {e}").into()),
            Err(_) => std::thread::sleep(Duration::from_millis(3)),
        }
    };
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
    let (mut running, mut exit, mut exit_requested) = (false, false, false);
    let deadline = Instant::now() + Duration::from_secs(args.seconds);
    let mut exit_deadline = deadline + Duration::from_secs(5);
    let mut last_pair_at = tick();
    let mut rendered: Option<RenderedPair> = None;
    let mut last_head: Option<HeadPose> = None;
    let mut screen: Option<Posef> = None;
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
        if let Some(located) = session.locate_views(&local, frame.display_time)? {
            let positions = [vec3(located[0].pose.position), vec3(located[1].pose.position)];
            let fov = [fov_angles(located[0].fov), fov_angles(located[1].fov)];
            let head = math::head_from_views(quat(located[0].pose.orientation), positions, fov);
            if poses.as_mut().is_some_and(|p| p.write_head(&head)) {
                stats.poses_written += 1;
            }
            last_head = Some(head);
        }
        let mut aiming: [Option<(HandPose, HandPose)>; 2] = [None, None];
        let mut grip_poses = [HandPose::default(); 2];
        if let Some((set, poses_of)) = &controllers
            && session.sync(set).is_ok()
        {
            // Aim poses, then grip poses, of both hands.
            let mut located = [[HandPose::default(); 2]; 2];
            for ((action, spaces), out) in poses_of.iter().zip(located.iter_mut()) {
                for (hand, (space, path)) in spaces.iter().zip(hand_paths).enumerate() {
                    let pose = session.pose_active(action, path).unwrap_or(false).then(|| session.locate(space, &local, frame.display_time).ok().flatten()).flatten();
                    if let Some(p) = pose {
                        out[hand] = HandPose { orientation: quat(p.orientation), position: vec3(p.position), valid: 1, received_tick: tick() };
                    }
                }
            }
            let [hands, grips] = located;
            grip_poses = grips;
            for (hand, seen) in hands_seen.iter_mut().enumerate() {
                let tracked = hands[hand].valid != 0;
                if tracked != *seen {
                    *seen = tracked;
                    println!("{} controller {}", ["Left", "Right"][hand], if tracked { "tracked" } else { "lost" });
                }
            }
            if hands_out.as_mut().is_some_and(|h| h.write(&hands, &grips)) {
                stats.hands_written += 1;
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
                }
            }
            if let (Some(out), Some(pad)) = (hands_out.as_mut(), &pad_actions) {
                let mut gamepad = pad.read(&session);
                if let Some(reach) = reach.as_mut() {
                    let head = last_head.as_ref().map(|h| (h.orientation, h.position));
                    let at = [0, 1].map(|h| [grips[h], hands[h]].into_iter().find(|p| p.valid != 0).map(|p| p.position));
                    for (hand, amplitude, seconds) in reach.update(&mut gamepad, head, at, Instant::now()) {
                        if let Some(haptic) = &haptic {
                            let _ = session.vibrate(haptic, hand_paths[hand], amplitude, seconds);
                        }
                    }
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
            if let Some(pair) = input.read(&context, eyes, seen)? {
                seen = pair.sequence;
                stats.pairs += 1;
                last_pair_at = tick();
                rendered = poses.as_mut().and_then(|p| p.read_pair(seen)).filter(|p| p.eyes.iter().all(|e| e.valid != 0));
            }
            for hand_panel in &mut hand_panels {
                hand_panel.update(&session, &device, &context);
            }
            for (swapchain, source) in swapchains.iter().zip(&sources) {
                swapchain.fill(|image| {
                    d3d::copy_texture(&context, image, source);
                    d3d::flush(&context);
                })?;
            }
            if let Some(pair) = &rendered {
                screen = None;
                let view = |eye: usize| {
                    let e = &pair.eyes[eye];
                    let [left, right, up, down] = e.fov[eye];
                    ProjectionView {
                        pose: pose(e.orientation, math::eye_position(e, eye)),
                        fov: Fovf { angleLeft: left, angleRight: right, angleUp: up, angleDown: down },
                        swapchain: &swapchains[eye],
                    }
                };
                layers.push(Layer::Projection { space: &local, views: [view(0), view(1)] });
                stats.projected += 1;
                // In play, where the game's aim ray goes: from the head along the controller, or
                // where a weapon in the palm points.
                for (reticle, aimed) in reticles.iter().zip(aiming) {
                    let (Some(swapchain), Some((hand, grip)), Some(head)) = (reticle, aimed, &last_head) else { continue };
                    let position = if args.reticle_palm {
                        math::converged(if grip.valid != 0 { grip.position } else { hand.position }, hand.orientation)
                    } else {
                        math::along(head.position, hand.orientation, RETICLE_DISTANCE)
                    };
                    layers.push(Layer::Overlay {
                        space: &local,
                        swapchain,
                        pose: pose(hand.orientation, position),
                        size: Extent2Df { width: RETICLE_SIZE, height: RETICLE_SIZE },
                    });
                }
                if controllers.is_some() {
                    let main_width = hand_panels[0].shown_size().0;
                    layers.extend(hand_panels.iter().filter_map(|p| p.layer(&local, &grip_poses, last_head.as_ref(), args.panel_nearest, main_width)));
                }
            } else {
                let (space, quad_pose, width) = match &last_head {
                    Some(head) => {
                        let anchored = *screen.get_or_insert_with(|| {
                            let (orientation, position) = math::ahead_of(head.orientation, head.position, SCREEN_DISTANCE);
                            pose(orientation, position)
                        });
                        (&local, anchored, args.screen_width)
                    }
                    None => (&view_space, pose(math::IDENTITY, [0.0, 0.0, -SCREEN_DISTANCE]), 4.0),
                };
                for (eye, swapchain) in swapchains.iter().enumerate() {
                    let size = Extent2Df { width, height: width * image_aspect };
                    layers.push(Layer::Quad { space, eye, swapchain, pose: quad_pose, size });
                }
            }
            stats.visible += 1;
        }
        session.end_frame(frame.display_time, &layers)?;
        stats.frames += 1;
    }

    println!(
        "Frame cycles: {}; stereo image submissions: {}; pairs received: {}; head poses sent: {}; hand records sent: {}; finger records sent: {}; projection-layer frames: {}",
        stats.frames, stats.visible, stats.pairs, stats.poses_written, stats.hands_written, stats.fingers_written, stats.projected
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

    fn producer(format: u32, fence: bool) -> Producer {
        Producer { width: 1, height: 1, adapter: Luid::default(), format, fence }
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
        assert_eq!(swapchain_format(&offered, &producer(R8G8B8A8_UNORM, false)), Some(R8G8B8A8_UNORM_SRGB));
        assert_eq!(swapchain_format(&offered, &producer(B8G8R8A8_UNORM, true)), Some(B8G8R8A8_UNORM_SRGB));
        assert_eq!(swapchain_format(&offered, &producer(R10G10B10A2_UNORM, true)), Some(R10G10B10A2_UNORM));
        assert_eq!(swapchain_format(&[R8G8B8A8_UNORM as i64], &producer(R10G10B10A2_UNORM, true)), None);
    }
}
