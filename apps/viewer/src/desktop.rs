//! The 2D window ([`monaka_contract::manifest::DESKTOP_ARG`]): the run without a headset. The game
//! renders as for the headset, from a still synthetic head; its left eye shows in a desktop
//! window with the debug views drawn on it, and the debug menu and its panels float over it as
//! windows used with the mouse ([`DebugViews::window_ui`]). The window never takes focus, so the
//! game keeps playing while it is clicked.

use crate::capture::Capture;
use crate::debug_views::DebugViews;
use crate::console_feed::ConsoleFeed;
use crate::view_commands;
use crate::{PRODUCER_SILENCE_MS, console, meter};
use monaka_channel::d3d::{self, create_texture, find_adapter, simple_texture_desc};
use monaka_channel::pose::PoseChannel;
use monaka_channel::{ChannelName, tick};
use monaka_contract::protocol::{HeadPose, RenderedPair};
use monaka_overlay::blit::{self, BlitPass};
use monaka_channel::console::ConsoleChannel;
use monaka_channel::hands::HandChannel;
use monaka_contract::protocol::HandPose;
use monaka_overlay::fake_vr::FakeVr;
use monaka_overlay::panels::{Asked, Content};
use monaka_overlay::window::Window;
use std::error::Error;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use windows::Win32::Graphics::Direct3D11::{D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;

/// The window's inside when it opens (at 96 dpi).
const SIZE: [u32; 2] = [1280, 800];
/// The synthetic head's eye distance (metres).
const IPD: f32 = 0.064;
/// The window's background around the picture.
const BACKGROUND: [f32; 4] = [0.05, 0.055, 0.07, 1.0];

/// Runs the 2D window on `name` until it is closed, the launcher stops it, `seconds` pass or the
/// game stops publishing; captures go into `captures`.
pub fn run(name: &ChannelName, seconds: u64, producer_wait: u64, captures: Option<PathBuf>) -> Result<i32, Box<dyn Error>> {
    let producer = crate::wait_for_producer(name, producer_wait)?;
    println!("GPU stereo input: {}x{} format {} (2D window)", producer.width, producer.height, producer.format);
    let adapter = find_adapter(producer.adapter)?;
    println!("GPU: {}", d3d::describe_adapter(&adapter));
    let (device, context, _) = d3d::create_device(Some(&adapter))?;
    let mut source_desc = simple_texture_desc(producer.width, producer.height, DXGI_FORMAT(producer.format as i32));
    source_desc.BindFlags = D3D11_BIND_SHADER_RESOURCE.0 as u32;
    let sources = [create_texture(&device, &source_desc)?, create_texture(&device, &source_desc)?];
    let eyes = [&sources[0], &sources[1]];
    // Each eye as shown: copied from its source, the debug views drawn over it.
    let mut drawn_desc = source_desc;
    drawn_desc.BindFlags = (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32;
    let drawn = [create_texture(&device, &drawn_desc)?, create_texture(&device, &drawn_desc)?];

    let mut poses = crate::pose_channel(PoseChannel::open(name))?;
    // Still, straight ahead, with a headset's field of view: the game renders as for a headset.
    // The fake head and controllers (the Fake VR panel): the head ahead, the hands held forward.
    let mut fake = FakeVr::desktop();
    let mut head = HeadPose::synthetic(fake.head[0], fake.head[1], HeadPose::SYNTHETIC_FOV, IPD);
    let mut hands = crate::side_channel("hand channel", HandChannel::open(name));
    if let Some(poses) = poses.as_mut() {
        poses.write_head(&head);
    }
    let mut debug_views = DebugViews::new(name, DXGI_FORMAT(producer.format as i32), captures.clone());
    let mut console_feed = ConsoleFeed::new(name);
    // The window's own command line (`monaka_play view`), and where its captures go.
    let mut commands = ConsoleChannel::open_view(name).ok();
    let shots = captures.clone().unwrap_or_else(std::env::temp_dir);
    let mut capture = captures.map(Capture::new);
    let mut blit = BlitPass::new(&device)?;
    let mut window = Window::open(&device, "Monaka 2D", SIZE)?;

    let mut input = crate::open_channel(name, &device)?;
    console::catch_ctrl_c();
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let mut meter = meter::Meter::new("Window", [producer.width, producer.height], Instant::now());
    let (mut seen, mut shown, mut frames) = (0u64, 0u64, 0u64);
    // Whether the loop ended on the window's closing (else a stop, the time, the game's silence).
    let mut closed = true;
    let mut last_pair_at = tick();
    let mut rendered: Option<RenderedPair> = None;
    println!("2D window open: the debug menu and its panels are in it. Close it to stop.");

    while window.pump() {
        let now = Instant::now();
        if console::stop_requested() || now >= deadline {
            closed = false;
            break;
        }
        if tick().saturating_sub(last_pair_at) > PRODUCER_SILENCE_MS {
            println!("No stereo pairs for 15 seconds; stopping.");
            closed = false;
            break;
        }
        if let Some(poses) = poses.as_mut()
            && poses.write_head(&head)
        {
            meter.head(false);
        }
        // A frozen frame stays: no pairs are read, and the silence check is told they need not be.
        let read = if debug_views.frozen() {
            last_pair_at = tick();
            None
        } else if debug_views.wants_depth() {
            input.read_with_depth(&context, eyes, seen)?
        } else {
            input.read(&context, eyes, seen)?.map(|pair| (pair, None))
        };
        meter.frame(now, read.as_ref().map(|(pair, _)| (pair.sequence, pair.tick)), tick());
        if let Some(keys) = read.as_ref().and_then(|(pair, _)| debug_views.eye_keys(pair.sequence)) {
            meter.eyes(keys);
        }
        let arrived = read.is_some();
        let mut pair_depth = None;
        if let Some((pair, depth)) = read {
            seen = pair.sequence;
            shown += 1;
            last_pair_at = tick();
            rendered = poses.as_mut().and_then(|p| p.read_pair(seen)).filter(|p| p.eyes.iter().all(|e| e.valid != 0));
            pair_depth = depth;
            if let Some(capture) = capture.as_mut().filter(|_| !debug_views.captures_drawn()) {
                capture.arrived(&device, &context, eyes, seen, rendered.as_ref());
            }
        }
        for (eye, image) in drawn.iter().enumerate() {
            d3d::copy_texture(&context, image, &sources[eye]);
            debug_views.draw_eye(&device, &context, image, eye, rendered.as_ref(), seen, eyes, pair_depth.as_ref().zip(input.depth_textures()));
        }
        if arrived && let (Some(capture), Some(drawn)) = (capture.as_mut(), debug_views.drawn_eyes()) {
            capture.arrived(&device, &context, drawn, seen, rendered.as_ref());
        }

        let (target, size) = window.target(&device)?;
        monaka_overlay::window::clear(&context, &target, BACKGROUND);
        blit.draw(&device, &context, &target, &drawn[0], blit::fitted([producer.width, producer.height], size))?;
        let game = debug_views.game();
        let content = Content { stats: meter.view(now, game), console: console_feed.view(now), fake_vr: &fake };
        let mut capture_asked = false;
        window.ui(&device, &context, &target, |ctx| capture_asked = debug_views.window_ui(ctx, &content, now))?;
        // A panel's ask: a command for the game's console, or the fake head and controllers moved.
        match debug_views.take_asked() {
            Some(Asked::Console(command)) => console_feed.ask(&command),
            Some(Asked::FakeVr(changed)) => {
                fake = changed;
                head = HeadPose::synthetic(fake.head[0], fake.head[1], HeadPose::SYNTHETIC_FOV, IPD);
            }
            None => {}
        }
        if let Some(hands) = hands.as_mut()
            && let Some(tracked) = write_controllers(hands, &fake)
        {
            meter.hands(tracked);
        }
        // A command for the window: done now, or a capture of this frame as drawn.
        if let Some(channel) = commands.as_mut()
            && let Some((number, command)) = channel.pending()
        {
            let before = fake;
            let answer = match view_commands::run(&command, &mut fake, &mut debug_views, now) {
                view_commands::Asked::Answer(text) => text,
                view_commands::Asked::Capture { window: whole } => {
                    let texture = if whole { d3d::view_texture(&target).ok() } else { Some(drawn[0].clone()) };
                    let name = format!("{}-{frames}.png", if whole { "window" } else { "eye" });
                    match texture.ok_or_else(|| "the window's picture is unreadable".to_owned()).and_then(|t| view_commands::save_png(&device, &context, &t, &shots, &name)) {
                        Ok(path) => path.display().to_string(),
                        Err(why) => format!("error: {why}"),
                    }
                }
            };
            if fake != before {
                head = HeadPose::synthetic(fake.head[0], fake.head[1], HeadPose::SYNTHETIC_FOV, IPD);
            }
            channel.answer(number, &answer);
        }
        if capture_asked {
            crate::capture::start(capture.as_mut());
        }
        window.present()?;
        frames += 1;
        if let Some(capture) = capture.as_mut() {
            capture.step(&device, &context);
        }
    }
    println!("2D window: {frames} frames shown, {shown} pairs received");
    if closed {
        println!("2D window closed: VR stops");
        return Ok(monaka_contract::manifest::CLOSED_BY_PLAYER);
    }
    Ok(if shown > 0 { 0 } else { 2 })
}

/// The fake controllers into the run's hand channel, each frame (none while the hands are off): a
/// controller's grip a little below and behind its aim, as on a real one. Which sides were sent
/// as located, once written.
fn write_controllers(hands: &mut HandChannel, fake: &FakeVr) -> Option<[bool; 2]> {
    let located = fake.controllers().map(|controller| {
        controller.map(|(position, orientation)| {
            let aim = HandPose { orientation, position, valid: 1, received_tick: tick() };
            let palm = monaka_core::math::rotate(orientation, [0.0, -0.03, 0.08]);
            (aim, HandPose { position: [0, 1, 2].map(|k| position[k] + palm[k]), ..aim })
        })
    });
    let written = hands.write(&located.map(|h| h.map_or(HandPose::default(), |h| h.0)), &located.map(|h| h.map_or(HandPose::default(), |h| h.1)));
    written.then(|| located.map(|h| h.is_some()))
}
