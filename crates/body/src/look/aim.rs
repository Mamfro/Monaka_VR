//! What aims, for every delivery of the look: the head, a hand, the left hand's tool.
//!
//! Head aim: the character looks, aims and attacks where the head points; the head alone pitches
//! it, and the head's yaw is added on top of mouse and stick turning. Hand aim: the aiming
//! controller's direction replaces the head's, toward the point the held weapon points at
//! `WEAPON_CONVERGENCE` metres out; with a melee weapon or bare hands drawn and the hand rig on,
//! the head aims instead (the attack lands where the player looks, not where the swinging
//! controller happens to point). Tool aim ([`ToolAim`]): with hand aim on the right controller,
//! while the left trigger uses the left hand's tool (the quick item or accessory: grappling hook,
//! throwables, medkits) the left controller aims instead, whatever the right hand holds, and the
//! press waits for that aim. What aims itself (a throw, a bow by hand) keeps the game's aim on the
//! head ([`aimed`]).

use monaka_core::camera::{direction_yaw_pitch, head_yaw_pitch, pointing_yaw_pitch};
use monaka_core::math::{Quat, Vec3, converged};
use monaka_core::options::Options;
use monaka_contract::protocol::{HandPose, LEFT_HAND, RIGHT_HAND};
use monaka_hook::mem;
use monaka_producer::log;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering::*};

#[cfg(test)]
mod tests;

/// What aims, for a run.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AimSettings {
    pub head_aim: bool,
    pub hand_aim: bool,
    /// The controller hand aim follows (`LEFT_HAND` or `RIGHT_HAND`).
    pub aim_hand: usize,
    pub hand_rig: bool,
    /// With a melee weapon or bare hands drawn (and the hand rig on), the head aims.
    pub melee_aim_head: bool,
    /// With hand aim on the right controller: while the left trigger uses the left hand's tool,
    /// the left controller aims ([`ToolAim`]).
    pub tool_aim: bool,
}

/// What aims this update.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aim {
    /// Yaw and pitch in radians, positive left and up.
    pub yaw: f32,
    pub pitch: f32,
    /// A controller gave it (else the head).
    pub by_hand: bool,
    /// The left controller gave it, for the left hand's tool.
    pub tool: bool,
}

impl AimSettings {
    /// Nothing aims (before a run's settings are known).
    pub const NONE: Self = Self { head_aim: false, hand_aim: false, aim_hand: RIGHT_HAND, hand_rig: false, melee_aim_head: true, tool_aim: false };

    /// From the shared `aim` ([`monaka_core::options::Aim`]: the launcher's `aim=` choice, or
    /// the older `head_aim`, `hand_aim` and `hand_rig` switches), only where `possible`; plus
    /// `melee_aim_head` and `tool_aim` (default on).
    pub fn from_options(options: &Options, aim: monaka_core::options::Aim, possible: bool) -> Self {
        Self {
            head_aim: possible && aim.head,
            hand_aim: possible && aim.hand,
            aim_hand: aim.side,
            hand_rig: possible && aim.rig,
            melee_aim_head: options.switch("melee_aim_head", true),
            tool_aim: options.switch("tool_aim", true),
        }
    }

    /// The direction to aim at `now` (`monaka_channel::tick`): the aiming controller's `palm` with
    /// hand aim (unless melee aims with the head, or the arms climb: [`crate::traverse`]), the
    /// left controller's while the left hand's tool aims ([`TOOL`]), else the head's
    /// `orientation`. `palm` gives a controller's palm pose if it is tracked.
    pub fn aim_at(&self, now: u64, head: Quat, palm: impl Fn(usize) -> Option<HandPose>) -> Aim {
        let tool = TOOL.aims(self, now, || palm(LEFT_HAND).is_some());
        let melee = crate::held::wielded(self.aim_hand).is_some_and(crate::held::Kind::melee);
        let melee_head = self.hand_rig && ((self.melee_aim_head && melee) || crate::traverse::traversing());
        let side = if tool { LEFT_HAND } else { self.aim_hand };
        match (self.hand_aim && (tool || !melee_head)).then(|| palm(side)).flatten() {
            Some(palm) => {
                let (yaw, pitch) = direction_yaw_pitch(weapon_point(&palm)).unwrap_or_else(|| pointing_yaw_pitch(palm.orientation));
                Aim { yaw, pitch, by_hand: true, tool }
            }
            None => {
                let (yaw, pitch) = head_yaw_pitch(head);
                Aim { yaw, pitch, by_hand: false, tool: false }
            }
        }
    }
}

