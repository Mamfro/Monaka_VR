//! The debug views ([`monaka_overlay`]): what the producer drew for the frame each eye was rendered
//! in, and what the viewer draws itself (the controllers), drawn into the eye images with the
//! pose and field of view each was rendered with, so they move with the image when the runtime
//! reprojects it; switched from the debug menu ([`crate::debug_menu`]). The menu's choices
//! (all but the freeze) are remembered between runs in `%APPDATA%\monaka\viewer-debug.txt`.

use crate::debug_menu::{Buzz, HeadsetMenu};
use crate::debug_panels::HeadsetPanels;
use crate::recorder::Recorder;
use monaka_channel::debug::{DebugChannel, Source};
use monaka_channel::debug_image::ImageReader;
use monaka_channel::page::PageChannel;
use monaka_contract::debug_draw::{DebugDepth, GameReport};
use monaka_channel::{ChannelName, d3d, tick};
use monaka_core::camera::Mat34;
use monaka_contract::debug_draw::{self, Categories, Category, Primitive};
use monaka_core::math::{self, Quat, Vec3};
use monaka_contract::gamepad::Gamepad;
use monaka_contract::protocol::{HandPose, HeadPose, RenderedPair};
use monaka_core::stream;
use monaka_openxr::{Layer, Session, Space, Swapchain};
use monaka_overlay::eye::EyeView;
use monaka_channel::fence::ReadDepth;
use monaka_overlay::lines::LinePass;
use monaka_overlay::quad::QuadPass;
use monaka_overlay::menu::{Command, DepthNear, Settings, Status, View};
use monaka_overlay::egui;
use crate::ring::RingView;
use monaka_overlay::panels::{self, Content, PanelId};
use monaka_overlay::views::{self, ViewPass};
use std::path::PathBuf;
use std::time::Instant;
use windows::Win32::Graphics::Direct3D11::{D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;

/// Where a log depth ramp goes black (metres): it spans near and far at once.
const LOG_RANGE: f32 = 1000.0;
/// How often the wanted categories are said to the producer again (ms).
const SAY_EVERY_MS: u64 = 500;
/// Line width in pixels.
const LINE_WIDTH: f32 = 3.0;
/// How long a capture asked for from the menu takes the eyes as drawn.
const DRAWN_CAPTURE: std::time::Duration = std::time::Duration::from_secs(2);
/// The menu's size in the 2D window (points).
const MENU_POINTS: [f32; 2] = [460.0, 520.0];
/// Lengths of the controllers' axes (metres): the aim pose's, the grip pose's.
const AIM_AXES: f32 = 0.10;
const GRIP_AXES: f32 = 0.05;

pub struct DebugViews<'s> {
    channel: Option<DebugChannel>,
    /// The game's page ([`monaka_contract::page`]), shown as a tab of the menu.
    page: Option<PageChannel>,
    /// None until the first drawing, and after the pass failed (logged once).
    lines: Option<LinePass>,
    lines_failed: bool,
    views: Option<ViewPass>,
    views_failed: bool,
    /// The latest pair came with depth.
    depth: bool,
    name: ChannelName,
    /// The depth a D3D11 game shares for the views (its generation and reader), and the latest of
    /// each eye's frames; whether depth 1 is near in them.
    shared_depth: SharedEyes,
    depth_reversed: bool,
    /// The game's render target shared for the Game buffer view, the latest of each eye's frames.
    shared_target: SharedEyes,
    /// Each eye as drawn, kept for captures that include the drawings (with the menu put in, as
    /// the headset showed it over the eye).
    copies: [Option<ID3D11Texture2D>; 2],
    quad: Option<QuadPass>,
    recorder: Recorder,
    /// A capture asked for from the menu takes the eyes as drawn until then.
    drawn_capture_until: Option<Instant>,
    /// The pair last recorded, and this frame's head and controllers (left aim, right aim, left
    /// grip, right grip) as tracked.
    recorded: Option<u64>,
    tracked: (stream::Pose, [stream::Pose; 4]),
    format: DXGI_FORMAT,
    settings: Settings,
    /// What a panel asked for (a console command, the fake head and controllers changed), until
    /// taken ([`DebugViews::take_asked`]).
    asked: Option<panels::Asked>,
    menu: HeadsetMenu,
    /// The panels switched on in the menu, as the headset shows them.
    panels: HeadsetPanels,
    /// The debug ring: the menu first, then the panels; and the pointer on them.
    ring: RingView<'s>,
    /// A capture asked for with the pointer, for the next read of the pad to say.
    capture_asked: bool,
    said_at: u64,
    /// The pair whose primitives `eyes` holds.
    read: Option<u64>,
    eyes: [Vec<Primitive>; 2],
    sources: [Source; 2],
    /// The viewer's own drawings this frame.
    own: Vec<Primitive>,
    drawn: Vec<Primitive>,
}

