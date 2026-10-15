//! What the game is doing now, as the features that must react to it ask: is another camera showing
//! the view, is a video playing, is the character climbing or fighting? Each game detects these its
//! own way (a getter's answer, an animation state, a pointer being set) and reports them here; the
//! shared features read them, so head aim, the publisher, the arms and the gestures react the same
//! way in every game.
//!
//! A report holds for a freshness window ([`Situations::report`]): a game renews it while it lasts,
//! and one no longer renewed (the hook stopped being called: a load, a menu the game draws without
//! its frame) expires to "not known" rather than staying stuck on. A situation never reported is
//! "not known" too, which a feature can tell apart from "reported not happening" ([`Situations::state`])
//! when it has a fallback of its own (the arms' wrist-height guess at climbing).
//!
//! Times are milliseconds on one steady clock (the producer's: `monaka_producer::situation`).

/// One thing the game can be doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Situation {
    /// A camera other than the player's own shows the view: a cutscene, a takedown, a scripted look.
    /// The player's look angles do not reach the view meanwhile.
    CameraTaken,
    /// A full-screen video plays (no world drawn behind it).
    Video,
    /// A menu or the pause screen is up over the game.
    Menu,
    /// The world is not being drawn: loading, a black screen, a menu drawn alone.
    WorldHidden,
    /// The character climbs, hangs from a ledge, holds a ladder or rides a line: its animation
    /// places the hands.
    Traversal,
    /// The character is in a fight: raised hands are a guard or an attack, not a climb.
    Fighting,
    /// The player aims down a weapon's sights.
    Sights,
}

impl Situation {
    pub const ALL: [Situation; 7] =
        [Situation::CameraTaken, Situation::Video, Situation::Menu, Situation::WorldHidden, Situation::Traversal, Situation::Fighting, Situation::Sights];

    pub const fn index(self) -> usize {
        self as usize
    }

    pub const fn label(self) -> &'static str {
        match self {
            Situation::CameraTaken => "another camera shows the view",
            Situation::Video => "a video plays",
            Situation::Menu => "a menu is up",
            Situation::WorldHidden => "the world is not drawn",
            Situation::Traversal => "climbing",
            Situation::Fighting => "fighting",
            Situation::Sights => "aiming down sights",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Entry {
    /// Ever reported.
    reported: bool,
    /// The latest report.
    active: bool,
    /// The latest report counts until this time.
    until: u64,
    /// When the latest report's value began (its first report after a change).
    since: u64,
    /// Times it began.
    began: u32,
}

/// Every situation's latest report.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Situations {
    entries: [Entry; Situation::ALL.len()],
}

impl Situations {
    pub const fn new() -> Self {
        Self { entries: [Entry { reported: false, active: false, until: 0, since: 0, began: 0 }; Situation::ALL.len()] }
    }

    /// The game says whether `situation` holds at `now`, good for `fresh` milliseconds: a game
    /// that reports each frame gives a few frames' worth (a report missed now and then must not
    /// flicker it); one that knows a situation lasts a while ("fighting for the next 8 s") gives
    /// that. Returns the new value when it changed (an expired report counts as a change).
    pub fn report(&mut self, situation: Situation, active: bool, now: u64, fresh: u64) -> Option<bool> {
        let was = self.state(situation, now);
        let entry = &mut self.entries[situation.index()];
        let changed = was != Some(active);
        if changed {
            entry.since = now;
            if active {
                entry.began += 1;
            }
        }
        entry.reported = true;
        entry.active = active;
        // A shorter report never cuts a longer one still running.
        entry.until = if active && !changed { entry.until.max(now + fresh) } else { now + fresh };
        changed.then_some(active)
    }

    /// The latest report while it holds: `None` when never reported or no longer fresh.
    pub fn state(&self, situation: Situation, now: u64) -> Option<bool> {
        let entry = &self.entries[situation.index()];
        (entry.reported && now <= entry.until).then_some(entry.active)
    }

    /// Whether `situation` holds now (not known counts as not).
    pub fn active(&self, situation: Situation, now: u64) -> bool {
        self.state(situation, now) == Some(true)
    }

    /// How long `situation` has held (milliseconds), while it does.
    pub fn active_for(&self, situation: Situation, now: u64) -> Option<u64> {
        self.active(situation, now).then(|| now.saturating_sub(self.entries[situation.index()].since))
    }

    /// Whether any of `situations` holds now.
    pub fn any(&self, situations: &[Situation], now: u64) -> bool {
        situations.iter().any(|&s| self.active(s, now))
    }

    /// Head aim stands down: the player's look does not reach the view (another camera has it, a
    /// video or a menu is up, nothing is drawn), so writing it there would turn the view twice or
    /// fight the game's own camera. The head still looks around whatever is shown.
    pub fn head_aim_stands_down(&self, now: u64) -> bool {
        self.any(&[Situation::CameraTaken, Situation::Video, Situation::Menu, Situation::WorldHidden], now)
    }

    /// The frames go to the flat screen: no world is drawn to stand in (a video, nothing drawn).
    pub fn flat(&self, now: u64) -> bool {
        self.flat_reason(now).is_some()
    }

    /// Why the frames go to the flat screen ([`Situations::flat`]): the first of its situations
    /// that holds now.
    pub fn flat_reason(&self, now: u64) -> Option<Situation> {
        [Situation::Video, Situation::WorldHidden].into_iter().find(|&s| self.active(s, now))
    }

    /// The arms go to the game's animation: a cutscene places them, or a climb does (unless the
    /// character is fighting, whose raised hands stay on the controllers).
    pub fn arms_to_game(&self, now: u64) -> bool {
        self.active(Situation::CameraTaken, now) || (self.active(Situation::Traversal, now) && !self.active(Situation::Fighting, now))
    }

    /// Gestures and swings press nothing: the game is not taking the player's actions (a cutscene,
    /// a video, a menu, nothing drawn) or the hands hold on (climbing).
    pub fn hands_held_back(&self, now: u64) -> bool {
        self.head_aim_stands_down(now) || (self.active(Situation::Traversal, now) && !self.active(Situation::Fighting, now))
    }

    /// The head's position fades out of the view: down a sight, the eye stays on its line.
    pub fn head_position_fades(&self, now: u64) -> bool {
        self.active(Situation::Sights, now)
    }

    /// How many times each reported situation began, for a run's summary: `"climbing 3, fighting
    /// 12"`, or `"none reported"`.
    pub fn summary(&self) -> String {
        let parts: Vec<String> = Situation::ALL
            .iter()
            .filter(|s| self.entries[s.index()].reported)
            .map(|s| format!("{} {}", s.label(), self.entries[s.index()].began))
            .collect();
        if parts.is_empty() { "none reported".to_owned() } else { parts.join(", ") }
    }
}

#[cfg(test)]
mod tests;
