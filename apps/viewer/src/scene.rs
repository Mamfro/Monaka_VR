//! The background in the headset while no game shows (`background`), the eye images under the
//! home's panels in place of the plain dark: the captured room ([`monaka_overlay::scene`]), or a
//! made one ([`monaka_overlay::pattern`], Drift). Both are drawn the same way ([`Eyes`]), and the
//! player turns in both. The captured room is the first in the `scenes` folder beside the viewer; it is read on a
//! thread of its own the first time it shows, and each capture's colours as the clock comes to it,
//! so the headset never waits on the files. Its light follows the clock (each capture faded into
//! the next as its time comes: [`Minutes`]), or stays at the hour the player set (`scene_time`); a
//! new one set fades in over [`CHANGE_SECONDS`].
//! With no menu open, the right stick turns the player in it, 45 degrees a flick ([`SnapStick`],
//! [`Placement`]); recentring puts the room's front ahead again. The triggers sweep the time of day
//! (LT back, RT on: [`Sweep`]), the captures blending into each other while held; let go, it
//! settles on the nearest capture.
//!
//! A room with `ambience.ogg` in its folder sounds too ([`Sound`]): its left channel from out past
//! the room's left wall, its right past the right, faded in and out with the room.

use crate::fov_angles;
use crate::home::HomeView;
use monaka_channel::d3d::{self, simple_texture_desc};
use monaka_contract::protocol::dxgi_format;
use monaka_core::math::{Quat, Vec3, add, multiply, rotate, sub, wrap_radians, yaw_rotation};
use monaka_sound::{Ambience, Clip};
use monaka_openxr::{Layer, ProjectionView, Session, Space, Swapchain, Time, sys};
use monaka_overlay::eye::EyeView;
use monaka_overlay::pattern::{FACE, Pattern, PatternPass, room};
use monaka_overlay::scene::files::Minutes;
use monaka_overlay::scene::{ScenePass, files, pack};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::Instant;
use windows::Win32::Graphics::Direct3D11::{D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;

/// The folder beside the viewer that scenes are in.
const SCENES: &str = "scenes";
/// How long a time the player sets takes to fade in (seconds).
const CHANGE_SECONDS: f32 = 0.5;
/// How far a flick of the right stick turns the player.
const SNAP: f32 = std::f32::consts::FRAC_PI_4;
/// The stick turns once pushed past [`FLICK`] sideways, and again only once back within [`REST`].
const FLICK: f32 = 0.75;
const REST: f32 = 0.35;
/// How fast a trigger pulled all the way sweeps the time (hours a second); a lighter pull is
/// slower still (as its square), for fine steps.
const SWEEP: f64 = 1.5;
/// A trigger counts once pulled past this much.
const TRIGGER_REST: f64 = 0.1;
/// Let go, the time comes to the nearest capture about this quickly (each second, this many times
/// closer).
const SETTLE: f64 = 6.0;

/// The room's sound, in its folder.
const AMBIENCE: &str = "ambience.ogg";
/// Each channel sounds this far past the wall the middle capture sees on its side (metres), and
/// the wall is taken as no further than [`FARTHEST`] (an open side: the sound stays near).
const BEYOND_WALL: f32 = 2.0;
const FARTHEST: f32 = 12.0;
/// How long the sound takes to fade in or out with the room (seconds).
const SOUND_FADE: f32 = 1.0;

/// The room as read on its own thread, with its sound decoded if it has one.
type Read = Result<(files::Loaded, Option<Result<Clip, String>>), String>;

/// A capture's colours, one image per capture point, as read on their own thread.
type TimeRead = Receiver<Result<Vec<Vec<u8>>, String>>;

enum State {
    /// Not read yet: it has not been asked to show.
    Waiting,
    Reading(Receiver<Read>),
    Ready(Box<Ready>),
    /// It could not be read or drawn (said in the log): the dark shows for the rest of the run.
    Failed,
}

struct Ready {
    pass: ScenePass,
    places: Vec<&'static str>,
    times: Vec<Minutes>,
    face: u32,
    /// A capture's colours being read, while one is.
    reading: Option<(Minutes, TimeRead)>,
    /// Captures that could not be read (said once): the one before stays instead.
    unreadable: Vec<Minutes>,
    shown: TimeShown,
    sound: Option<Sound>,
}

/// The room's sound playing: each of its channels from a place in the scene.
struct Sound {
    ambience: Ambience,
    /// Where each channel comes from (scene axes), and how far that is from the middle capture.
    places: [Vec3; 2],
    near: [f32; 2],
    /// Where each channel was last heard from (kept while the head is not known).
    heard: [monaka_sound::Hearing; 2],
    /// How loud it is now (0 to 1), fading with the room, and when it was last set.
    level: f32,
    at: Instant,
}

impl Sound {
    /// `clip` playing, its channels past the walls `sides` metres to the left and right.
    fn start(clip: Clip, sides: [f32; 2]) -> Result<Self, String> {
        let [left, right] = sides.map(|wall| wall.min(FARTHEST) + BEYOND_WALL);
        let ambience = Ambience::play(clip)?;
        println!("Home scene: {AMBIENCE} plays, {left:.1} m to the left and {right:.1} m to the right");
        Ok(Self { ambience, places: [[-left, 0.0, 0.0], [right, 0.0, 0.0]], near: [left, right], heard: [monaka_sound::Hearing { direction: [-1.0, 0.0, 0.0], gain: 1.0 }, monaka_sound::Hearing { direction: [1.0, 0.0, 0.0], gain: 1.0 }], level: 0.0, at: Instant::now() })
    }
}

/// Which capture shows, as the clock or the player moves it.
#[derive(Default)]
pub(crate) struct TimeShown {
    /// The capture shown last (the one mostly shown, while fading).
    shown: Option<Minutes>,
    /// A fade from one capture to another, and when it started.
    change: Option<(Minutes, Minutes, Instant)>,
}

/// The backgrounds there are to draw (`none`, the third choice, is the plain dark).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Background {
    /// The captured room.
    Home,
    /// Colours drifting round a dome.
    Drift,
}

