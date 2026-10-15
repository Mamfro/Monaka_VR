//! Monaka's features: what it does for the player in a game, each named for its mechanism (stereo,
//! the look, the hand rig, the finger rig, melee, ...). A game has a feature when its adapter
//! provides what one of the feature's methods needs (`monaka_producer::game`); the set is read from
//! the adapter's code at compile time, never declared beside it.
//!
//! Some providers serve several features and are not features themselves: what each hand holds,
//! the game's actions, the situations it reports (`crate::situation`).
//!
//! Each shared option belongs to one feature (`Opt::owner`): only the component that provides that
//! feature may offer it, so a game's options say what its code honours ([`fault`], checked at
//! compile time while the game's options are built). The `aim` choices follow the features too:
//! motion controls need the hand rig, look and pointing aim need the look ([`aim_choice_allowed`]).
//!
//! At run time the set narrows to what the run has ([`Available`]): a feature the player's
//! settings leave off, or one this build cannot carry (a fact its table lacks), with the reason,
//! for the stop's summary.

use crate::manifest::{Kind, Opt, SHARED};
use crate::situation::Situation;

/// Something Monaka does for the player in a game, named for its mechanism.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Feature {
    /// Two eye views, each drawn from the headset's pose.
    Stereo,
    /// The character's look steered by the head (or a pointing controller): it looks, aims and
    /// attacks where that points.
    Look,
    /// The game's arms posed on the controllers.
    HandRig,
    /// The game's finger bones posed from the tracked fingers.
    FingerRig,
    /// Melee by swinging: a swing attacks, or the hit lands where the real blade does.
    Melee,
    /// The VR controllers feed the game's gamepad.
    Gamepad,
    /// The character follows the player's real steps.
    RoomScale,
    /// The HUD captured as a layer and placed at a depth in each eye.
    HudLayer,
    /// The HUD's pieces cut out into panels on the hands.
    HudPanels,
    /// The game's temporal passes (upscaling, its camera history) kept per eye.
    PerEyeHistory,
    /// Reloading by bringing the hand to the gun and away.
    Reload,
    /// A lock's pick and screwdriver turned by twisting the wrists.
    Lockpick,
    /// A bow drawn by pulling the string hand back.
    Bow,
    /// A throw with the hand's own speed and way.
    Throw,
    /// Crouching by lowering the head.
    Duck,
    /// Each eye's depth given to the headset, so its own reprojection of a late frame corrects
    /// the head moving as well as turning.
    DepthLayer,
}

impl Feature {
    /// Every feature, in a fixed order.
    pub const ALL: [Feature; 16] = [
        Feature::Stereo,
        Feature::Look,
        Feature::HandRig,
        Feature::FingerRig,
        Feature::Melee,
        Feature::Gamepad,
        Feature::RoomScale,
        Feature::HudLayer,
        Feature::HudPanels,
        Feature::PerEyeHistory,
        Feature::Reload,
        Feature::Lockpick,
        Feature::Bow,
        Feature::Throw,
        Feature::Duck,
        Feature::DepthLayer,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Feature::Stereo => "stereo",
            Feature::Look => "look",
            Feature::HandRig => "hand rig",
            Feature::FingerRig => "finger rig",
            Feature::Melee => "melee",
            Feature::Gamepad => "gamepad",
            Feature::RoomScale => "room-scale",
            Feature::HudLayer => "HUD layer",
            Feature::HudPanels => "HUD panels",
            Feature::PerEyeHistory => "per-eye history",
            Feature::Reload => "reload",
            Feature::Lockpick => "lockpick",
            Feature::Bow => "bow",
            Feature::Throw => "throw",
            Feature::Duck => "duck",
            Feature::DepthLayer => "depth layer",
        }
    }

    pub const fn same(self, other: Feature) -> bool {
        self as u32 == other as u32
    }
}

/// A set of features. It is built in `const fn` (a game's set is read from its adapter at compile
/// time), so it keeps one bit per feature inside; nothing outside sees them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Features(u32);

impl Features {
    pub const NONE: Self = Self(0);

    pub const fn of(feature: Feature) -> Self {
        Self(1 << feature as u32)
    }

