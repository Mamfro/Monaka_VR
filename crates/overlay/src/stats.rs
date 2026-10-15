//! The Stats panel ([`crate::panels::PanelId::Stats`]): what shows the pairs (the headset, or the
//! 2D window) and how steadily, the game's frames, the pairs between them, which game frame each
//! eye came from, what Monaka's own work costs the game, and the poses the viewer sends back. The
//! viewer measures; this lays it out.

use crate::menu::panel::{ACCENT, DIM, ON, TEXT};
use egui::{Color32, FontId, Pos2, RichText, Sense, Stroke, Vec2};
use monaka_contract::debug_draw::{COSTS, Cost, CostTime};

#[cfg(test)]
mod tests;

const WARN: Color32 = Color32::from_rgb(240, 110, 90);
/// A graph's line, and its background.
const LINE: Color32 = Color32::from_rgb(120, 190, 255);
const GRAPH_BACKGROUND: Color32 = Color32::from_rgb(30, 33, 42);
/// A graph's size (points).
const GRAPH: Vec2 = Vec2::new(236.0, 46.0);

/// One measured stretch of the run, as the panel shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StatsView {
    /// What shows the pairs: "Headset" or "Window".
    pub display: &'static str,
    /// Its frames a second, and their time: the mean and the longest (ms).
    pub display_fps: f32,
    pub display_ms: f32,
    pub worst_display_ms: f32,
    /// Its latest frames' times (ms), oldest first, for the graph.
    pub display_times: Vec<f32>,
    /// New pairs a second from the game.
    pub pairs_per_second: f32,
    /// How old a pair was when first shown: the mean and the oldest (ms; the game's publish time
    /// is the system tick, so each is good to about 16 ms).
    pub pair_age_ms: f32,
    pub worst_pair_age_ms: f32,
    /// Shown frames a second that had no new pair (the last one shown again).
    pub repeated_per_second: f32,
    /// Pairs a second the game published that were never shown (a newer one came first).
    pub skipped_per_second: f32,
    /// The pairs' eye images (pixels).
    pub eye_size: [u32; 2],
    /// Which game frame each eye came from, where the game names them.
    pub eyes: Option<EyeAges>,
    /// The game's side, while its producer says it.
    pub game: Option<GameView>,
    pub poses: PoseFlow,
}

/// The game frames a stretch's pairs were drawn in, each eye against the pair's newer one.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EyeAges {
    /// Each eye's share of the pairs it was the newer eye in (or both were from one frame), left
    /// first.
    pub newest: [f32; 2],
    /// Each eye's mean age behind the pair's newer eye (frames).
    pub age: [f32; 2],
    /// The share of the pairs whose eyes are from one frame.
    pub together: f32,
}

/// The game's frames, as its producer measures them at its presents.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GameView {
    /// Its frames a second and their time (mean, longest; ms), where its producer measures them
    /// (a game presenting through Monaka's own driver, or telling it from its own loop, does).
    pub frames: Option<[f32; 3]>,
    /// Its latest frames' times (ms), oldest first, for the graph.
    pub times: Vec<f32>,
    /// What Monaka's own work cost the game a frame, by [`Cost`] (all zero: not measured yet).
    pub costs: [CostTime; COSTS],
    /// Its swapchain (0: unknown).
    pub size: [u32; 2],
    /// The run's rendering settings, `Label: value` a line.
    pub settings: String,
}

/// The poses the viewer sends to the game, a second, and what is tracked.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PoseFlow {
    pub head: f32,
    pub hands: f32,
    pub fingers: f32,
    pub gaze: f32,
    pub head_tracked: bool,
    /// Left first.
    pub controllers: [bool; 2],
    pub fingers_tracked: [bool; 2],
}

/// A rate and a time: "71.9 / s   13.9 ms (longest 22.1)".
pub fn rate_and_time(per_second: f32, mean_ms: f32, worst_ms: f32) -> String {
    format!("{per_second:.1} / s   {mean_ms:.1} ms (longest {worst_ms:.1})")
}