pub struct HomeScene<'s> {
    /// The captured room's folder, if there is one.
    folder: Option<PathBuf>,
    eyes: Eyes<'s>,
    /// The captured room, as read.
    state: State,
    /// The made background, as made.
    made: Made,
    stick: SnapStick,
    sweep: Sweep,
}

/// The time of day as the triggers move it: how far the time shown is from what the clock (or the
/// setting) says.
#[derive(Default)]
pub(crate) struct Sweep {
    /// How far from the clock's or the setting's time (seconds).
    offset: f64,
    /// How fast the triggers move it now (hours a second; 0: let go).
    speed: f64,
    /// The setting the offset was made against: another (the player set the time, or the clock
    /// on or off) and the offset goes.
    against: Option<Option<u8>>,
    at: Option<Instant>,
}

impl Sweep {
    /// The triggers as the gamepad has them (0 to 255): LT back, RT on.
    pub(crate) fn triggers(&mut self, back: u8, on: u8) {
        let pull = |t: u8| ((f64::from(t) / 255.0 - TRIGGER_REST) / (1.0 - TRIGGER_REST)).clamp(0.0, 1.0);
        self.speed = (pull(on).powi(2) - pull(back).powi(2)) * SWEEP;
    }

    /// The time to show (seconds past midnight) at `now`, `base` the clock's or the setting's time
    /// and `setting` the hour set (none: the clock), the scene captured at `times`; and whether to
    /// blend the captures all the way (while swept, or settling onto one).
    pub(crate) fn time(&mut self, base: f64, setting: Option<u8>, times: &[Minutes], now: Instant) -> (f64, bool) {
        if self.against != Some(setting) {
            self.offset = 0.0;
            self.against = Some(setting);
        }
        let step = self.at.map_or(0.0, |at| (now - at).as_secs_f64().min(0.1));
        self.at = Some(now);
        if self.speed != 0.0 {
            self.offset = (self.offset + self.speed * 3600.0 * step).rem_euclid(86_400.0);
            return ((base + self.offset).rem_euclid(86_400.0), true);
        }
        if self.offset == 0.0 {
            return (base, false);
        }
        // Let go: onto the nearest capture, rather than two half-shown.
        let shown = base + self.offset;
        let gap = files::nearest_time(times, shown.rem_euclid(86_400.0)) - shown.rem_euclid(86_400.0);
        if gap.abs() < 1.0 {
            self.offset += gap;
            return ((base + self.offset).rem_euclid(86_400.0), false);
        }
        self.offset += gap * (step * SETTLE).min(1.0);
        ((base + self.offset).rem_euclid(86_400.0), true)
    }
}