impl<'s> DebugViews<'s> {
    /// The views for the run on `channel`, over eye images made in `format`; recordings are saved
    /// into `captures`.
    pub fn new(channel: &ChannelName, format: DXGI_FORMAT, captures: Option<PathBuf>) -> Self {
        let settings = remembered();
        let name = channel;
        let channel = match DebugChannel::open(name) {
            Ok(opened) => Some(opened),
            Err(e) => {
                println!("Debug views: no debug block ({e}); only the viewer's own drawings");
                None
            }
        };
        let page = PageChannel::open(name).map_err(|e| println!("Debug views: no game page ({e})")).ok();
        if settings.wanted.0 != 0 {
            let on: Vec<&str> = Category::ALL.iter().filter(|c| settings.wanted.has(**c)).map(|c| c.label()).collect();
            println!("Debug views on: {}", on.join(", "));
        }
        Self {
            channel,
            page,
            lines: None,
            lines_failed: false,
            views: None,
            views_failed: false,
            depth: false,
            name: name.clone(),
            shared_depth: SharedEyes::default(),
            shared_target: SharedEyes::default(),
            depth_reversed: false,
            copies: [None, None],
            quad: None,
            recorder: Recorder::new(captures),
            drawn_capture_until: None,
            recorded: None,
            tracked: Default::default(),
            format,
            settings,
            asked: None,
            menu: HeadsetMenu::new(format),
            panels: HeadsetPanels::new(format),
            ring: RingView::new("The debug menu", format, 1, 0),
            capture_asked: false,
            said_at: 0,
            read: None,
            eyes: [Vec::new(), Vec::new()],
            sources: [Source::Nothing; 2],
            own: Vec::new(),
            drawn: Vec::new(),
        }
    }

    /// What the menu shows about the run.
    fn status(&mut self) -> Status {
        let viewer = Category::ALL.iter().filter(|c| c.viewer_drawn()).fold(Categories::NONE, |set, c| set.with(*c, true));
        let offered = Categories(self.channel.as_mut().map_or(0, |c| c.offered().0) | viewer.0);
        let synced = self.sources.iter().any(|s| matches!(s, Source::Synced(_)));
        let depth = self.depth || offered.has(Category::Depth);
        let depth_passes = self.channel.as_mut().and_then(|c| c.depth()).map_or(0, |d| d.passes);
        let (listed, shared) = self.channel.as_mut().map(|c| c.candidates()).unwrap_or_default();
        let mut depth_buffers = [Default::default(); monaka_contract::debug_draw::DEPTH_CANDIDATES];
        depth_buffers[..listed.len()].copy_from_slice(&listed);
        let (listed_targets, target_shared) = self.channel.as_mut().map(|c| c.targets()).unwrap_or_default();
        let mut targets = [Default::default(); monaka_contract::debug_draw::DEPTH_CANDIDATES];
        targets[..listed_targets.len()].copy_from_slice(&listed_targets);
        Status {
            offered,
            synced,
            depth,
            depth_passes,
            depth_buffers,
            depth_buffer_count: listed.len() as u32,
            depth_buffer_shared: shared,
            targets,
            target_count: listed_targets.len() as u32,
            target_shared,
            recorded: self.recorder.seconds(),
            page: self.page.as_mut().and_then(PageChannel::read).unwrap_or_default(),
        }
    }

