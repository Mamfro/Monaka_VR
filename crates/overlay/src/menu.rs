//! The in-headset debug menu: tabs of items, moved through like a console menu (the left stick up
//! and down, left and right for an item's value, A to switch or run, LT and RT for the tabs, B to
//! close). The menu owns the selection; what each item switches lives in [`Settings`], which the
//! viewer acts on. [`panel`] draws it.

use crate::panels::{PanelId, Panels};
use monaka_core::debug_draw::{Categories, Category, DEPTH_CANDIDATES, DepthCandidate};
use monaka_core::page::{self, Page};

pub mod panel;
pub(crate) mod painter;

#[cfg(test)]
mod tests;

/// What the viewer shows in place of each eye's image.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum View {
    #[default]
    Picture,
    /// The depth buffer as grey, near white.
    Depth,
    /// The picture with the depth buffer's edges over it.
    DepthEdges,
    /// The picture tinted by how many frames old each eye is.
    FrameAge,
    /// Where the two eyes differ.
    Difference,
    /// One of the game's render targets, as stored (its normals, its albedo...).
    GameBuffer,
}

impl View {
    pub const ALL: [View; 6] = [View::Picture, View::Depth, View::DepthEdges, View::FrameAge, View::Difference, View::GameBuffer];

    pub const fn label(self) -> &'static str {
        match self {
            View::Picture => "Picture",
            View::Depth => "Depth",
            View::DepthEdges => "Depth edges",
            View::FrameAge => "Frame age",
            View::Difference => "Left/right difference",
            View::GameBuffer => "Game buffer",
        }
    }

    /// Needs each eye's depth.
    pub const fn needs_depth(self) -> bool {
        matches!(self, View::Depth | View::DepthEdges)
    }
}

/// One line of a tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    /// A drawing or an action, on or off.
    Category(Category),
    /// What each eye shows (left and right pick).
    View,
    /// Which of the game's render targets the Game buffer view shows: the game side's pick or one
    /// of those it drew into (left and right pick).
    Target,
    /// Which of the game's depth buffers a depth view shows, where the game shares its depth: its
    /// own pick or one of those it drew with (left and right pick).
    DepthBuffer,
    /// Which pass of the game's depth a depth view shows, where the game shares its depth and
    /// uses it for several passes a frame (left and right pick).
    DepthPass,
    /// Which end of the depth is near (left and right pick).
    DepthNear,
    /// The distance a depth view goes black at (left and right pick).
    DepthRange,
    /// The depth view's ramp: even steps of distance, or of its doublings.
    DepthCurve,
    /// The eyes stop taking new frames, so the held one can be looked around.
    Freeze,
    /// Captures (the capture chord) are saved with the debug drawing on them.
    CaptureOverlays,
    /// The last 30 seconds are kept, to save on request.
    Record,
    /// Saves the last 30 seconds kept.
    SaveRecording,
    /// Saves a capture of the eyes with the debug drawing on them (the menu closes for it).
    SaveCapture,
    /// A panel of its own beside the menu, on or off ([`crate::panels`]).
    Panel(PanelId),
    /// Row `n` of the game's page ([`monaka_core::page`]).
    Page(u8),
}

/// What a run of an item asks the viewer to do once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    SaveRecording,
    SaveCapture,
    /// A press on the game page's button `id`, for the producer.
    Page { id: u32, press: page::Press },
}

/// Which end of a depth is near.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DepthNear {
    /// As the game says; a depth never cleared is taken as reversed (1 near), as most recent
    /// engines draw.
    #[default]
    Auto,
    Zero,
    One,
}

impl DepthNear {
    pub const ALL: [DepthNear; 3] = [DepthNear::Auto, DepthNear::Zero, DepthNear::One];

    pub const fn label(self) -> &'static str {
        match self {
            DepthNear::Auto => "Auto",
            DepthNear::Zero => "Depth 0",
            DepthNear::One => "Depth 1",
        }
    }
}

/// The distances (metres) a depth view can go black at.
pub const DEPTH_RANGES: [f32; 6] = [2.0, 5.0, 10.0, 25.0, 50.0, 100.0];