/// The made background: its room being made on its own thread, or made.
enum Made {
    Waiting,
    Making(Receiver<files::Loaded>),
    Ready(Box<MadeReady>),
    /// It could not be made or drawn (said in the log): the dark shows instead.
    Failed,
}

struct MadeReady {
    pass: ScenePass,
    paint: PatternPass,
}

/// The eye images a background is drawn into, and where it stands round the player.
struct Eyes<'s> {
    format: DXGI_FORMAT,
    swapchains: [Swapchain<'s>; 2],
    /// Each eye drawn here, then copied into its swapchain's image (and the panels frosted over
    /// it, read from here).
    images: [ID3D11Texture2D; 2],
    /// The views drawn this frame, for its layer; none when it did not draw.
    drawn: Option<[sys::View; 2]>,
    /// Where the room stands round the player, as the stick turned them.
    placement: Placement,
}

impl Eyes<'_> {
    /// Draws `pass` (at `times`: the capture, the one it fades into, how far) for this frame's views
    /// of `space` at `time`, the home's panels frosted over it: whether it drew.
    #[allow(clippy::too_many_arguments)]
    fn draw(&mut self, pass: &mut ScenePass, times: (Minutes, Minutes, f32), session: &Session<'_>, device: &ID3D11Device, context: &ID3D11DeviceContext, space: &Space<'_>, time: Time, home: &mut HomeView<'_>) -> Result<bool, String> {
        let Ok(Some(views)) = session.locate_views(space, time) else { return Ok(false) };
        let (now, next, fade) = times;
        for (eye, view) in views.iter().enumerate() {
            let seen_as = EyeView { orientation: view.pose.orientation.xyzw(), position: view.pose.position.xyz(), fov: fov_angles(view.fov) };
            if !pass.draw(device, context, &self.images[eye], self.format, &self.placement.eye(seen_as), now, next, fade)? {
                return Ok(false);
            }
            let image = &self.images[eye];
            let format = self.format;
            self.swapchains[eye]
                .fill(|target| {
                    d3d::copy_texture(context, target, image);
                    home.frost_eye(device, context, target, format, image, &seen_as);
                })
                .map_err(|e| format!("its images could not be shown ({e})"))?;
        }
        self.drawn = Some(views);
        Ok(true)
    }

    /// This frame's eye images, when [`Eyes::draw`] drew them.
    fn layer<'a>(&'a self, space: &'a Space<'a>) -> Option<Layer<'a>> {
        let views = self.drawn.as_ref()?;
        let view = |eye: usize| ProjectionView { pose: views[eye].pose, fov: views[eye].fov, swapchain: &self.swapchains[eye], depth: None };
        Some(Layer::Projection { space, views: [view(0), view(1)] })
    }
}

/// Where the room stands round the player: a point `p` of the room the headset tracks in is the
/// scene's `yaw_rotation(yaw) · p + offset` (yaw in radians, left positive).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Placement {
    yaw: f32,
    offset: Vec3,
}