    /// The eyes stop taking new pairs: the held one can be looked around.
    pub fn frozen(&self) -> bool {
        self.settings.frozen
    }

    /// Captures are taken from the eyes as drawn ([`DebugViews::drawn_eyes`]), not as they came:
    /// by the menu's switch, or for a capture asked for from the menu.
    pub fn captures_drawn(&self) -> bool {
        self.settings.capture_overlays || self.drawn_capture_until.is_some_and(|until| Instant::now() < until)
    }

    /// Both eyes as drawn this frame, while captures include the drawings.
    pub fn drawn_eyes(&self) -> Option<[&ID3D11Texture2D; 2]> {
        match (&self.copies, self.captures_drawn()) {
            ([Some(left), Some(right)], true) => Some([left, right]),
            _ => None,
        }
    }

    /// Whether the pairs' depth is wanted, whether or not the headset takes it.
    pub fn wants_depth(&self) -> bool {
        self.settings.view.needs_depth()
    }

    /// Whether the debug menu is open (it has the pad).
    pub fn menu_open(&self) -> bool {
        self.menu.open()
    }

    /// Opens the debug menu now (asked from Monaka's home), ahead of `head`; the pad drives it.
    pub fn open_menu(&mut self, head: Option<(Quat, Vec3)>) {
        let status = self.status();
        self.menu.open_now(&status);
        self.open_ring(head);
    }

    /// The ring round `head` (none: where it stood), the menu in front.
    fn open_ring(&mut self, head: Option<(Quat, Vec3)>) {
        if let Some(head) = head {
            self.ring.place_at(head);
        }
        self.ring.show_in_front(0);
    }

    /// The pad before anything else reads it ([`HeadsetMenu::pad`]): the buzzes to give, and
    /// whether a capture (of the eyes as drawn, the menu on them) was asked for.
    pub fn pad(&mut self, pad: &mut Gamepad, head: Option<(Quat, Vec3)>, hands: &[HandPose; 2], now: Instant) -> (Vec<Buzz>, bool) {
        // Read even while closed: the first opening with a game page goes to it.
        let status = self.status();
        let was_open = self.menu.open();
        let mut asked = self.menu.pad(pad, now, &status);
        self.panels.follow(self.settings.panels);
        self.ring.set_pages(1 + self.panels.len());
        // Opened: the ring round the head, the menu in front; LB / RB turn it.
        if self.menu.open() && !was_open {
            self.open_ring(head);
        }
        // Panels switched on in an earlier run show from the start, round the head.
        if !self.ring.placed()
            && self.panels.len() > 0
            && let Some(head) = head
        {
            self.ring.place_at(head);
        }
        if asked.turn != 0 {
            self.ring.turn(asked.turn as isize, now);
        }
        self.ring.step(now);
        if self.menu.open() {
            asked.buzzes.extend(self.ring.point(pad, hands, |_| true));
            *pad = Gamepad::default();
        } else if was_open {
            self.ring.leave();
        }
        let capture = std::mem::take(&mut self.capture_asked);
        if capture {
            asked.buzzes.extend([monaka_contract::protocol::LEFT_HAND, monaka_contract::protocol::RIGHT_HAND].map(|hand| (hand, 0.6, 0.15)));
        }
        (asked.buzzes, capture)
    }

    /// The 2D window's menu and panels, as egui windows used with the mouse: whether a capture (of
    /// the eyes as drawn) was asked for.
    pub fn window_ui(&mut self, ctx: &egui::Context, content: &Content<'_>, now: Instant) -> bool {
        let status = self.status();
        let before = self.settings;
        let (menu, settings) = (self.menu.model(), &mut self.settings);
        // Opened once at the start: the first game page there is shows, as in the headset.
        menu.opened(&status);
        let mut command = None;
        egui::Window::new("Monaka debug")
            .title_bar(false)
            .frame(egui::Frame::NONE)
            .resizable(false)
            .fixed_size(MENU_POINTS)
            .default_pos([12.0, 12.0])
            .show(ctx, |ui| {
                if let Some(click) = monaka_overlay::menu::panel::layout(ui, menu, settings, &status, true, false) {
                    command = menu.click(click, settings, &status);
                }
            });
        for id in settings.panels.iter() {
            let mut open = true;
            let asked = panels::window(id).default_pos([MENU_POINTS[0] + 24.0, 12.0]).show(ctx, |ui| panels::window_page(ui, id, content, &mut open)).and_then(|shown| shown.inner.flatten());
            if asked.is_some() {
                self.asked = asked;
            }
            if !open {
                settings.panels = settings.panels.with(id, false);
            }
        }
        self.act(before, command, now)
    }