/// What the menu switches; the viewer acts on it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub wanted: Categories,
    pub view: View,
    /// When a shared depth is taken: just before the game's clear of it this many (1 the first:
    /// the scene, before the game reuses it), or at the present (0).
    pub depth_pass: u32,
    /// The shared depth's buffer: 0 the game side's own pick, else that one of the list (1 the
    /// first).
    pub depth_buffer: u32,
    pub depth_near: DepthNear,
    /// The Game buffer view's render target: 0 the game side's pick, else that one of the list.
    pub target: u32,
    /// The distance (metres) a depth view goes black at, one of [`DEPTH_RANGES`].
    pub depth_range: f32,
    /// The depth view's ramp in even steps per doubling of distance (else of distance).
    pub depth_log: bool,
    pub frozen: bool,
    pub capture_overlays: bool,
    pub recording: bool,
    /// The panels shown beside the menu.
    pub panels: Panels,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            wanted: Categories::NONE,
            view: View::Picture,
            depth_pass: 1,
            depth_buffer: 0,
            depth_near: DepthNear::Auto,
            target: 0,
            depth_range: 10.0,
            depth_log: false,
            frozen: false,
            capture_overlays: false,
            recording: false,
            panels: Panels::NONE,
        }
    }
}

/// What the menu shows about the run, beside the settings.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Status {
    /// What the producer offers (with what the viewer draws itself).
    pub offered: Categories,
    /// The producer's drawings land on the frame each eye was drawn in.
    pub synced: bool,
    /// The pairs come with depth, or the game shares it.
    pub depth: bool,
    /// How many passes the game's shared depth had in the latest frame (0: none shared).
    pub depth_passes: u32,
    /// The depth buffers the game drew with that can be shared, and which is shared (1 the first,
    /// 0 none).
    pub depth_buffers: [DepthCandidate; DEPTH_CANDIDATES],
    pub depth_buffer_count: u32,
    pub depth_buffer_shared: u32,
    /// The same for the game's render targets.
    pub targets: [DepthCandidate; DEPTH_CANDIDATES],
    pub target_count: u32,
    pub target_shared: u32,
    /// Seconds recorded and kept.
    pub recorded: f32,
    /// The game's page, while its producer writes one (empty otherwise).
    pub page: Page,
}

/// A press the menu acts on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Press {
    Up,
    Down,
    Left,
    Right,
    Switch,
    PreviousTab,
    NextTab,
    Close,
}

pub struct Tab {
    pub name: &'static str,
    pub items: &'static [Item],
}

pub const TABS: [Tab; 6] = [
    Tab {
        name: "Overlays",
        items: &[
            Item::Category(Category::Controllers),
            Item::Category(Category::Skeleton),
            Item::Category(Category::BoneAxes),
            Item::Category(Category::Fingers),
            Item::Category(Category::HandTargets),
            Item::Category(Category::Aim),
            Item::Category(Category::Swings),
            Item::Category(Category::Melee),
            Item::Category(Category::RoomScale),
            Item::Category(Category::MenuReach),
            Item::Category(Category::Probe),
        ],
    },
    Tab { name: "Game", items: &[Item::Category(Category::HideHud), Item::Category(Category::SkipHandRig)] },
    Tab { name: "Views", items: &[Item::View, Item::Target, Item::DepthBuffer, Item::DepthPass, Item::DepthNear, Item::DepthCurve, Item::DepthRange] },
    Tab { name: "Tools", items: &[Item::SaveCapture, Item::Freeze, Item::CaptureOverlays, Item::Record, Item::SaveRecording] },
    Tab { name: "Panels", items: &[Item::Panel(PanelId::Stats), Item::Panel(PanelId::Console), Item::Panel(PanelId::FakeVr)] },
    // Its lines are the game page's rows; named by the page's title.
    Tab { name: "Game page", items: &[] },
];

/// The tab showing the game's page, there only while the game writes one.
pub const PAGE_TAB: usize = 5;

/// A click on the menu with a pointer (the 2D window): on a tab, or on a line of the tab shown
/// (`Switch` for the primary button, `Left` for the secondary: a choice stepped back).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Click {
    Tab(usize),
    Line(usize, Press),
}

/// The menu's place: the tab shown and the line selected on each.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Menu {
    pub tab: usize,
    pub selected: [usize; TABS.len()],
    pub open: bool,
    /// The menu has opened on the game's page once (the first opening with one goes there).
    pub paged: bool,
}

impl Menu {
    pub fn item(&self) -> Item {
        if self.tab == PAGE_TAB {
            return Item::Page(self.selected[PAGE_TAB].min(u8::MAX as usize) as u8);
        }
        TABS[self.tab].items[self.selected[self.tab]]
    }

