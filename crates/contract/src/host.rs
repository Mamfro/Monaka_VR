//! What a host (this Monaka's launcher and viewer) can do for a pack beyond what the contract's
//! major version guarantees, by name. A pack names in its text the host features it *requires*
//! (the launcher refuses it without them, naming each) and those it *uses* (it runs without them,
//! with less). The launcher writes the features it has into the run folder ([`FEATURES_FILE`], one
//! name per line), so a producer can skip work no one would read.
//!
//! A name is never reused or removed within a major version; a host only gains names. A name this
//! host does not know is simply one it does not have.

/// The run folder's list of the host's features, one name per line.
pub const FEATURES_FILE: &str = "host-features.txt";

/// Each eye's depth beside its image, in the channel's `depth` block, given to the headset.
pub const DEPTH: &str = "depth";
/// HUD pieces placed on the hands from the `panels` block.
pub const PANELS: &str = "panels";
/// The tracked fingers in the `fingers` block.
pub const FINGERS: &str = "fingers";
/// Where a hand's reticle points, from the `reticles` block.
pub const RETICLES: &str = "reticles";
/// The window's Controls tab: options on it and the game's how-tos.
pub const CONTROLS_TAB: &str = "controls_tab";
/// A game the launcher joins as it starts (`launch at_start`).
pub const LAUNCH_AT_START: &str = "launch_at_start";

/// Every feature this host has.
pub const HOST: &[&str] = &[DEPTH, PANELS, FINGERS, RETICLES, CONTROLS_TAB, LAUNCH_AT_START];

/// Whether this host has feature `name`.
pub fn has(name: &str) -> bool {
    HOST.contains(&name)
}

/// Those of `names` this host does not have, in order.
pub fn missing<'a>(names: &[&'a str]) -> Vec<&'a str> {
    names.iter().copied().filter(|name| !has(name)).collect()
}

/// [`FEATURES_FILE`]'s text for this host.
pub fn features_text() -> String {
    HOST.join("\n")
}

/// The features a [`FEATURES_FILE`]'s text names.
pub fn parse_features(text: &str) -> Vec<String> {
    text.lines().map(str::trim).filter(|line| !line.is_empty()).map(str::to_owned).collect()
}

#[cfg(test)]
mod tests;