    /// The menu's settings as they are.
    pub fn settings(&self) -> Settings {
        self.settings
    }

    /// Changes the menu's settings from outside it (the 2D window's command line), remembered as a
    /// change in the menu is.
    pub fn change_settings(&mut self, change: impl FnOnce(&mut Settings), now: Instant) {
        let before = self.settings;
        change(&mut self.settings);
        self.act(before, None, now);
    }

    /// What a panel asked for since the last call.
    pub fn take_asked(&mut self) -> Option<panels::Asked> {
        self.asked.take()
    }

    /// What follows a press on the menu, by pad or pointer: settings changed are remembered, a
    /// command run; whether a capture was asked for.
    fn act(&mut self, before: Settings, command: Option<Command>, now: Instant) -> bool {
        let capture = command == Some(Command::SaveCapture);
        if capture {
            // Long enough for the capture's pairs to arrive; the menu stays open, so it is in them.
            self.drawn_capture_until = Some(now + DRAWN_CAPTURE);
        }
        if self.settings != before {
            self.said_at = 0;
            remember(&self.settings);
            if !self.settings.recording {
                self.recorder.clear();
            }
        }
        if let (Some(Command::Page { id, press }), Some(page)) = (command, self.page.as_mut())
            && !page.press(id, press)
        {
            println!("Debug views: a press on the game page was lost (its block was busy)");
        }
        if command == Some(Command::SaveRecording) {
            match self.recorder.save() {
                Ok(path) => println!("Recording: the last {:.0} s saved to {}", self.recorder.seconds(), path.display()),
                Err(e) => println!("Recording: not saved ({e})"),
            }
        }
        capture
    }

    /// Hands the debug ring to the headset: the panels switched on (`content`: what they show),
    /// and the menu while it is open, each drawn when it changed.
    pub fn show_in_headset(&mut self, session: &'s Session<'_>, device: &ID3D11Device, context: &ID3D11DeviceContext, content: &Content<'_>, now: Instant) {
        self.panels.follow(self.settings.panels);
        self.ring.set_pages(1 + self.panels.len());
        let open = self.menu.open();
        // The outline marks the one in front while the menu is open (LB / RB move it).
        let front = |page: usize, ring: &RingView<'_>| open && ring.front() == page;
        for index in 0..self.panels.len() {
            let page = 1 + index;
            let (in_front, events) = (front(page, &self.ring), self.ring.take_events(page));
            let (ring, panels) = (&mut self.ring, &mut self.panels);
            let mut asked = None;
            ring.show(page, session, context, |fresh| {
                let (image, said) = panels.draw(index, device, context, content, in_front, events, now, fresh)?;
                asked = said;
                Ok(image)
            });
            if asked.is_some() {
                self.asked = asked;
            }
        }
        if !open {
            return;
        }
        let status = self.status();
        let settings = self.settings;
        let (in_front, events) = (front(0, &self.ring), self.ring.take_events(0));
        let (ring, menu) = (&mut self.ring, &mut self.menu);
        let mut click = None;
        ring.show(0, session, context, |fresh| {
            let (image, clicked) = menu.draw(device, context, &settings, &status, in_front, events, fresh)?;
            click = clicked;
            Ok(image)
        });
        if let Some(click) = click {
            let before = self.settings;
            let (menu, settings) = (self.menu.model(), &mut self.settings);
            let command = menu.click(click, settings, &status);
            if self.act(before, command, now) {
                self.capture_asked = true;
            }
        }
    }