    /// The menu opened: on the game's page the first time there is one, and never on a page gone.
    pub fn opened(&mut self, status: &Status) {
        let buttons = buttons(status);
        if !self.paged && !buttons.is_empty() {
            self.paged = true;
            self.tab = PAGE_TAB;
            self.selected[PAGE_TAB] = buttons[0];
        }
        if self.tab == PAGE_TAB && status.page.is_empty() {
            self.tab = 0;
        }
    }

    /// The tabs there are now: the page's only while the game writes one.
    fn shown(tab: usize, status: &Status) -> bool {
        tab != PAGE_TAB || !status.page.is_empty()
    }

    /// Whether `item` can be switched in this run.
    pub fn available(item: Item, settings: &Settings, status: &Status) -> bool {
        match item {
            Item::Category(category) => status.offered.has(category),
            Item::View => true,
            Item::Target => settings.view == View::GameBuffer && status.target_count > 0,
            Item::DepthPass => settings.view.needs_depth() && status.depth_passes > 0,
            Item::DepthNear | Item::DepthCurve => settings.view.needs_depth() && status.depth,
            // A log curve spans a fixed 10 cm to 1 km.
            Item::DepthRange => settings.view.needs_depth() && status.depth && !settings.depth_log,
            Item::DepthBuffer => settings.view.needs_depth() && status.depth_buffer_count > 0,
            Item::SaveRecording => settings.recording && status.recorded > 0.0,
            Item::Freeze | Item::CaptureOverlays | Item::Record | Item::SaveCapture | Item::Panel(_) => true,
            Item::Page(line) => status.page.rows().get(usize::from(line)).is_some_and(|r| r.kind() == page::Kind::Button),
        }
    }

    /// Acts on `press`; a command for the viewer when an item runs.
    pub fn press(&mut self, press: Press, settings: &mut Settings, status: &Status) -> Option<Command> {
        if !Self::shown(self.tab, status) {
            self.tab = 0;
        }
        let items = TABS[self.tab].items.len();
        let line = &mut self.selected[self.tab];
        let step_tab = |tab: usize, step: usize| (1..=TABS.len()).map(|k| (tab + k * step) % TABS.len()).find(|t| Self::shown(*t, status)).unwrap_or(0);
        match press {
            // The page's lines are its buttons: its other rows are read, not chosen.
            Press::Up | Press::Down if self.tab == PAGE_TAB => {
                let buttons = buttons(status);
                let at = buttons.iter().position(|b| b == line);
                *line = match (at, press) {
                    (_, _) if buttons.is_empty() => *line,
                    (None, _) => buttons[0],
                    (Some(at), Press::Up) => buttons[(at + buttons.len() - 1) % buttons.len()],
                    (Some(at), _) => buttons[(at + 1) % buttons.len()],
                };
            }
            Press::Up => *line = (*line + items - 1) % items,
            Press::Down => *line = (*line + 1) % items,
            Press::PreviousTab => self.tab = step_tab(self.tab, TABS.len() - 1),
            Press::NextTab => self.tab = step_tab(self.tab, 1),
            Press::Close => self.open = false,
            Press::Left | Press::Right | Press::Switch => {
                let item = self.item();
                if !Self::available(item, settings, status) {
                    return None;
                }
                return change(item, press, settings, status);
            }
        }
        None
    }

    /// Acts on a pointer's click: a tab shown, or a line chosen and pressed.
    pub fn click(&mut self, click: Click, settings: &mut Settings, status: &Status) -> Option<Command> {
        match click {
            Click::Tab(tab) => {
                if tab < TABS.len() && Self::shown(tab, status) {
                    self.tab = tab;
                }
                None
            }
            Click::Line(line, press) => {
                let lines = if self.tab == PAGE_TAB { status.page.rows().len() } else { TABS[self.tab].items.len() };
                if line >= lines {
                    return None;
                }
                self.selected[self.tab] = line;
                self.press(press, settings, status)
            }
        }
    }
}

