//! The player's choices, kept between runs in `%APPDATA%\monaka\settings.txt`: one `game.option=value`
//! line per changed option, `monaka.option=value` for Monaka's own settings (every game's), and
//! `selected=game`. A game's values are kept while it is not on offer (a pack removed, or not
//! readable after an update), for when it comes back.

use crate::games::Values;
use monaka_contract::manifest::MONAKA;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Default, Debug, PartialEq)]
pub struct Settings {
    /// The game shown last.
    pub selected: Option<String>,
    /// Chosen values by game id.
    pub values: BTreeMap<String, Values>,
    /// Monaka's own settings ([`MONAKA`]), the same for every game.
    pub monaka: Values,
    /// Where [`Settings::save`] writes: the file they were loaded from.
    file: Option<PathBuf>,
}

/// The player's settings file; none without `APPDATA`.
fn file() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join(r"monaka\settings.txt"))
}

impl Settings {
    /// The player's settings, from their file.
    pub fn load() -> Settings {
        file().map(Settings::load_from).unwrap_or_default()
    }

    /// The settings kept in `file` (all defaults while it is missing), saved back there.
    pub fn load_from(file: PathBuf) -> Settings {
        let mut settings = std::fs::read_to_string(&file).map(|t| Settings::parse(&t)).unwrap_or_default();
        settings.file = Some(file);
        settings
    }

    pub fn save(&self) -> Result<(), String> {
        let file = self.file.as_ref().ok_or("APPDATA is not set")?;
        if let Some(folder) = file.parent() {
            std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
        }
        std::fs::write(file, self.text()).map_err(|e| format!("{}: {e}", file.display()))
    }

    /// Values for a run of `game`: its own (all defaults when none were saved) and Monaka's own
    /// settings beside them.
    pub fn values(&self, game: &str) -> Values {
        let mut values = self.monaka.clone();
        values.0.extend(self.values.get(game).into_iter().flat_map(|v| v.0.clone()));
        values
    }

    fn parse(text: &str) -> Settings {
        let mut settings = Settings::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else { continue };
            if key.trim() == "selected" {
                settings.selected = Some(value.trim().to_owned());
            } else if let Some(option) = key.trim().strip_prefix(MONAKA.id).and_then(|k| k.strip_prefix('.')) {
                settings.monaka.set(option, value.trim());
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
        for (game, values) in std::iter::once((MONAKA.id, &self.monaka)).chain(self.values.iter().map(|(g, v)| (g.as_str(), v))) {
            for (option, value) in &values.0 {
                lines.push(format!("{game}.{option}={}", value.replace(['\r', '\n'], " ")));
            }
        }
        lines.join("\n") + "\n"
    }
}

#[cfg(test)]
mod tests;