/// What aims at `now` ([`AimSettings::aim_at`]), the head `head` or a controller's `palm`; with
/// what is in hand aiming itself (`aims_itself`, asked of the aim chosen) the head's instead, so
/// the game's aim (the flashlight with it) stays the head's. The source is counted.
pub fn aimed(settings: &AimSettings, now: u64, head: Quat, palm: impl Fn(usize) -> Option<HandPose>, aims_itself: impl FnOnce(&Aim) -> bool) -> Aim {
    let mut aim = settings.aim_at(now, head, palm);
    if aims_itself(&aim) {
        (aim.yaw, aim.pitch) = head_yaw_pitch(head);
        aim.by_hand = false;
        aim.tool = false;
    }
    note_source(aim.by_hand, settings.hand_aim);
    aim
}

/// The left trigger uses the tool in the left hand: pulled past this (XInput's threshold), the
/// left controller aims...
const TOOL_TRIGGER: f32 = 30.0 / 255.0;
/// ...and goes on aiming this long after it is let go (a throwable leaves on the release).
const TOOL_HOLD_MS: u64 = 600;
/// How old the pad's last read may be (ms) for the trigger to count.
const TRIGGER_MAX_AGE_MS: u64 = 200;

/// The left hand's tool aimed by the left controller (`tool_aim`, with hand aim on the right
/// controller): while the left trigger is pulled, and [`TOOL_HOLD_MS`] after, the left controller
/// aims. The press itself reaches the game through the pad's gate
/// ([`monaka_pad::gate_left_trigger`]): a tool that fires on the press, along the camera (the
/// grappling hook), must see a camera built from the left controller's aim, so the press is let
/// through the update after the first that wrote that aim ([`ToolAim::gate`]).
pub struct ToolAim {
    /// Until when (`monaka_channel::tick`) the left controller aims.
    until: AtomicU64,
    presses: AtomicU64,
    /// The update that first wrote the left controller's aim for the tool now in use.
    from: Mutex<Option<u64>>,
}

pub static TOOL: ToolAim = ToolAim::new();

impl ToolAim {
    pub const fn new() -> Self {
        Self { until: AtomicU64::new(0), presses: AtomicU64::new(0), from: Mutex::new(None) }
    }

    /// Whether the tool can take the aim at all: hand aim on the right controller, tool aim on,
    /// the arms not climbing.
    pub fn possible(&self, settings: &AimSettings) -> bool {
        settings.hand_aim && settings.tool_aim && settings.aim_hand != LEFT_HAND && !crate::traverse::traversing()
    }

    /// Whether the tool aims now: the left trigger held, or let go within [`TOOL_HOLD_MS`].
    /// `left_tracked` says whether the left controller is tracked, for the log of a press.
    pub fn aims(&self, settings: &AimSettings, now: u64, left_tracked: impl FnOnce() -> bool) -> bool {
        if !self.possible(settings) {
            return false;
        }
        if let Some(pull) = monaka_pad::left_trigger(TRIGGER_MAX_AGE_MS).filter(|&t| t >= TOOL_TRIGGER)
            && self.until.swap(now + TOOL_HOLD_MS, Relaxed) <= now
            && self.presses.fetch_add(1, Relaxed) < 20
        {
            log!("tool aim: left trigger {pull:.2}; the left controller aims (left palm {})", if left_tracked() { "tracked" } else { "NOT tracked: the head aims" });
        }
        now < self.until.load(Relaxed)
    }