    /// The panels' layers, and the menu's while it is open.
    pub fn layers<'a>(&'a self, space: &'a Space<'a>) -> impl Iterator<Item = Layer<'a>> + 'a {
        let first = if self.menu.open() { 0 } else { 1 };
        (first..=self.panels.len()).filter_map(move |page| self.ring.layer(page, space))
    }

    /// The game's timing and settings, as its producer says them.
    pub fn game(&mut self) -> Option<GameReport> {
        self.channel.as_mut().and_then(DebugChannel::game)
    }

    /// The game frames the eyes of pair `sequence` were drawn in, while the Stats panel shows
    /// (the producer names them then).
    pub fn eye_keys(&mut self, sequence: u64) -> Option<[u64; 2]> {
        if !self.settings.panels.has(PanelId::Stats) {
            return None;
        }
        self.channel.as_mut()?.eye_keys(sequence)
    }

    /// This frame's head and controllers (aim and grip poses, tracking space): the controllers drawn
    /// as axes, both kept for a recording.
    pub fn controllers(&mut self, head: Option<&HeadPose>, aims: &[HandPose; 2], grips: &[HandPose; 2]) {
        let pose = |p: &HandPose| stream::Pose { orientation: p.orientation, position: p.position, tracked: p.valid != 0 };
        let head = head.map_or_else(stream::Pose::default, |h| stream::Pose { orientation: h.orientation, position: h.position, tracked: h.valid != 0 });
        self.tracked = (head, [pose(&aims[0]), pose(&aims[1]), pose(&grips[0]), pose(&grips[1])]);
        self.own.clear();
        if !self.settings.wanted.has(Category::Controllers) {
            return;
        }
        for (pose, size) in aims.iter().map(|p| (p, AIM_AXES)).chain(grips.iter().map(|p| (p, GRIP_AXES))) {
            if pose.valid != 0 {
                self.own.extend(debug_draw::axes(Category::Controllers, &matrix(pose.orientation, pose.position), size));
            }
        }
    }

    /// Draws eye `eye` of pair `sequence` (rendered as `pair` says) over `image`, its swapchain
    /// image, just filled with the eye: the menu's view of it (from `sources`, both eyes' images,
    /// and `depth`, the pair's), then the drawings over it.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_eye(
        &mut self,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        image: &ID3D11Texture2D,
        eye: usize,
        pair: Option<&RenderedPair>,
        sequence: u64,
        sources: [&ID3D11Texture2D; 2],
        depth: Option<(&ReadDepth, [&ID3D11Texture2D; 2])>,
    ) {
        if eye == 0 {
            self.say();
            self.depth = depth.is_some();
            if depth.is_none() && self.settings.view.needs_depth() {
                self.read_shared_depth(device, context);
            }
            if self.settings.view == View::GameBuffer
                && let Some(record) = self.channel.as_mut().and_then(|c| c.target())
            {
                self.shared_target.update(device, context, &self.name.debug_target(), &record);
            }
        }
        self.draw_over(device, context, image, eye, pair, sequence, sources, depth);
        if self.captures_drawn() {
            self.keep(device, context, image, eye, pair);
        }
        if eye == 1 && self.settings.recording && self.recorded != Some(sequence) {
            self.recorded = Some(sequence);
            self.record(pair, sequence);
        }
    }

    /// Keeps pair `sequence` as shown (its eyes `pair`, this frame's head and controllers, each
    /// eye's drawings) in the recording.
    fn record(&mut self, pair: Option<&RenderedPair>, sequence: u64) {
        let Some(pair) = pair else { return };
        let eyes = [0, 1].map(|eye| {
            let e = &pair.eyes[eye];
            stream::Eye { orientation: e.orientation, position: math::eye_position(e, eye), fov: e.fov[eye] }
        });
        let (head, controllers) = self.tracked;
        let frame = stream::Frame { sequence, tick: tick(), eyes, head, controllers, primitives: self.eyes.clone() };
        self.recorder.push(&frame);
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_over(
        &mut self,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        image: &ID3D11Texture2D,
        eye: usize,
        pair: Option<&RenderedPair>,
        sequence: u64,
        sources: [&ID3D11Texture2D; 2],
        depth: Option<(&ReadDepth, [&ID3D11Texture2D; 2])>,
    ) {
        let wanted = self.settings.wanted;
        if self.read != Some(sequence) && (wanted.draws() || self.settings.view == View::FrameAge || self.settings.recording) {
            self.read = Some(sequence);
            if let Some(channel) = self.channel.as_mut() {
                self.sources = channel.read_pair(sequence, &mut self.eyes);
            }
        }
        // The views need no poses: a frame on the flat screen gets them too.
        self.draw_view(device, context, image, eye, sources, depth);
        // The drawings are placed by the pose each eye was rendered from.
        let Some(pair) = pair else { return };
        if !wanted.draws() || self.lines_failed {
            return;
        }
        self.drawn.clear();
        self.drawn.extend(self.eyes[eye].iter().chain(&self.own).filter(|p| Category::from_index(p.category).is_some_and(|c| wanted.has(c))));
        if self.drawn.is_empty() {
            return;
        }
        let e = &pair.eyes[eye];
        let view = EyeView { orientation: e.orientation, position: math::eye_position(e, eye), fov: e.fov[eye] };
        if self.lines.is_none() {
            match LinePass::new(device) {
                Ok(pass) => self.lines = Some(pass),
                Err(e) => {
                    println!("Debug views: the line pass could not be made ({e}); no lines this run");
                    self.lines_failed = true;
                    return;
                }
            }
        }
        if let Some(lines) = self.lines.as_mut()
            && let Err(e) = lines.draw(device, context, image, self.format, &view, &self.drawn, LINE_WIDTH)
        {
            println!("Debug views: drawing failed ({e}); no lines this run");
            self.lines = None;
            self.lines_failed = true;
        }
    }

    /// Keeps eye `eye` as drawn (`image`), for a capture, with the menu put in where the eye
    /// (rendered as `pair` says) saw it.
    fn keep(&mut self, device: &ID3D11Device, context: &ID3D11DeviceContext, image: &ID3D11Texture2D, eye: usize, pair: Option<&RenderedPair>) {
        let mut desc = d3d::texture_desc(image);
        desc.Format = self.format;
        let fits = self.copies[eye].as_ref().is_some_and(|c| {
            let had = d3d::texture_desc(c);
            (had.Width, had.Height) == (desc.Width, desc.Height)
        });
        if !fits {
            let mut copy_desc = d3d::private_copy_desc(&desc);
            copy_desc.BindFlags = (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32;
            self.copies[eye] = d3d::create_texture(device, &copy_desc).ok();
        }
        let Some(copy) = &self.copies[eye] else { return };
        d3d::copy_texture(context, copy, image);
        let Some(pair) = pair else { return };
        let panels = (0..self.panels.len()).filter_map(|index| Some((self.panels.image(index)?, self.ring.rectangle(1 + index)?)));
        let menu = self.menu.image().and_then(|image| Some((image, self.ring.rectangle(0)?)));
        let shown: Vec<_> = panels.chain(menu).collect();
        if shown.is_empty() {
            return;
        }
        if self.quad.is_none() {
            self.quad = QuadPass::new(device).map_err(|e| println!("Debug views: the menu cannot go into captures ({e})")).ok();
        }
        let e = &pair.eyes[eye];
        let view = EyeView { orientation: e.orientation, position: math::eye_position(e, eye), fov: e.fov[eye] };
        for (picture, panel) in shown {
            if let Some(quad) = self.quad.as_mut()
                && let Err(e) = quad.draw(device, context, copy, self.format, &view, picture, &panel)
            {
                println!("Debug views: the menu could not go into a capture ({e})");
            }
        }
    }

    /// The menu's view of eye `eye` over `image`, when it is not the plain picture.
    fn draw_view(&mut self, device: &ID3D11Device, context: &ID3D11DeviceContext, image: &ID3D11Texture2D, eye: usize, sources: [&ID3D11Texture2D; 2], depth: Option<(&ReadDepth, [&ID3D11Texture2D; 2])>) {
        if self.settings.view == View::Picture || self.views_failed {
            return;
        }
        if self.views.is_none() {
            match ViewPass::new(device) {
                Ok(pass) => self.views = Some(pass),
                Err(e) => {
                    println!("Debug views: the view pass could not be made ({e}); the eyes show the picture");
                    self.views_failed = true;
                    return;
                }
            }
        }
        let keys = self.sources.map(|s| match s {
            Source::Synced(key) => Some(key),
            _ => None,
        });
        let age = match keys {
            [Some(left), Some(right)] => (left.max(right) - keys[eye].unwrap_or(0)) as u32,
            _ => 0,
        };
        let depth = match depth {
            Some((read, textures)) => Some(views::Depth { texture: textures[eye], rect: read.rects[eye], distances: read.params.distances_at_0_and_1() }),
            None => shared_depth_for(self.shared_depth.eyes[eye].as_ref(), self.depth_reversed),
        };
        let view = views::Eye { image: sources[eye], other: sources[1 - eye], depth, age, range: if self.settings.depth_log { LOG_RANGE } else { self.settings.depth_range }, logarithmic: self.settings.depth_log, buffer: self.shared_target.eyes[eye].as_ref() };
        if let Some(pass) = self.views.as_mut()
            && let Err(e) = pass.draw(device, context, image, self.format, self.settings.view, &view)
        {
            println!("Debug views: the view could not be drawn ({e}); the eyes show the picture");
            self.views = None;
            self.views_failed = true;
        }
    }

    /// The depth a D3D11 game shares, read when there is a new one, as the latest of the eye whose
    /// frame it is (both, when that is not known).
    fn read_shared_depth(&mut self, device: &ID3D11Device, context: &ID3D11DeviceContext) {
        let Some(record) = self.channel.as_mut().and_then(|c| c.depth()) else { return };
        if self.shared_depth.update(device, context, &self.name.debug_depth(), &record) {
            // A depth the game never clears is not known: taken as reversed, as most draw now.
            self.depth_reversed = match self.settings.depth_near {
                DepthNear::Auto => record.reversed != 0,
                DepthNear::Zero => false,
                DepthNear::One => true,
            };
        }
    }

    /// Tells the producer what is wanted, now and then.
    fn say(&mut self) {
        let now = tick();
        if now.saturating_sub(self.said_at) < SAY_EVERY_MS {
            return;
        }
        self.said_at = now;
        // A depth view asks a D3D11 game to share its depth; the Stats panel, to time its work.
        let stats = self.settings.panels.has(PanelId::Stats);
        let wanted = self.settings.wanted.with(Category::Depth, self.settings.view.needs_depth()).with(Category::Targets, self.settings.view == View::GameBuffer).with(Category::Stats, stats);
        if let Some(channel) = self.channel.as_mut() {
            channel.want(monaka_channel::debug::Wanted { categories: wanted, pass: self.settings.depth_pass, buffer: self.settings.depth_buffer, target: self.settings.target });
        }
    }
}

/// The shared depth of the frame `key` (else the newest), for the views.
fn shared_depth_for(texture: Option<&ID3D11Texture2D>, reversed: bool) -> Option<views::Depth<'_>> {
    let texture = texture?;
    let desc = d3d::texture_desc(texture);
    // Its near and far are not known: near is taken as 10 cm, far as infinitely far.
    let distances = if reversed { (f32::INFINITY, 0.1) } else { (0.1, f32::INFINITY) };
    Some(views::Depth { texture, rect: [0, 0, desc.Width, desc.Height], distances })
}

