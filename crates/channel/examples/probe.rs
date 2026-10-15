//! Reads pairs from a live stereo channel and reports what arrived; writes a head pose back.
//!
//! `cargo run -p monaka_channel --example probe -- <channel name> [seconds] [--save <folder> <pairs>] [--hand <yaw>] [--verify-moving]`
//!
//! Works on either transport (keyed mutex or fence). Only one reader may hold a channel, so run it
//! instead of the viewer, not beside it. The head pose it sends has a lopsided field of view per eye
//! (like a real headset), so pairs whose left eye was made from the right eye's view can be told
//! apart in their pose records. `--save` keeps that many consecutive pairs (copied on the GPU as they
//! arrive, so none are skipped) and writes them as half-size PNG files with their pose records.

use monaka_channel::d3d::{copy_texture, create_device, create_texture, find_adapter, read_image, read_pixel, simple_texture_desc};
use monaka_channel::hands::HandChannel;
use monaka_channel::pose::PoseChannel;
use monaka_channel::reader::{self, FenceReader};
use monaka_channel::{ChannelName, tick};
use monaka_core::bmp;
use monaka_contract::protocol::{HandPose, HeadPose, RenderedPair};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;

/// Left, right, up, down half-angles in radians; each eye reaches further outward than inward.
const LEFT_FOV: [f32; 4] = [-0.90, 0.75, 0.85, -0.90];
const RIGHT_FOV: [f32; 4] = [-0.75, 0.90, 0.85, -0.90];

/// Half-size PNG from tightly packed texels in the channel's actual DXGI format.
fn write_png(path: &Path, width: u32, height: u32, format: DXGI_FORMAT, rgba: &[u8]) -> std::io::Result<()> {
    let (w, h) = (width / 2, height / 2);
    let pixels: Vec<u8> = (0..h)
        .flat_map(|y| (0..w).map(move |x| ((y * 2 * width + x * 2) * 4) as usize))
        .flat_map(|p| {
            let [r, g, b, _] = bmp::dxgi_rgba8(format.0 as u32, rgba[p..p + 4].try_into().unwrap()).expect("supported channel format");
            [r, g, b]
        })
        .collect();
    std::fs::write(path, monaka_core::png::rgb(w, h, &pixels))
}