    pub const fn with(self, feature: Feature) -> Self {
        Self(self.0 | Self::of(feature).0)
    }

    pub const fn with_all(self, features: &[Feature]) -> Self {
        let mut set = self;
        let mut i = 0;
        while i < features.len() {
            set = set.with(features[i]);
            i += 1;
        }
        set
    }

    pub const fn has(self, feature: Feature) -> bool {
        self.0 & Self::of(feature).0 != 0
    }

    pub const fn without(self, feature: Feature) -> Self {
        Self(self.0 & !Self::of(feature).0)
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Every feature in the set, in [`Feature::ALL`]'s order.
    pub fn iter(self) -> impl Iterator<Item = Feature> {
        Feature::ALL.into_iter().filter(move |f| self.has(*f))
    }
}

pub const fn same_str(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// Whether a game with `has` can carry the `aim` choice `choice`: motion controls (`hand_rig`)
/// need the hand rig; look (`head`) and pointing (`controller`) aim need the look; `off` always.
pub const fn aim_choice_allowed(choice: &str, has: Features) -> bool {
    if same_str(choice, "hand_rig") {
        has.has(Feature::HandRig)
    } else if same_str(choice, "head") || same_str(choice, "controller") {
        has.has(Feature::Look)
    } else {
        same_str(choice, "off")
    }
}

/// A message built in `const fn` (a compile-time check's, which cannot format): pieces appended
/// into a fixed buffer, cut at its end.
#[derive(Clone, Copy)]
pub struct Message {
    bytes: [u8; 240],
    len: usize,
}

impl Message {
    pub const fn new() -> Self {
        Self { bytes: [0; 240], len: 0 }
    }

    /// `piece` added, as many whole characters of it as fit.
    pub const fn push(mut self, piece: &str) -> Self {
        let piece = piece.as_bytes();
        let mut i = 0;
        while i < piece.len() {
            // A character's first byte says how many bytes it takes.
            let lead = piece[i];
            let width = if lead < 0x80 { 1 } else if lead >= 0xf0 { 4 } else if lead >= 0xe0 { 3 } else { 2 };
            if self.len + width > self.bytes.len() {
                break;
            }
            let mut k = 0;
            while k < width {
                self.bytes[self.len] = piece[i + k];
                self.len += 1;
                k += 1;
            }
            i += width;
        }
        self
    }

    pub const fn text(&self) -> &str {
        match core::str::from_utf8(self.bytes.split_at(self.len).0) {
            Ok(text) => text,
            Err(_) => "(a message cut inside a character)",
        }
    }
}

impl Default for Message {
    fn default() -> Self {
        Self::new()
    }
}

/// What is wrong, if anything, with the component named `component`, providing `provides`, offering
/// `options` in a game with `has`, as one line naming the component and the option. Run while a
/// game's options are built at compile time, so a game that breaks a rule does not build:
///
/// - a shared option is offered only by a component providing its feature;
/// - a game's own option never takes a shared option's id, and a shared option adjusted for a game
///   (its default, range, help, tab, condition) keeps the shared one's kind;
/// - every `aim` choice is one the game's features carry.
pub const fn fault(component: &str, provides: &[Feature], options: &[Opt], has: Features) -> Option<Message> {
    let mut i = 0;
    while i < options.len() {
        let opt = &options[i];
        let start = Message::new().push("component `").push(component).push("` offers `").push(opt.id).push("`: ");
        if let Some(owner) = opt.owner {
            let mut provided = false;
            let mut p = 0;
            while p < provides.len() {
                provided |= provides[p].same(owner);
                p += 1;
            }
            if !provided {
                return Some(start.push("only a component providing the ").push(owner.name()).push(" may offer it"));
            }
        }
        let mut s = 0;
        while s < SHARED.len() {
            if same_str(SHARED[s].id, opt.id) {
                if opt.owner.is_none() {
                    return Some(start.push("that is a shared option's id: offer the shared constant (`manifest::SHARED`)"));
                }
                if !same_kind(&SHARED[s].kind, &opt.kind) {
                    return Some(start.push("a shared option adjusted for a game keeps its kind"));
                }
            }
            s += 1;
        }
        if same_str(opt.id, "aim")
            && let Kind::Choice { choices, .. } = opt.kind
        {
            let mut c = 0;
            while c < choices.len() {
                if !aim_choice_allowed(choices[c].0, has) {
                    return Some(start.push("its choice `").push(choices[c].0).push("` needs a feature the game does not have (hand_rig: the hand rig; head, controller: the look)"));
                }
                c += 1;
            }
        }
        i += 1;
    }
    None
}

const fn same_kind(a: &Kind, b: &Kind) -> bool {
    matches!(
        (a, b),
        (Kind::Switch { .. }, Kind::Switch { .. })
            | (Kind::Choice { .. }, Kind::Choice { .. })
            | (Kind::Number { .. }, Kind::Number { .. })
            | (Kind::Text, Kind::Text)
            | (Kind::Size, Kind::Size)
    )
}

/// The id offered twice among `options`, if any.
pub const fn repeated<'a>(options: &[&'a [Opt]]) -> Option<&'a str> {
    let mut a = 0;
    while a < options.len() {
        let mut i = 0;
        while i < options[a].len() {
            let mut b = a;
            while b < options.len() {
                let mut j = if b == a { i + 1 } else { 0 };
                while j < options[b].len() {
                    if same_str(options[a][i].id, options[b][j].id) {
                        return Some(options[a][i].id);
                    }
                    j += 1;
                }
                b += 1;
            }
            i += 1;
        }
        a += 1;
    }
    None
}