    /// The left trigger's gate at update `frame`: while `gate` (the tool possible, no cutscene) a
    /// press is held back until a camera built from the left controller's aim is there, the update
    /// after the first that wrote it (`tool`: this update wrote it). Without the gate nothing waits.
    pub fn gate(&self, gate: bool, tool: bool, frame: u64) {
        monaka_pad::gate_left_trigger(gate);
        let Ok(mut from) = self.from.lock() else { return };
        *from = if tool && gate { Some(from.unwrap_or(frame)) } else { None };
        if monaka_pad::left_trigger_held_back() && from.is_none_or(|first| first < frame) {
            monaka_pad::open_left_trigger();
        }
    }

    /// Hands the trigger back to the game (head aim not steering; the end of a run).
    pub fn release(&self) {
        monaka_pad::gate_left_trigger(false);
        if let Ok(mut from) = self.from.lock() {
            *from = None;
        }
    }
}

impl Default for ToolAim {
    fn default() -> Self {
        Self::new()
    }
}

/// The game's input-action converter got `value` for the action at `binding` (its id at +0, flags
/// at +0x10, 0x10 meaning inverted). When the id is a vertical look action (`is_pitch`), the value
/// that means "no input" (0, or 1 when inverted); otherwise `None`. While head aim steers, the head
/// alone pitches the character, so mouse and stick pitch are neutralised this way (all three Dying
/// Light games lay their converters' bindings out so).
pub fn neutral_pitch(binding: usize, is_pitch: impl Fn(u32) -> bool) -> Option<f32> {
    let action = mem::read::<u32>(binding)?;
    is_pitch(action).then(|| if mem::read::<u8>(binding + 0x10).is_some_and(|f| f & 0x10 != 0) { 1.0 } else { 0.0 })
}

static HAND_UPDATES: AtomicU64 = AtomicU64::new(0);
static HEAD_UPDATES: AtomicU64 = AtomicU64::new(0);
/// 0 before the first update, then 1 for the head, 2 for the hand.
static SOURCE: AtomicU64 = AtomicU64::new(0);
static SWITCHES: AtomicU64 = AtomicU64::new(0);

/// The point a weapon held in `palm` points at ([`converged`], where the viewer's reticle sits), as
/// the game's camera sees it (tracking space, from the camera): the camera is where room-scale
/// following has walked the character ([`crate::roomscale::anchor`]), not at the tracking origin.
/// Aimed at from the origin, shots would miss the reticle by up to the distance walked.
pub fn weapon_point(palm: &HandPose) -> Vec3 {
    from_camera(converged(palm.position, palm.orientation), crate::roomscale::anchor())
}

/// `point` (tracking space) from a camera that following has walked to `anchor` (x and z).
fn from_camera(point: Vec3, anchor: [f32; 2]) -> Vec3 {
    [point[0] - anchor[0], point[1], point[2] - anchor[1]]
}

/// Counts which direction aimed this update and, with hand aim on, logs the first switches between
/// the controller and the head.
pub fn note_source(by_hand: bool, hand_aim: bool) {
    (if by_hand { &HAND_UPDATES } else { &HEAD_UPDATES }).fetch_add(1, Relaxed);
    let now = if by_hand { 2 } else { 1 };
    if SOURCE.swap(now, Relaxed) != now && hand_aim && SWITCHES.fetch_add(1, Relaxed) < 40 {
        log!("aiming with the {}", if by_hand { "controller" } else { "head (controller off or not tracked)" });
    }
}

/// Hand aim's tally for the end-of-run report.
pub fn report_sources(hand_aim: bool) {
    if hand_aim {
        log!("hand aim: {} updates aimed by the controller, {} by the head", HAND_UPDATES.load(Relaxed), HEAD_UPDATES.load(Relaxed));
    }
}
