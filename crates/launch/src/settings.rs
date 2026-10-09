//! The player's choices, kept between runs in `%APPDATA%\monaka\settings.txt`: one `game.option=value`
//! line per changed option, and `selected=game`. A game's values are kept while it is not on offer
//! (a pack removed, or not readable after an update), for when it comes back.

use crate::games::Values;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Default, Debug, PartialEq)]
pub struct Settings {
    /// The game shown last.
    pub selected: Option<String>,
    /// Chosen values by game id.
    pub values: BTreeMap<String, Values>,
}

fn file() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join(r"monaka\settings.txt"))
}

impl Settings {
    pub fn load() -> Settings {
        file().and_then(|f| std::fs::read_to_string(f).ok()).map(|t| Settings::parse(&t)).unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let file = file().ok_or("APPDATA is not set")?;
        if let Some(folder) = file.parent() {
            std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
        }
        std::fs::write(&file, self.text()).map_err(|e| format!("{}: {e}", file.display()))
    }

    /// Values for `game`, empty (all defaults) when none were saved.
    pub fn values(&self, game: &str) -> Values {
        self.values.get(game).cloned().unwrap_or_default()
    }

    fn parse(text: &str) -> Settings {
        let mut settings = Settings::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else { continue };
            if key.trim() == "selected" {
                settings.selected = Some(value.trim().to_owned());
            } else if let Some((game, option)) = key.trim().split_once('.')
                && !game.is_empty()
                && game.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            {
                settings.values.entry(game.to_owned()).or_default().set(option, value.trim());
            }
        }
        settings
    }

    fn text(&self) -> String {
        let mut lines = Vec::new();
        if let Some(selected) = &self.selected {
            lines.push(format!("selected={selected}"));
        }
        for (game, values) in &self.values {
            for (option, value) in &values.0 {
                lines.push(format!("{game}.{option}={}", value.replace(['\r', '\n'], " ")));
            }
        }
        lines.join("\n") + "\n"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let text = "selected=dl2\ndl2.mode=framegen\ndl1.extra=probe_rig=1 hand_hold=0,0.01,0\na pack.x=1\nremoved_pack.x=1\njunk\n";
        let settings = Settings::parse(text);
        assert_eq!(settings.selected.as_deref(), Some("dl2"));
        assert_eq!(settings.values("dl1").0["extra"], "probe_rig=1 hand_hold=0,0.01,0");
        assert!(!settings.values.contains_key("a pack"));
        assert_eq!(settings.values("removed_pack").0["x"], "1", "kept for when the pack comes back");
        assert_eq!(Settings::parse(&settings.text()), settings);
    }
}