impl Placement {
    /// The player turned by `angle` (radians, right positive) where they stand, at `head`: the
    /// scene goes round the upright line through the head, so they stay where they are in it.
    pub(crate) fn turned(self, angle: f32, head: Vec3) -> Self {
        let at = [head[0], 0.0, head[2]];
        let shift = rotate(yaw_rotation(self.yaw), sub(at, rotate(yaw_rotation(-angle), at)));
        Placement { yaw: wrap_radians(self.yaw - angle), offset: add(self.offset, shift) }
    }

    /// The scene's point at room point `p`.
    pub(crate) fn scene_point(self, p: Vec3) -> Vec3 {
        add(rotate(yaw_rotation(self.yaw), p), self.offset)
    }

    /// `eye` as it stands in the scene.
    fn eye(self, eye: EyeView) -> EyeView {
        EyeView { orientation: multiply(yaw_rotation(self.yaw), eye.orientation), position: self.scene_point(eye.position), fov: eye.fov }
    }
}

/// The right stick as snap turns: a flick sideways turns once.
pub(crate) struct SnapStick {
    armed: bool,
}

impl Default for SnapStick {
    fn default() -> Self {
        Self { armed: true }
    }
}

impl SnapStick {
    /// The turn (radians, right positive) the stick at `x` (-1 left to 1 right) asks for now.
    pub(crate) fn flick(&mut self, x: f32) -> Option<f32> {
        if x.abs() < REST {
            self.armed = true;
        } else if self.armed && x.abs() > FLICK {
            self.armed = false;
            return Some(SNAP.copysign(x));
        }
        None
    }
}

impl<'s> HomeScene<'s> {
    /// The background drawn into eye images `eye_size` big: the captured room is the first in the
    /// `scenes` folder beside the viewer, if there is one. None when the runtime has no image
    /// format for it (said in the log).
    pub fn new(session: &'s Session<'s>, device: &ID3D11Device, offered: &[i64], eye_size: Option<(u32, u32)>) -> Option<Self> {
        let scenes = std::env::current_exe().ok()?.parent()?.join(SCENES);
        let folder = files::find(&scenes);
        if folder.is_none() {
            println!("Home scene: none in {}; the made backgrounds are there still", scenes.display());
        }
        let (width, height) = eye_size?;
        let Some(format) = [dxgi_format::R8G8B8A8_UNORM_SRGB, dxgi_format::B8G8R8A8_UNORM_SRGB].into_iter().find(|&f| offered.contains(&(f as i64))) else {
            println!("Home scene: the runtime offers no sRGB image format; the plain dark shows");
            return None;
        };
        let made = || -> Result<_, String> {
            let swapchains = [session.create_drawn_swapchain(format as i64, width, height), session.create_drawn_swapchain(format as i64, width, height)];
            let [Ok(left), Ok(right)] = swapchains else { return Err("its swapchains could not be made".into()) };
            let mut desc = simple_texture_desc(width, height, DXGI_FORMAT(format as i32));
            desc.BindFlags = (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32;
            let image = || d3d::create_texture(device, &desc).map_err(|e| e.to_string());
            Ok(([left, right], [image()?, image()?]))
        };
        match made() {
            Ok((swapchains, images)) => Some(Self {
                folder,
                eyes: Eyes { format: DXGI_FORMAT(format as i32), swapchains, images, drawn: None, placement: Placement::default() },
                state: State::Waiting,
                made: Made::Waiting,
                stick: SnapStick::default(),
                sweep: Sweep::default(),
            }),
            Err(e) => {
                println!("Home scene: {e}; the plain dark shows");
                None
            }
        }
    }

    /// Draws `background` for this frame's views of `space` at `time`, the captured room at `hour`
    /// (none: as the clock says), the home's panels frosted over it: true when it did. False while
    /// it is still being read or made (the dark shows meanwhile), or when it cannot be.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(&mut self, session: &Session<'_>, device: &ID3D11Device, context: &ID3D11DeviceContext, space: &Space<'_>, time: Time, background: Background, hour: Option<u8>, home: &mut HomeView<'_>) -> bool {
        self.eyes.drawn = None;
        match background {
            Background::Home => {
                let Some(folder) = self.folder.clone() else { return false };
                self.step(device, hour);
                let State::Ready(ready) = &mut self.state else { return false };
                let readable: Vec<Minutes> = ready.times.iter().copied().filter(|t| !ready.unreadable.contains(t)).collect();
                let (seconds, swept) = self.sweep.time(seconds_of_day(hour), hour, &readable, Instant::now());
                let (now, next, fade) = if swept { files::blended_times(&readable, seconds) } else { files::shown_times(&readable, seconds) };
                ready.read_soon(&folder, [now, next]);
                let on_gpu = |time: Minutes| ready.pass.has_time(time);
                let shown = if swept && on_gpu(now) && on_gpu(next) {
                    // Swept: the blend as it is, the capture mostly shown kept for when it settles.
                    ready.shown.hold(if fade >= 0.5 { next } else { now });
                    Some((now, next, fade))
                } else {
                    ready.shown.pick(on_gpu, now, next, fade, Instant::now())
                };
                let Some((now, next, fade)) = shown else { return false };
                match self.eyes.draw(&mut ready.pass, (now, next, fade), session, device, context, space, time, home) {
                    Ok(drawn) => drawn,
                    Err(e) => {
                        println!("Home scene: {e}; the plain dark shows");
                        self.state = State::Failed;
                        false
                    }
                }
            }
            Background::Drift => {
                self.make(device);
                let Made::Ready(made) = &mut self.made else { return false };
                made.paint.draw(context, Pattern::Drift, files::local_seconds());
                match self.eyes.draw(&mut made.pass, (0, 0, 0.0), session, device, context, space, time, home) {
                    Ok(drawn) => drawn,
                    Err(e) => {
                        println!("Drift background: {e}; the plain dark shows");
                        self.made = Made::Failed;
                        false
                    }
                }
            }
        }
    }