fn change(item: Item, press: Press, settings: &mut Settings, status: &Status) -> Option<Command> {
    match item {
        Item::Category(category) => settings.wanted = settings.wanted.with(category, !settings.wanted.has(category)),
        Item::View => {
            let at = View::ALL.iter().position(|v| *v == settings.view).unwrap_or(0);
            let step = if press == Press::Left { View::ALL.len() - 1 } else { 1 };
            settings.view = View::ALL[(at + step) % View::ALL.len()];
        }
        Item::DepthPass => {
            // 0 (at the present), then 1 to the passes there are, round.
            let count = status.depth_passes.max(1) + 1;
            let step = if press == Press::Left { count - 1 } else { 1 };
            settings.depth_pass = (settings.depth_pass.min(count - 1) + step) % count;
        }
        Item::Target => {
            // 0 (the game side's pick), then each listed target, round.
            let count = status.target_count + 1;
            let step = if press == Press::Left { count - 1 } else { 1 };
            settings.target = (settings.target.min(count - 1) + step) % count;
        }
        Item::DepthBuffer => {
            // 0 (the game side's pick), then each listed buffer, round.
            let count = status.depth_buffer_count + 1;
            let step = if press == Press::Left { count - 1 } else { 1 };
            settings.depth_buffer = (settings.depth_buffer.min(count - 1) + step) % count;
        }
        Item::DepthCurve => settings.depth_log = !settings.depth_log,
        Item::DepthRange => {
            let at = DEPTH_RANGES.iter().position(|r| *r == settings.depth_range).unwrap_or(2);
            let step = if press == Press::Left { DEPTH_RANGES.len() - 1 } else { 1 };
            settings.depth_range = DEPTH_RANGES[(at + step) % DEPTH_RANGES.len()];
        }
        Item::DepthNear => {
            let at = DepthNear::ALL.iter().position(|n| *n == settings.depth_near).unwrap_or(0);
            let step = if press == Press::Left { DepthNear::ALL.len() - 1 } else { 1 };
            settings.depth_near = DepthNear::ALL[(at + step) % DepthNear::ALL.len()];
        }
        Item::Freeze => settings.frozen = !settings.frozen,
        Item::CaptureOverlays => settings.capture_overlays = !settings.capture_overlays,
        Item::Record => settings.recording = !settings.recording,
        Item::Panel(panel) => settings.panels = settings.panels.with(panel, !settings.panels.has(panel)),
        Item::SaveRecording => return (press == Press::Switch).then_some(Command::SaveRecording),
        Item::SaveCapture => return (press == Press::Switch).then_some(Command::SaveCapture),
        Item::Page(line) => {
            let id = status.page.rows().get(usize::from(line))?.id;
            let press = match press {
                Press::Left => page::Press::Left,
                Press::Right => page::Press::Right,
                _ => page::Press::Switch,
            };
            return Some(Command::Page { id, press });
        }
    }
    None
}

/// The lines of the game's page that are buttons.
fn buttons(status: &Status) -> Vec<usize> {
    status.page.rows().iter().enumerate().filter(|(_, r)| r.kind() == page::Kind::Button).map(|(line, _)| line).collect()
}

/// Turns the left stick's vertical and horizontal push into presses: one at the push, then
/// repeating while held, like a console menu.
#[derive(Clone, Copy, Debug, Default)]
pub struct StickRepeat {
    /// The direction held and when it next repeats (seconds).
    held: Option<(Press, f64)>,
}

/// Push needed to count (of full tilt), and let-go below which the stick is centred again.
const PUSH: f32 = 0.6;
const CENTRED: f32 = 0.35;
/// First repeat after this long, then every this often (seconds).
const FIRST_REPEAT: f64 = 0.4;
const REPEAT: f64 = 0.12;

impl StickRepeat {
    /// The stick at `x`, `y` (each -1..=1, up positive) at `now` (seconds).
    pub fn step(&mut self, x: f32, y: f32, now: f64) -> Option<Press> {
        let pushed = if y >= PUSH {
            Some(Press::Up)
        } else if y <= -PUSH {
            Some(Press::Down)
        } else if x >= PUSH {
            Some(Press::Right)
        } else if x <= -PUSH {
            Some(Press::Left)
        } else {
            None
        };
        match (self.held, pushed) {
            (Some((held, at)), Some(press)) if held == press => {
                if now >= at {
                    self.held = Some((held, now + REPEAT));
                    return Some(held);
                }
                None
            }
            (_, Some(press)) => {
                self.held = Some((press, now + FIRST_REPEAT));
                Some(press)
            }
            (Some(_), None) if x.abs() < CENTRED && y.abs() < CENTRED => {
                self.held = None;
                None
            }
            _ => None,
        }
    }
}