fn describe_pose(pair: Option<RenderedPair>) -> String {
    match pair {
        None => "no pose record (flat)".into(),
        Some(pair) => {
            let left = pair.eyes[0].fov[0];
            // The DL2 producer marks interpolated pairs with a gaze sample time of -1 (gaze is otherwise unused).
            let kind = if pair.eyes[0].gaze.sample_time == -1 {
                "interpolated (left eye made from the right eye's view)"
            } else if left == pair.eyes[1].fov[1] {
                "left eye made from the right eye's view"
            } else {
                "rendered stereo"
            };
            format!("{kind}; left eye fov {left:.2?}; head y {:.3}", pair.eyes[0].position[1])
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let usage = "usage: probe <channel name> [seconds] [--save <folder> <pairs>] [--hand <yaw degrees left> [--hand-sweep] [--hand-swing]] [--pad-walk] [--pad-hold <button>]";
    let mut args = std::env::args().skip(1);
    let name = ChannelName::new(args.next().ok_or(usage)?).ok_or("bad channel name")?;
    let (mut seconds, mut save, mut hand_yaw): (u64, Option<(PathBuf, usize)>, Option<f32>) = (3, None, None);
    let mut verify_moving = false;
    let mut hand_sweep = false;
    // `--hand-swing`: the hand swung like a club every two seconds: turned from 60 degrees left of
    // `--hand` to 60 right in 0.15 s (a tip half a metre out goes 7 m/s), then back slowly.
    let mut hand_swing = false;
    // `--pad-walk`: the VR controllers' gamepad with the left stick held fully forward (the
    // controller-pad check: the character walks).
    let mut pad_walk = false;
    // `--pad-hold <none|a|b|x|y|start|back>`: that button pressed on the VR gamepad for half a
    // second every two seconds (`--pad-walk`: the stick forward for 1.5 s of every 2).
    let mut pad_hold: Option<u16> = None;
    while let Some(arg) = args.next() {
        if arg == "--save" {
            let folder = PathBuf::from(args.next().ok_or(usage)?);
            let count: usize = args.next().ok_or(usage)?.parse()?;
            save = Some((folder, count.clamp(1, 32)));
        } else if arg == "--verify-moving" {
            verify_moving = true;
        } else if arg == "--hand" {
            hand_yaw = Some(args.next().ok_or(usage)?.parse()?);
        } else if arg == "--hand-sweep" {
            hand_sweep = true;
        } else if arg == "--hand-swing" {
            hand_swing = true;
        } else if arg == "--pad-walk" {
            pad_walk = true;
        } else if arg == "--pad-hold" {
            use monaka_contract::gamepad::button;
            pad_hold = Some(match args.next().ok_or(usage)?.as_str() {
                "none" => 0,
                "a" => button::A,
                "b" => button::B,
                "x" => button::X,
                "y" => button::Y,
                "start" => button::START,
                "back" => button::BACK,
                _ => return Err("--pad-hold takes none, a, b, x, y, start or back".into()),
            });
        } else {
            seconds = arg.parse()?;
        }
    }

    // A producer creates its channel with its first frame; wait up to ten seconds for it.
    let producer = reader::wait_for(&name, Duration::from_secs(10)).ok_or("no producer on that channel")?.map_err(|mismatch| mismatch.to_string())?;
    let (width, height, adapter, format) = (producer.width, producer.height, producer.adapter, DXGI_FORMAT(producer.format as i32));
    println!("channel {width}x{height} format {} adapter {}:{}", format.0, adapter.high, adapter.low);

    let (device, context, _) = create_device(Some(&find_adapter(adapter)?))?;
    let mut reader = FenceReader::open(&name, &device)?;
    let mut poses = PoseChannel::open(&name)?;
    let head = HeadPose { valid: 1, ipd: 0.064, position: [0.0, 1.7, 0.0], fov: [LEFT_FOV, RIGHT_FOV], ..HeadPose::default() };
    // `--hand`: a right controller held out in front (0.45 m ahead, 0.2 m below the head, in view
    // when the arms follow it) and turned that many degrees left, for hand-aim checks; level, or
    // with `--hand-sweep` pitching between 60 degrees down and up every 8 seconds, slowly (the
    // camera's position against its pitch). Its grip pose sits in the palm, 8 cm behind and 3 cm
    // below the aim pose, turned the same. A left controller mirrors it across the head.
    let mut hands = (hand_yaw.is_some() || pad_walk || pad_hold.is_some() || hand_swing).then(|| HandChannel::open(&name)).transpose()?;
    let started = Instant::now();
    let hand = |yaw: f32| {
        let pitch = if hand_sweep { 60f32.to_radians() * (started.elapsed().as_secs_f32() * std::f32::consts::TAU / 8.0).sin() } else { 0.0 };
        let (sy, cy) = (yaw.to_radians() * 0.5).sin_cos();
        let (sp, cp) = (pitch * 0.5).sin_cos();
        // Yaw about +Y, then pitch about the turned +X.
        let orientation = [cy * sp, sy * cp, -sy * sp, cy * cp];
        let aim = HandPose { orientation, position: [0.15, 1.5, -0.45], valid: 1, received_tick: tick() };
        let palm = monaka_core::math::rotate(orientation, [0.0, -0.03, 0.08]);
        let grip = HandPose { position: [0, 1, 2].map(|k| aim.position[k] + palm[k]), ..aim };
        // The left controller mirrors it across the head (same direction, 0.15 m left).
        let mirror = |p: HandPose| HandPose { position: [-p.position[0], p.position[1], p.position[2]], ..p };
        ([mirror(aim), aim], [mirror(grip), grip])
    };

    let texture = || create_texture(&device, &simple_texture_desc(width, height, format));
    let targets: [ID3D11Texture2D; 2] = [texture()?, texture()?];
    let targets = [&targets[0], &targets[1]];
    let mut kept: Vec<(u64, [ID3D11Texture2D; 2], Option<RenderedPair>)> = Vec::new();
    let save_after = Instant::now() + Duration::from_secs(2);
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let (mut seen, mut pairs, mut first) = (0u64, 0u64, None);
    let mut latest_tick = 0;
    let (mut pose_records, mut missing_poses, mut unequal_eyes, mut changes) = (0u64,0u64,0u64,0u64);
    let mut previous_orientation = None;
    let mut yaw_component_range = [f32::INFINITY, f32::NEG_INFINITY];
    while Instant::now() < deadline {
        poses.write_head(&head);
        // `--hand-swing` keeps an idle pad live, as the viewer does: the game stops reading a pad it
        // sees unplugged, and the swings' attacks go through the pad.
        if let (Some(channel), true) = (hands.as_mut(), pad_walk || pad_hold.is_some() || hand_swing) {
            // Pulsed, not held: games ignore input already held when they get focus.
            let phase = started.elapsed().as_millis() % 2000;
            let gamepad = monaka_contract::gamepad::Gamepad {
                buttons: if phase < 500 { pad_hold.unwrap_or(0) } else { 0 },
                thumb_ly: if pad_walk && phase < 1500 { i16::MAX } else { 0 },
                ..Default::default()
            };
            channel.write_pad(&monaka_contract::protocol::PadState { gamepad, valid: 1, received_tick: tick() });
        }
        if let (Some(channel), Some(yaw)) = (hands.as_mut(), hand_yaw) {
            let swung = if hand_swing {
                let phase = (started.elapsed().as_millis() % 2000) as f32;
                match phase {
                    p if p < 150.0 => 60.0 - 120.0 * p / 150.0,
                    p if p < 1650.0 => -60.0 + 120.0 * (p - 150.0) / 1500.0,
                    _ => 60.0,
                }
            } else {
                0.0
            };
            let (aims, grips) = hand(yaw + swung);
            channel.write(&aims, &grips);
        }
        if let Some(pair) = reader.read(&context, targets, seen)? {
            seen = pair.sequence;
            latest_tick = pair.tick;
            pairs += 1;
            if verify_moving {
                if let Some(rendered) = poses.read_pair(pair.sequence) {
                    pose_records += 1;
                    if rendered.eyes[0] != rendered.eyes[1] { unequal_eyes += 1; }
                    let orientation = rendered.eyes[0].orientation;
                    if previous_orientation.is_some_and(|previous| previous != orientation) { changes += 1; }
                    previous_orientation = Some(orientation);
                    yaw_component_range[0] = yaw_component_range[0].min(orientation[1]);
                    yaw_component_range[1] = yaw_component_range[1].max(orientation[1]);
                } else { missing_poses += 1; }
            }
            first.get_or_insert(pair);
            if let Some((_, count)) = &save
                && kept.len() < *count
                && Instant::now() >= save_after
            {
                let copies = [texture()?, texture()?];
                copy_texture(&context, &copies[0], targets[0]);
                copy_texture(&context, &copies[1], targets[1]);
                kept.push((pair.sequence, copies, poses.read_pair(pair.sequence)));
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let Some(first) = first else { return Err("no pair arrived".into()) };
    println!("pairs read {pairs}; first sequence {}; last sequence {seen} ({} ms old)", first.sequence, tick().saturating_sub(latest_tick));
    println!("empty reads: {} with the header locked by the producer, {} with no newer finished pair", reader.busy, reader.idle);
    for (eye, texture) in targets.iter().enumerate() {
        let pixel = read_pixel(&device, &context, texture, width / 2, height / 2)?;
        println!("eye {eye} centre pixel bytes {pixel:02x?}");
    }
    println!("pose record for {seen}: {}", describe_pose(poses.read_pair(seen)));
    if let Some((folder, _)) = save {
        std::fs::create_dir_all(&folder)?;
        for (index, (sequence, eyes, pose)) in kept.iter().enumerate() {
            for (eye, side) in eyes.iter().zip(["left", "right"]) {
                let (w, h, rgba) = read_image(&device, &context, eye)?;
                write_png(&folder.join(format!("pair-{index:02}-{side}.png")), w, h, format, &rgba)?;
            }
            println!("saved pair {index:02} (sequence {sequence}): {}", describe_pose(*pose));
        }
    }
    if verify_moving {
        println!("moving pose check: {pose_records} records, {missing_poses} missing, {unequal_eyes} unequal eyes, {changes} orientation changes; quaternion y range {yaw_component_range:?}");
        if pose_records < 2 || missing_poses != 0 || unequal_eyes != 0 || changes < 2 { return Err("moving pose verification failed".into()); }
    }
    Ok(())
}