    /// Makes the made background the first time it is asked for (its room on a thread of its own,
    /// so the headset never waits), and takes it once made.
    fn make(&mut self, device: &ID3D11Device) {
        match &self.made {
            Made::Waiting => {
                let (send, made) = channel();
                std::thread::spawn(move || {
                    let _ = send.send(room(Pattern::Drift, FACE));
                });
                self.made = Made::Making(made);
            }
            Made::Making(made) => match made.try_recv() {
                Err(TryRecvError::Empty) => {}
                Ok(loaded) => {
                    let ready = ScenePass::new(device, &loaded).and_then(|mut pass| {
                        let paint = PatternPass::new(device, FACE)?;
                        pass.add_time_views(0, vec![paint.view()]);
                        Ok(MadeReady { pass, paint })
                    });
                    self.made = match ready {
                        Ok(ready) => {
                            println!("Background: Drift");
                            Made::Ready(Box::new(ready))
                        }
                        Err(e) => {
                            println!("Drift background: {e}; the plain dark shows");
                            Made::Failed
                        }
                    };
                }
                Err(TryRecvError::Disconnected) => self.made = Made::Failed,
            },
            Made::Ready(_) | Made::Failed => {}
        }
    }

    /// The right stick, `x` as the gamepad has it, with the head at `head`: a flick turns the
    /// player 45 degrees where they stand.
    pub fn turn(&mut self, x: i16, head: Option<Vec3>) {
        if let Some(angle) = self.stick.flick(f32::from(x) / 32767.0)
            && let Some(head) = head
        {
            self.eyes.placement = self.eyes.placement.turned(angle, head);
        }
    }