/// An image the game shares for the views, kept as the latest of each eye's frames (both, when
/// the eye is not known).
#[derive(Default)]
struct SharedEyes {
    /// The generation opened, and its reader.
    reader: Option<(u32, ImageReader)>,
    eyes: [Option<ID3D11Texture2D>; 2],
}

impl SharedEyes {
    /// Reads the image named `name` that `record` describes when there is a new one; whether one
    /// was read.
    fn update(&mut self, device: &ID3D11Device, context: &ID3D11DeviceContext, name: &str, record: &DebugDepth) -> bool {
        if self.reader.as_ref().is_none_or(|(generation, _)| *generation != record.generation) {
            self.eyes = [None, None];
            self.reader = match ImageReader::open(name, device) {
                Ok(reader) => Some((record.generation, reader)),
                Err(e) => {
                    println!("Debug views: the game's shared image {name} cannot be opened ({e})");
                    None
                }
            };
        }
        let Some((_, reader)) = self.reader.as_mut() else { return false };
        for slot in &mut self.eyes {
            if slot.is_none() {
                let mut desc = d3d::simple_texture_desc(reader.width, reader.height, reader.format);
                desc.BindFlags = D3D11_BIND_SHADER_RESOURCE.0 as u32;
                *slot = d3d::create_texture(device, &desc).ok();
            }
        }
        let [Some(left), Some(right)] = &self.eyes else { return false };
        let (into, also) = match record.eye {
            0 => (left, None),
            1 => (right, None),
            _ => (left, Some(right)),
        };
        if !reader.read(context, into) {
            return false;
        }
        if let Some(also) = also {
            d3d::copy_texture(context, also, into);
        }
        true
    }
}

