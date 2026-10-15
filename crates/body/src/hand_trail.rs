//! Both hands' recent way in the game world, for what the hands do outside the arms' own update (a
//! throw leaving the hand, a bow's draw, a reload gesture). A game keeps one [`HandTrails`] per
//! thing it follows (the controllers' palms on the tracking origin its arms stand on, or the hands
//! of the arms it animates) and notes it each time its rig poses the arms; each side is a trail
//! (`monaka_core::trail`), for its speed and stroke, with the latest palm-to-world matrix noted on
//! it. Times are seconds on one clock ([`now`]), which the hand gestures keep time by too.

use monaka_core::camera::{Mat34, apply_head};
use monaka_core::gesture::{Release, ReleaseRules};
use monaka_core::math::Vec3;
use monaka_contract::protocol::HandPose;
use monaka_core::trail::Trail;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// What a side with no place this note does to its trail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lost {
    /// Kept: it empties as time goes on ([`Trail::expire`]).
    Kept,
    /// Forgotten at once: the hand's next way starts afresh.
    Forgotten,
}

/// How long each hand's way is kept and how speeds are read on it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrailRules {
    /// Seconds kept.
    pub kept: f64,
    /// A velocity is read over at least this long (seconds): a few game frames.
    pub span: f64,
    pub lost: Lost,
}

/// One hand: its way in the world, and the latest palm-to-world matrix noted on it.
#[derive(Debug)]
struct Hand {
    trail: Trail,
    latest: Option<Mat34>,
}

/// Both hands' ways (`LEFT_HAND`, `RIGHT_HAND`), kept as `rules` say.
#[derive(Debug)]
pub struct HandTrails {
    rules: TrailRules,
    hands: Mutex<[Hand; 2]>,
}

/// The trails' clock.
static STARTED: OnceLock<Instant> = OnceLock::new();

/// Seconds on the trails' clock.
pub fn now() -> f64 {
    STARTED.get_or_init(Instant::now).elapsed().as_secs_f64()
}

impl HandTrails {
    pub const fn new(rules: TrailRules) -> Self {
        let hands = [Hand { trail: Trail::new(rules.kept), latest: None }, Hand { trail: Trail::new(rules.kept), latest: None }];
        Self { rules, hands: Mutex::new(hands) }
    }

    /// How the ways are kept and their speeds read.
    pub fn rules(&self) -> TrailRules {
        self.rules
    }

    /// Notes the palms placed on `origin` now (from the rig's pose). Several notes in one update
    /// give one pose: the first is kept ([`Trail::SAME_UPDATE`]).
    pub fn note(&self, origin: &Mat34, palms: &[Option<HandPose>; 2]) {
        let worlds = palms.map(|palm| palm.filter(|p| p.valid != 0).map(|p| apply_head(origin, p.orientation, p.position)));
        self.keep(worlds.map(|world| world.filter(|m| m.iter().all(|v| v.is_finite())).map(|m| (position(&m), Some(m)))));
    }

    /// Notes each side's place now (a hand read back from the game's arms), without its turn.
    pub fn note_positions(&self, positions: [Option<Vec3>; 2]) {
        self.keep(positions.map(|p| p.filter(|p| p.iter().all(|c| c.is_finite())).map(|p| (p, None))));
    }

    /// Each side's place (and palm-to-world matrix, if known) into its trail; a side with none
    /// goes as [`TrailRules::lost`] says; what is too old goes.
    fn keep(&self, places: [Option<(Vec3, Option<Mat34>)>; 2]) {
        let now = now();
        let Ok(mut hands) = self.hands.lock() else { return };
        for (hand, place) in hands.iter_mut().zip(places) {
            match place {
                Some((position, world)) => {
                    if hand.trail.push_position(now, position) && world.is_some() {
                        hand.latest = world;
                    }
                }
                None if self.rules.lost == Lost::Forgotten => hand.trail.clear(),
                None => {}
            }
            hand.trail.expire(now);
        }
    }

    /// Hand `side`'s latest palm-to-world matrix, if it was noted within `fresh` seconds.
    pub fn palm(&self, side: usize, fresh: f64) -> Option<Mat34> {
        let now = now();
        let hands = self.hands.lock().ok()?;
        let hand = hands.get(side)?;
        hand.trail.fresh(now, fresh)?;
        hand.latest
    }

    /// Hand `side`'s latest place.
    pub fn position(&self, side: usize) -> Option<Vec3> {
        self.hands.lock().ok()?.get(side)?.trail.latest().map(|s| s.position)
    }

    /// Hand `side`'s fastest velocity (m/s, each over [`TrailRules::span`]) over the last `within`
    /// seconds. An action's speed is its peak: by the time a button is let go the hand may already
    /// be slowing.
    pub fn fastest(&self, side: usize, within: f64) -> Option<Vec3> {
        let hands = self.hands.lock().ok()?;
        hands.get(side)?.trail.fastest(within, self.rules.span).map(|peak| peak.velocity)
    }

    /// Which way hand `side` travelled through its fastest moment in the last `within` seconds
    /// (unit): from `before` seconds before that moment to `after` seconds after it (the newest note
    /// at most). `None` when it hardly moved. Steadier than one span's velocity, and a flick that
    /// stopped before the button was let go keeps its way.
    pub fn stroke(&self, side: usize, within: f64, before: f64, after: f64) -> Option<Vec3> {
        let hands = self.hands.lock().ok()?;
        hands.get(side)?.trail.stroke(within, self.rules.span, before, after)
    }

    /// Hand `side`'s throw now ([`Release::read`]), `None` without a peak.
    pub fn release(&self, side: usize, rules: &ReleaseRules) -> Option<Release> {
        let hands = self.hands.lock().ok()?;
        Release::read(&hands.get(side)?.trail, rules)
    }

    /// How many notes each side holds now.
    pub fn held(&self) -> [usize; 2] {
        self.hands.lock().map(|hands| hands.each_ref().map(|h| h.trail.len())).unwrap_or_default()
    }
}

/// The position in a palm-to-world matrix.
fn position(world: &Mat34) -> Vec3 {
    [world[3], world[7], world[11]]
}

#[cfg(test)]
mod tests;