    /// The room's sound this frame: heard from the head (its `orientation` and position in the
    /// room the headset tracks), at `volume` (0 to 1) while the room `shown`, fading out while it
    /// is not.
    pub fn hear(&mut self, shown: bool, volume: f32, head: Option<(Quat, Vec3)>) {
        let placement = self.eyes.placement;
        let State::Ready(ready) = &mut self.state else { return };
        let Some(sound) = ready.sound.as_mut() else { return };
        let now = Instant::now();
        let step = (now - sound.at).as_secs_f32() / SOUND_FADE;
        sound.at = now;
        sound.level = if shown { (sound.level + step).min(1.0) } else { (sound.level - step).max(0.0) };
        let loud = sound.level * volume.clamp(0.0, 1.0);
        let Some((orientation, position)) = head else {
            sound.ambience.set(sound.heard, loud);
            return;
        };
        let (orientation, position) = (multiply(yaw_rotation(placement.yaw), orientation), placement.scene_point(position));
        // A mono recording is heard from both places alike (the mix reads its one channel twice).
        sound.heard = [0, 1].map(|c| monaka_sound::hearing(sound.places[c], orientation, position, sound.near[c]));
        sound.ambience.set(sound.heard, loud);
    }

    /// The triggers as the gamepad has them (0 to 255; both 0 while a menu has the pad): LT sweeps
    /// the time back, RT on.
    pub fn sweep(&mut self, back: u8, on: u8) {
        self.sweep.triggers(back, on);
    }

    /// The player recentred: the room's front is ahead again.
    pub fn recentred(&mut self) {
        self.eyes.placement = Placement::default();
    }

    /// This frame's eye images, when [`HomeScene::draw`] drew them.
    pub fn layer<'a>(&'a self, space: &'a Space<'a>) -> Option<Layer<'a>> {
        self.eyes.layer(space)
    }

    /// Starts reading the scene the first time it is asked for, and takes it once read.
    fn step(&mut self, device: &ID3D11Device, hour: Option<u8>) {
        match &self.state {
            State::Waiting => {
                let Some(folder) = self.folder.clone() else { return };
                let (send, read) = channel();
                std::thread::spawn(move || {
                    let wanted = |times: &[Minutes]| {
                        let (now, next, _) = files::shown_times(times, seconds_of_day(hour));
                        if now == next { vec![now] } else { vec![now, next] }
                    };
                    // A packed room carries its sound; a folder has it beside its captures.
                    let clip = if pack::is_pack(&folder) {
                        match pack::sound(&folder) {
                            Ok(bytes) => bytes.map(|bytes| monaka_sound::decode_bytes(&bytes, "the room's sound")),
                            Err(e) => Some(Err(e)),
                        }
                    } else {
                        let sound = folder.join(AMBIENCE);
                        sound.is_file().then(|| monaka_sound::decode(&sound))
                    };
                    let _ = send.send(files::load(&folder, wanted).map(|loaded| (loaded, clip)));
                });
                self.state = State::Reading(read);
            }
            State::Reading(read) => match read.try_recv() {
                Err(TryRecvError::Empty) => {}
                Ok(Ok((loaded, clip))) => match ScenePass::new(device, &loaded) {
                    Ok(pass) => {
                        let places: Vec<&'static str> = loaded.places.iter().map(|p| p.name).collect();
                        println!("Home scene: {} ({} capture points, {} times of day)", loaded.name, places.len(), loaded.times.len());
                        // A sound that cannot play leaves the room silent, not dark.
                        let sound = match clip.map(|clip| clip.and_then(|clip| Sound::start(clip, loaded.sides))) {
                            Some(Ok(sound)) => Some(sound),
                            Some(Err(e)) => {
                                println!("Home scene: {AMBIENCE}: {e}; silent");
                                None
                            }
                            None => None,
                        };
                        self.state = State::Ready(Box::new(Ready {
                            pass,
                            places,
                            times: loaded.times,
                            face: loaded.face,
                            reading: None,
                            unreadable: Vec::new(),
                            shown: TimeShown::default(),
                            sound,
                        }));
                    }
                    Err(e) => {
                        println!("Home scene: {e}; the plain dark shows");
                        self.state = State::Failed;
                    }
                },
                Ok(Err(e)) => {
                    println!("Home scene: {e}; the plain dark shows");
                    self.state = State::Failed;
                }
                Err(TryRecvError::Disconnected) => self.state = State::Failed,
            },
            State::Ready(_) | State::Failed => {}
        }
        if let State::Ready(ready) = &mut self.state {
            ready.take_time(device);
        }
    }
}