/// An eye's swapchain, made so the debug views can draw into it; a runtime that refuses that gets
/// one filled by copies only, and the views stay off.
pub fn eye_swapchain<'a>(session: &'a Session<'_>, format: i64, width: u32, height: u32) -> monaka_openxr::Result<Swapchain<'a>> {
    session.create_drawn_swapchain(format, width, height).or_else(|e| {
        println!("Debug views: eye images cannot be drawn into ({e}); the views stay off");
        session.create_swapchain(format, width, height)
    })
}

/// A pose as a [`Mat34`] (columns its x, y, z axes and its position).
fn matrix(orientation: Quat, position: Vec3) -> Mat34 {
    let [x, y, z] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]].map(|axis| math::rotate(orientation, axis));
    [x[0], y[0], z[0], position[0], x[1], y[1], z[1], position[1], x[2], y[2], z[2], position[2]]
}

fn settings_file() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|dir| PathBuf::from(dir).join("monaka").join("viewer-debug.txt"))
}

/// The menu's choices at the end of the last run.
fn remembered() -> Settings {
    settings_file().and_then(|path| std::fs::read_to_string(path).ok()).map_or_else(Settings::default, |text| parse(&text))
}

/// The choices kept between runs, one `id=value` line each: how a game's depth is taken and shown,
/// and whether captures include the drawings. Nothing that changes what the eyes show is kept: a
/// run starts with the drawings off, the picture plain, and nothing recorded.
fn parse(text: &str) -> Settings {
    let mut settings = Settings::default();
    for (id, value) in text.lines().filter_map(|line| line.split_once('=')).map(|(id, value)| (id.trim(), value.trim())) {
        match id {
            "depth_pass" => settings.depth_pass = value.parse().unwrap_or(1),
            "depth_buffer" => settings.depth_buffer = value.parse().unwrap_or(0),
            "target" => settings.target = value.parse().unwrap_or(0),
            "depth_near" => settings.depth_near = DepthNear::ALL.into_iter().find(|n| n.label() == value).unwrap_or_default(),
            "depth_range" => settings.depth_range = value.parse::<f32>().ok().filter(|r| monaka_overlay::menu::DEPTH_RANGES.contains(r)).unwrap_or(10.0),
            "depth_curve" => settings.depth_log = value == "log",
            "capture_overlays" => settings.capture_overlays = value == "on",
            _ => {}
        }
    }
    settings
}

fn text_of(settings: &Settings) -> String {
    let on_off = |on: bool| if on { "on" } else { "off" };
    format!(
        "depth_pass={}\ndepth_buffer={}\ntarget={}\ndepth_near={}\ndepth_range={}\ndepth_curve={}\ncapture_overlays={}\n",
        settings.depth_pass,
        settings.depth_buffer,
        settings.target,
        settings.depth_near.label(),
        settings.depth_range,
        if settings.depth_log { "log" } else { "linear" },
        on_off(settings.capture_overlays)
    )
}

fn remember(settings: &Settings) {
    let Some(path) = settings_file() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(&path, text_of(settings)) {
        println!("Debug views: could not remember the choices in {} ({e})", path.display());
    }
}

#[cfg(test)]
mod tests;
