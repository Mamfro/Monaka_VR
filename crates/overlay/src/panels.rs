//! Panels of their own beside the debug menu, switched on in its Panels tab: each an egui page
//! ([`layout`]) that the headset shows as a layer of its own ([`headset_page`], placed beside the
//! menu) and the 2D window as a window of its own. What they show comes from the viewer each time
//! they are drawn ([`Content`]).

use crate::console_view::{self, ConsoleView};
use crate::fake_vr::{self, FakeVr};
use crate::menu::panel::{BACKGROUND, DIM, TEXT};
use crate::stats::{self, StatsView};
use egui::{CornerRadius, Frame, Margin, RichText};

#[cfg(test)]
mod tests;

/// A panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum PanelId {
    /// The run's timing: the headset's or window's frames, the game's, the pairs between them,
    /// and the poses sent ([`crate::stats`]).
    Stats = 0,
    /// The game's probe console: what it has in, and what was asked of it and answered
    /// ([`crate::console_view`]).
    Console = 1,
    /// The 2D window's fake head and controllers ([`crate::fake_vr`]).
    FakeVr = 2,
}

impl PanelId {
    pub const ALL: [PanelId; 3] = [PanelId::Stats, PanelId::Console, PanelId::FakeVr];

    pub const fn label(self) -> &'static str {
        match self {
            PanelId::Stats => "Stats",
            PanelId::Console => "Console",
            PanelId::FakeVr => "Fake VR",
        }
    }

    const fn bit(self) -> u8 {
        1 << self as u8
    }

    /// Its page's width in the 2D window (points): fixed, so the window does not grow and shrink
    /// with its numbers.
    pub const fn window_width(self) -> f32 {
        match self {
            PanelId::Stats => 400.0,
            PanelId::Console => 560.0,
            PanelId::FakeVr => 520.0,
        }
    }
}

/// A set of panels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Panels(u8);

impl Panels {
    pub const NONE: Self = Self(0);

    pub const fn has(self, panel: PanelId) -> bool {
        self.0 & panel.bit() != 0
    }

    pub const fn with(self, panel: PanelId, on: bool) -> Self {
        if on { Self(self.0 | panel.bit()) } else { Self(self.0 & !panel.bit()) }
    }

    /// The panels in the set, in [`PanelId::ALL`]'s order.
    pub fn iter(self) -> impl Iterator<Item = PanelId> {
        PanelId::ALL.into_iter().filter(move |p| self.has(*p))
    }
}

/// What the panels show this time.
pub struct Content<'a> {
    pub stats: &'a StatsView,
    pub console: &'a ConsoleView,
    pub fake_vr: &'a FakeVr,
}

/// What a panel asks of the viewer: a console command (typed or a button's), or the fake head and
/// controllers changed.
#[derive(Clone, Debug, PartialEq)]
pub enum Asked {
    Console(String),
    FakeVr(FakeVr),
}

/// Panel `id`'s page, without a background of its own (the 2D window's windows have theirs); the
/// console command typed or of a button pressed on it, or the fake head and controllers changed.
pub fn layout(ui: &mut egui::Ui, id: PanelId, content: &Content<'_>) -> Option<Asked> {
    match id {
        PanelId::Stats => {
            stats::layout(ui, content.stats);
            None
        }
        PanelId::Console => console_view::layout(ui, content.console).map(Asked::Console),
        PanelId::FakeVr => fake_vr::layout(ui, content.fake_vr).map(Asked::FakeVr),
    }
}

/// Space inside a headset panel's edge (points).
const EDGE: i8 = 16;

/// Panel `id`'s window in the 2D window, in the debug menu's colours and corners so the two
/// match, dragged by anywhere; its page is [`window_page`].
pub fn window(id: PanelId) -> egui::Window<'static> {
    let frame = Frame::new().fill(BACKGROUND).corner_radius(CornerRadius::same(14)).inner_margin(Margin::same(EDGE));
    let width = id.window_width();
    egui::Window::new(id.label()).id(egui::Id::new(("panel", id.label()))).title_bar(false).frame(frame).resizable(false).min_width(width).max_width(width)
}

/// Panel `id` in its 2D window: its title as the headset shows it, a close button (clears `open`),
/// and its page; the console command of a button pressed on it.
pub fn window_page(ui: &mut egui::Ui, id: PanelId, content: &Content<'_>, open: &mut bool) -> Option<Asked> {
    ui.horizontal(|ui| {
        ui.label(RichText::new(id.label()).size(20.0).color(TEXT).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add(egui::Button::new(RichText::new("x").size(16.0).color(DIM)).frame(false)).on_hover_text("Close (the Panels tab opens it again)").clicked() {
                *open = false;
            }
        });
    });
    ui.add_space(4.0);
    layout(ui, id, content)
}

/// The keys, at the foot of each panel on the debug ring.
pub const KEYS: &str = "Point and pull the trigger.  Right stick: scroll.  LB/RB: panels.  B or hold Back: close.";

/// Panel `id` as a page on the debug ring ([`crate::ring_page`]): its title over its page, which
/// scrolls when it holds more than shows, outlined while in `front`; used with the pointer as the
/// 2D window's is with the mouse. What was asked on it.
pub fn ring_page(ui: &mut egui::Ui, id: PanelId, content: &Content<'_>, front: bool) -> Option<Asked> {
    crate::ring_page::style(ui);
    crate::ring_page::framed(ui, id.label(), front, KEYS, |ui| egui::ScrollArea::vertical().id_salt(id.label()).auto_shrink([false, false]).show(ui, |ui| layout(ui, id, content)).inner)
}