impl Ready {

    /// Starts reading the first of `times` not on the GPU, unless one is being read.
    fn read_soon(&mut self, folder: &std::path::Path, times: [Minutes; 2]) {
        let Some(time) = times.into_iter().find(|&t| !self.pass.has_time(t) && !self.unreadable.contains(&t)) else { return };
        if self.reading.is_some() {
            return;
        }
        let (send, read) = channel();
        let (folder, places, face) = (folder.to_path_buf(), self.places.clone(), self.face);
        std::thread::spawn(move || {
            let _ = send.send(files::load_time(&folder, &places, time, face));
        });
        self.reading = Some((time, read));
    }

    /// Puts a capture read meanwhile on the GPU.
    fn take_time(&mut self, device: &ID3D11Device) {
        let Some((time, read)) = &self.reading else { return };
        let taken = match read.try_recv() {
            Err(TryRecvError::Empty) => return,
            Ok(Ok(images)) => self.pass.add_time(device, *time, &images),
            Ok(Err(e)) => Err(e),
            Err(TryRecvError::Disconnected) => Err("its reader stopped".into()),
        };
        if let Err(e) = taken {
            println!("Home scene: {}: {e}; the capture before stays", files::clock_text(*time));
            self.unreadable.push(*time);
        }
        self.reading = None;
    }
}

impl TimeShown {
    /// `time` is the capture shown now, outright (the player sweeping the time shows the blend
    /// itself): no fade from before.
    pub(crate) fn hold(&mut self, time: Minutes) {
        self.shown = Some(time);
        self.change = None;
    }

    /// What to draw, given the capture `now` (as the clock or the player has it) fading `fade` into
    /// `next`: the capture, the one it fades into and how far. While `now` is still being read the
    /// capture shown stays; a new time the player set fades in from it.
    pub(crate) fn pick(&mut self, has: impl Fn(Minutes) -> bool, now: Minutes, next: Minutes, fade: f32, at: Instant) -> Option<(Minutes, Minutes, f32)> {
        if !has(now) {
            return self.shown.filter(|&h| has(h)).map(|h| (h, h, 0.0));
        }
        // A fade into `now` under way.
        if let Some((from, to, started)) = self.change
            && to == now
            && has(from)
        {
            let far = (at - started).as_secs_f32() / CHANGE_SECONDS;
            if far < 1.0 {
                self.shown = Some(if far >= 0.5 { now } else { from });
                return Some((from, now, far));
            }
        }
        self.change = None;
        // Another capture now than the one shown (set by the player, or the clock taken up again):
        // it fades in from the one shown (mostly shown, if set again mid-fade).
        if let Some(from) = self.shown.filter(|&h| h != now && has(h)) {
            self.change = Some((from, now, at));
            return Some((from, now, 0.0));
        }
        // As the clock goes, the capture mostly shown is the one it fades into once past half.
        self.shown = Some(if fade >= 0.5 && has(next) { next } else { now });
        Some((now, next, fade))
    }
}

/// Seconds past midnight on the clock, or `hour` o'clock when the player set one.
fn seconds_of_day(hour: Option<u8>) -> f64 {
    match hour {
        Some(hour) => f64::from(hour.min(23)) * 3600.0,
        None => files::local_seconds(),
    }
}

#[cfg(test)]
mod tests;