/// Situations a feature works better with reported, though it runs without (the look without a
/// report of another camera taking the view turns the view twice through a cutscene): named in the
/// summary.
pub const WANTS: &[(Feature, &[Situation])] = &[
    (Feature::Look, &[Situation::CameraTaken]),
    (Feature::Stereo, &[Situation::Video, Situation::WorldHidden]),
    (Feature::HandRig, &[Situation::CameraTaken]),
];

/// A run's features: those the adapter provides, less those the player's settings leave off and
/// those this build lost; and the situations it reports.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Available {
    provided: Features,
    reports: Vec<Situation>,
    off: Vec<(Feature, String)>,
    lost: Vec<(Feature, String)>,
}

impl Available {
    pub fn new(provided: Features, reports: &[Situation]) -> Self {
        Self { provided, reports: reports.to_vec(), off: Vec::new(), lost: Vec::new() }
    }

    /// The player's settings leave `feature` off for the run.
    pub fn off(&mut self, feature: Feature, why: impl Into<String>) {
        self.off.push((feature, why.into()));
    }

    /// This build cannot carry `feature`, because `why` (a fact its table lacks).
    pub fn lose(&mut self, feature: Feature, why: impl Into<String>) {
        self.lost.push((feature, why.into()));
    }

    /// The features the run has.
    pub fn now(&self) -> Features {
        self.off.iter().chain(&self.lost).fold(self.provided, |has, (gone, _)| has.without(*gone))
    }

    pub fn has(&self, feature: Feature) -> bool {
        self.now().has(feature)
    }

    /// One line per feature the adapter provides: on, off by the settings, or lost and why; then
    /// the situations a feature that is on runs without.
    pub fn report(&self) -> Vec<String> {
        let has = self.now();
        let mut lines = Vec::new();
        for feature in self.provided.iter() {
            let why = |list: &[(Feature, String)]| list.iter().filter(|(f, _)| *f == feature).map(|(_, why)| why.as_str()).collect::<Vec<_>>().join("; ");
            let state = if has.has(feature) {
                "on".to_owned()
            } else if self.lost.iter().any(|(f, _)| *f == feature) {
                format!("lost ({})", why(&self.lost))
            } else {
                format!("off ({})", why(&self.off))
            };
            lines.push(format!("{}: {state}", feature.name()));
        }
        for (feature, wanted) in WANTS {
            let short: Vec<&str> = wanted.iter().filter(|w| !self.reports.contains(w)).map(|w| w.label()).collect();
            if has.has(*feature) && !short.is_empty() {
                lines.push(format!("{}: runs without a report that {}", feature.name(), short.join(", or that ")));
            }
        }
        lines
    }
}

#[cfg(test)]
mod tests;