/// One line of the panel.
enum Line<'a> {
    Heading(&'a str),
    Value(&'a str, String, Color32),
    /// Frame times (ms, oldest first) against a frame's budget (ms).
    Graph(&'a str, &'a [f32], f32),
    Note(&'a str),
}

/// The panel's lines: each section's heading, then its values.
fn lines(view: &StatsView) -> Vec<Line<'_>> {
    let worse_than = |value: f32, limit: f32| if value > limit { WARN } else { TEXT };
    // A frame much longer than the mean is a hitch.
    let hitch = |worst: f32, mean: f32| if worst > 2.0 * mean.max(1.0) { WARN } else { TEXT };
    // The display's frame is the budget both graphs are held against.
    let budget = view.display_ms;
    let mut lines = vec![
        Line::Heading(view.display),
        Line::Value("Frames", rate_and_time(view.display_fps, view.display_ms, view.worst_display_ms), hitch(view.worst_display_ms, view.display_ms)),
        Line::Graph("Frame times", &view.display_times, budget),
        Line::Heading("Pairs"),
        Line::Value("New", format!("{:.1} / s   {}x{} each eye", view.pairs_per_second, view.eye_size[0], view.eye_size[1]), TEXT),
        Line::Value("Age when shown", format!("~{:.0} ms (oldest ~{:.0})", view.pair_age_ms, view.worst_pair_age_ms), worse_than(view.pair_age_ms, 50.0)),
        Line::Value("Shown again", format!("{:.1} / s", view.repeated_per_second), TEXT),
        Line::Value("Never shown", format!("{:.1} / s", view.skipped_per_second), worse_than(view.skipped_per_second, 0.5)),
    ];
    match view.eyes {
        Some(eyes) => {
            for (side, label) in ["Left eye", "Right eye"].into_iter().enumerate() {
                lines.push(Line::Value(label, format!("newer {:.0}%   {:.2} frames behind", eyes.newest[side] * 100.0, eyes.age[side]), TEXT));
            }
            lines.push(Line::Value("Both one frame", format!("{:.0}%", eyes.together * 100.0), TEXT));
        }
        None => lines.push(Line::Value("Eyes", "this game's pairs name no frames".to_owned(), DIM)),
    }
    lines.push(Line::Heading("Game"));
    match &view.game {
        Some(game) => {
            match game.frames {
                Some([fps, mean, worst]) => {
                    lines.push(Line::Value("Frames", rate_and_time(fps, mean, worst), hitch(worst, mean)));
                    lines.push(Line::Graph("Frame times", &game.times, budget));
                }
                None => lines.push(Line::Value("Frames", "not measured in this game".to_owned(), DIM)),
            }
            if game.size != [0, 0] {
                lines.push(Line::Value("Swapchain", format!("{}x{}", game.size[0], game.size[1]), TEXT));
            }
            lines.extend(game.settings.lines().filter_map(|l| l.split_once(": ")).map(|(label, value)| Line::Value(label, value.to_owned(), TEXT)));
            lines.push(Line::Heading("Monaka in the game (CPU a frame)"));
            if game.costs.iter().all(|c| c.mean_ms == 0.0 && c.worst_ms == 0.0) {
                lines.push(Line::Note("Measured from the next quarter second"));
            } else {
                // Monaka's work against the game's frame: a quarter of it is a lot.
                let frame = game.frames.map_or(budget, |[_, mean, _]| mean);
                for cost in Cost::ALL {
                    let c = game.costs[cost as usize];
                    lines.push(Line::Value(cost.label(), format!("{:.2} ms (longest {:.2})", c.mean_ms, c.worst_ms), worse_than(c.mean_ms, frame * 0.25)));
                }
            }
        }
        None => lines.push(Line::Note("The game's side says nothing yet")),
    }
    let tracked = |on: bool| if on { ("tracked", ON) } else { ("not tracked", DIM) };
    let pair = |[left, right]: [bool; 2]| match (left, right) {
        (true, true) => ("both tracked", ON),
        (false, false) => ("not tracked", DIM),
        (true, false) => ("left tracked", TEXT),
        (false, true) => ("right tracked", TEXT),
    };
    let poses = view.poses;
    let (head, head_colour) = tracked(poses.head_tracked);
    let (controllers, controllers_colour) = pair(poses.controllers);
    let (fingers, fingers_colour) = pair(poses.fingers_tracked);
    lines.extend([
        Line::Heading("Poses sent"),
        Line::Value("Head", format!("{:.0} / s   {head}", poses.head), head_colour),
        Line::Value("Controllers", format!("{:.0} / s   {controllers}", poses.hands), controllers_colour),
        Line::Value("Fingers", format!("{:.0} / s   {fingers}", poses.fingers), fingers_colour),
        Line::Value("Eye gaze", format!("{:.0} / s", poses.gaze), if poses.gaze > 0.0 { TEXT } else { DIM }),
    ]);
    lines
}

/// The Stats panel's page: one grid, so every section's values line up.
pub fn layout(ui: &mut egui::Ui, view: &StatsView) {
    egui::Grid::new("stats").num_columns(2).spacing([16.0, 3.0]).show(ui, |ui| {
        for line in lines(view) {
            match line {
                Line::Heading(text) => {
                    // Painted over both columns: a long heading does not widen the labels'.
                    let font = FontId::proportional(15.0);
                    let height = ui.fonts_mut(|f| f.row_height(&font));
                    let (rect, _) = ui.allocate_exact_size(Vec2::new(1.0, height), Sense::hover());
                    ui.painter().text(rect.left_center(), egui::Align2::LEFT_CENTER, text, font, ACCENT);
                    ui.end_row();
                }
                Line::Value(label, value, colour) => {
                    ui.label(RichText::new(label).size(13.0).color(DIM));
                    ui.label(RichText::new(value).size(13.0).color(colour));
                    ui.end_row();
                }
                Line::Graph(label, times, budget) => {
                    ui.label(RichText::new(label).size(13.0).color(DIM));
                    graph(ui, times, budget);
                    ui.end_row();
                }
                Line::Note(text) => {
                    ui.label(RichText::new(text).size(13.0).color(DIM));
                    ui.end_row();
                }
            }
        }
    });
}

/// A graph's top (ms): twice the budget, or the longest frame if longer, at most 100.
pub fn graph_top(times: &[f32], budget: f32) -> f32 {
    times.iter().copied().fold(2.0 * budget.max(1.0), f32::max).min(100.0)
}

/// How high a frame of `ms` reaches in a graph topped at `top` ms: 0 the bottom, 1 the top.
pub fn graph_height(ms: f32, top: f32) -> f32 {
    (ms / top.max(1.0)).clamp(0.0, 1.0)
}

/// Frame times (oldest left) as a line, against the budget's line; frames past one and a half
/// budgets marked.
fn graph(ui: &mut egui::Ui, times: &[f32], budget: f32) {
    let (rect, _) = ui.allocate_exact_size(GRAPH, Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, GRAPH_BACKGROUND);
    let top = graph_top(times, budget);
    let y = |ms: f32| rect.bottom() - graph_height(ms, top) * rect.height();
    let small = FontId::proportional(10.0);
    if budget > 0.0 {
        painter.hline(rect.x_range(), y(budget), Stroke::new(1.0, DIM));
        painter.text(Pos2::new(rect.right() - 3.0, y(budget) - 1.0), egui::Align2::RIGHT_BOTTOM, format!("{budget:.1}"), small.clone(), DIM);
    }
    painter.text(rect.left_top() + Vec2::new(3.0, 1.0), egui::Align2::LEFT_TOP, format!("{top:.0} ms"), small, DIM);
    if times.len() < 2 {
        return;
    }
    let step = rect.width() / (times.len() - 1) as f32;
    let points: Vec<Pos2> = times.iter().enumerate().map(|(i, ms)| Pos2::new(rect.left() + i as f32 * step, y(*ms))).collect();
    painter.add(egui::Shape::line(points.clone(), Stroke::new(1.2, LINE)));
    for (point, ms) in points.iter().zip(times) {
        if budget > 0.0 && *ms > 1.5 * budget {
            painter.circle_filled(*point, 1.8, WARN);
        }
    }
}
