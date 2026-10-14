//! The headset's Monaka menu (Start held): the options the viewer can change while VR runs
//! ([`LIVE`]), one per line, moved through like the debug menu (the left stick up and down, left
//! and right for a value, A to switch, B to close). Each line holds its option's value as the
//! launcher writes it; the viewer acts on a change and tells the launcher, which saves it.
//! [`panel`] draws it.

use crate::menu::Press;
use monaka_core::manifest::{Kind, LIVE, Opt, number_text};

pub mod panel;

#[cfg(test)]
mod tests;

/// One option and its value (`id=value` text, as the options file has it).
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub opt: &'static Opt,
    pub value: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SettingsMenu {
    pub lines: Vec<Line>,
    pub selected: usize,
    pub open: bool,
}

impl SettingsMenu {
    /// The lines `live` names (`id=value,id=value`, the viewer's `--live`), in its order; ids that
    /// are not [`LIVE`] options are left out.
    pub fn parse(live: &str) -> Self {
        let lines = live
            .split(',')
            .filter_map(|pair| pair.split_once('='))
            .filter_map(|(id, value)| LIVE.iter().find(|o| o.id == id).map(|opt| Line { opt, value: value.to_owned() }))
            .collect();
        Self { lines, ..Self::default() }
    }

    /// Acts on `press`; the line whose value changed, if one did.
    pub fn press(&mut self, press: Press) -> Option<&Line> {
        let count = self.lines.len();
        if count == 0 {
            self.open &= press != Press::Close;
            return None;
        }
        self.selected = self.selected.min(count - 1);
        let changed = match press {
            Press::Up => {
                self.selected = (self.selected + count - 1) % count;
                None
            }
            Press::Down => {
                self.selected = (self.selected + 1) % count;
                None
            }
            Press::Close => {
                self.open = false;
                None
            }
            Press::PreviousTab | Press::NextTab => None,
            Press::Left | Press::Right | Press::Switch => {
                let line = &mut self.lines[self.selected];
                let next = stepped(line.opt, &line.value, press);
                (next != line.value).then(|| line.value = next)
            }
        };
        changed.map(|()| &self.lines[self.selected])
    }
}

/// `value` of `opt` after `press`: a switch flips (A too), a choice moves round the window's
/// choices (A forward), a number moves by its step within its bounds.
pub fn stepped(opt: &Opt, value: &str, press: Press) -> String {
    let back = press == Press::Left;
    match opt.kind {
        Kind::Switch { .. } => (if value == "1" { "0" } else { "1" }).to_owned(),
        Kind::Choice { choices, window, .. } => {
            let offered = &choices[..window.min(choices.len())];
            let at = offered.iter().position(|(id, _)| *id == value).unwrap_or(0);
            let step = if back { offered.len() - 1 } else { 1 };
            offered[(at + step) % offered.len()].0.to_owned()
        }
        Kind::Number { min, max, step, .. } if press != Press::Switch => {
            let now = value.parse::<f64>().unwrap_or(min);
            number_text((now + if back { -step } else { step }).clamp(min, max))
        }
        Kind::Number { .. } | Kind::Text | Kind::Size => value.to_owned(),
    }
}

/// How a line shows its value: On or Off, a choice's label, a number; ones that step show arrows.
pub fn shown(opt: &Opt, value: &str) -> String {
    match opt.kind {
        Kind::Switch { .. } => (if value == "1" { "On" } else { "Off" }).to_owned(),
        Kind::Choice { choices, .. } => format!("<  {}  >", choices.iter().find(|(id, _)| *id == value).map_or(value, |(_, label)| label)),
        Kind::Number { .. } => format!("<  {value}  >"),
        Kind::Text | Kind::Size => value.to_owned(),
    }
}
