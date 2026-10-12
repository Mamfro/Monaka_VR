//! What the character aims with, and steering its look toward it: the parts of head aim and hand
//! aim the Dying Light producers share. A producer finds its player character and the moment its
//! camera is built; this picks the direction ([`AimSettings::aim_at`]) and writes the look
//! targets ([`LookSteer`]), and neutralises vertical look input while it does ([`neutral_pitch`]).
//!
//! Head aim: the character's look targets follow the head, so it looks, aims and attacks where the
//! head points; the head alone pitches it, and the head's yaw is added on top of mouse and stick
//! turning (`monaka_core::aim::HeadAim`). Hand aim: the aiming controller's direction replaces the
//! head's, toward the point the held weapon points at `WEAPON_CONVERGENCE` metres out; with a melee
//! weapon or bare hands drawn and the hand rig on, the head aims instead (the attack lands where the
//! player looks, not where the swinging controller happens to point). Tool aim ([`ToolAim`]): with
//! hand aim on the right controller, while the left trigger uses the left hand's tool (the quick
//! item or accessory: grappling hook, throwables, medkits) the left controller aims instead,
//! whatever the right hand holds, and the press waits for that aim.

use monaka_core::aim::{Convention, HeadAim, Look, PitchMode};
use monaka_core::camera::{direction_yaw_pitch, head_yaw_pitch, pointing_yaw_pitch};
use monaka_core::math::{Quat, Vec3, converged};
use monaka_core::options::Options;
use monaka_core::protocol::{HandPose, LEFT_HAND};
use monaka_hook::mem;
use monaka_producer::log;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering::*};

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

    /// These settings where `possible`, else nothing aims (head aim, hand aim and the rig off).
    pub fn only_if(self, possible: bool) -> Self {
        if possible { self } else { Self { head_aim: false, hand_aim: false, hand_rig: false, ..self } }
    }

    /// The direction to aim at `now` (`monaka_channel::tick`): the aiming controller's `palm` with
    /// hand aim (unless melee aims with the head, or the arms climb: [`crate::traverse`]), the
    /// left controller's while the left hand's tool aims ([`TOOL`]), else the head's
    /// `orientation`. `palm` gives a controller's palm pose if it is tracked.
    pub fn aim_at(&self, now: u64, head: Quat, palm: impl Fn(usize) -> Option<HandPose>) -> Aim {
        let tool = TOOL.aims(self, now, || palm(LEFT_HAND).is_some());
        let melee_head = self.hand_rig && ((self.melee_aim_head && crate::melee::holding_melee()) || crate::traverse::traversing());
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

/// Where a game's character keeps its look targets (floats at these offsets) and how its angles
/// run.
#[derive(Clone, Copy, Debug)]
pub struct LookFields {
    pub yaw: usize,
    pub pitch: usize,
    pub convention: Convention,
}

impl LookFields {
    pub fn read(&self, character: usize) -> Option<Look> {
        Some(Look { yaw: mem::read::<f32>(character + self.yaw)?, pitch: mem::read::<f32>(character + self.pitch)? })
    }

    pub fn write(&self, character: usize, look: Look) {
        mem::write(character + self.yaw, look.yaw);
        mem::write(character + self.pitch, look.pitch);
    }
}

/// Steering a character's look targets with head aim (the head alone pitches it), keeping what was
/// added so it can be handed back.
pub struct LookSteer {
    pub fields: LookFields,
    pub applied: HeadAim,
}

impl LookSteer {
    pub const fn new(fields: LookFields) -> Self {
        Self { fields, applied: HeadAim::NONE }
    }

    /// The head yaw (radians) the targets carry for `character` now: what the camera built from
    /// them was turned by, to turn back out of the view.
    pub fn share(&self, character: usize) -> f32 {
        if character == self.applied.owner() { self.applied.yaw_share_radians(&self.fields.convention) } else { 0.0 }
    }

    /// Steers `character` toward `yaw` and `pitch` (radians, positive left and up). Returns the
    /// look written and the character's own (what it had without the head), or `None` when its
    /// stored angles are implausible (nothing written; a new player object starts clean).
    pub fn steer(&mut self, character: usize, yaw: f32, pitch: f32) -> Option<(Look, Look)> {
        if !yaw.is_finite() || !pitch.is_finite() {
            return None;
        }
        let stored = self.fields.read(character)?;
        let (write, own) = self.applied.apply(character, stored, yaw, pitch, PitchMode::HeadOnly, &self.fields.convention)?;
        self.fields.write(character, write);
        Some((write, own))
    }

    /// Hands `character` its own look back (VR stopping); true when there was a share to hand back.
    pub fn release(&mut self, character: usize) -> bool {
        if !self.applied.holds_share() {
            return false;
        }
        let Some(stored) = self.fields.read(character) else { return false };
        match self.applied.release(character, stored) {
            Some(own) => {
                self.fields.write(character, own);
                true
            }
            None => false,
        }
    }

    pub fn holds_share(&self) -> bool {
        self.applied.holds_share()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_weapon_point_is_aimed_at_from_where_following_walked_the_camera() {
        // The reticle 10 m ahead of a palm held 0.4 m right of the head; the player walked 1 m
        // forward (-z) and the character followed: from the camera there, the point is 1 m nearer.
        let point = [0.4, 1.4, -10.0];
        assert_eq!(from_camera(point, [0.0, 0.0]), point, "no following: the origin is the camera");
        assert_eq!(from_camera(point, [0.0, -1.0]), [0.4, 1.4, -9.0]);
        // The yaw toward it differs: aimed from the origin it would miss to the side.
        let from_origin = direction_yaw_pitch(point).unwrap().0;
        let walked_sideways = direction_yaw_pitch(from_camera(point, [1.0, 0.0])).unwrap().0;
        assert!((from_origin - walked_sideways).abs() > 0.05, "{from_origin} {walked_sideways}");
    }
    use monaka_core::protocol::RIGHT_HAND;

    /// A manifest without an `aim` option: the older switches decide.
    const TEST_MANIFEST: monaka_core::manifest::Manifest = monaka_core::manifest::Manifest {
        id: "t",
        name: "Test",
        status: monaka_core::manifest::Status::Experimental,
        executables: &["T.exe"],
        steam_app: 0,
        producer: "monaka_t",
        log: "monaka_t.log",
        options_file: "t-options.txt",
        run_prefix: "t",
        launch: monaka_core::manifest::Launch::Manual,
        refused_arguments: &[],
        before: "",
        options: &[],
        licences: &[],
        left_hand_aims: true,
        weapon_ring: true,
        controls: &[],
    };

    #[test]
    fn settings_come_from_options() {
        let aim = |text: &str| monaka_core::options::Aim::from_options(&Options::parse(text), &TEST_MANIFEST);
        let s = AimSettings::from_options(&Options::parse("hand_rig=1\naim_hand=Left"), aim("hand_rig=1\naim_hand=Left"), true);
        assert!(s.head_aim && s.hand_aim && s.hand_rig && s.melee_aim_head && s.aim_hand == LEFT_HAND);
        let off = AimSettings::from_options(&Options::default(), aim("head_aim=0\nhand_rig=1"), true);
        assert!(!off.hand_aim && !off.hand_rig, "hand aim and the rig ride on head aim");
        let impossible = AimSettings::from_options(&Options::default(), aim("head_aim=1\nhand_rig=1"), false);
        assert!(!impossible.head_aim && !impossible.hand_rig, "an option cannot turn on what the run cannot do");
        assert_eq!(AimSettings::from_options(&Options::default(), aim(""), false).aim_hand, RIGHT_HAND);
    }

    #[test]
    fn the_head_aims_without_a_controller() {
        let chosen = |text: &str| monaka_core::options::Aim::from_options(&Options::parse(text), &TEST_MANIFEST);
        let s = AimSettings::from_options(&Options::default(), chosen("hand_aim=1"), true);
        assert!(s.tool_aim, "tool aim comes with hand aim");
        let aim = s.aim_at(0, [0.0, 0.0, 0.0, 1.0], |_| None);
        assert!(!aim.by_hand && !aim.tool && aim.yaw.abs() < 1e-6 && aim.pitch.abs() < 1e-6);
        let palm = HandPose { orientation: monaka_core::math::yaw_rotation(0.5), position: [0.0; 3], valid: 1, received_tick: 0 };
        let aim = s.aim_at(0, [0.0, 0.0, 0.0, 1.0], |_| Some(palm));
        assert!(aim.by_hand && !aim.tool && (aim.yaw - 0.5).abs() < 1e-3, "{aim:?}");
        // No pad read: the tool never aims; the left controller is never possible as the tool.
        assert!(!TOOL.aims(&s, 0, || true));
        let left = AimSettings::from_options(&Options::default(), chosen("hand_aim=1\naim_hand=left"), true);
        assert!(!TOOL.possible(&left) && !AimSettings::from_options(&Options::parse("tool_aim=0"), chosen("hand_aim=1"), true).tool_aim);
    }

    #[test]
    fn the_tool_gate_opens_the_update_after_the_aim() {
        let tool = ToolAim::new();
        // Gated without the aim yet: nothing opens; the first update that writes it is remembered.
        tool.gate(true, false, 10);
        assert!(tool.from.lock().unwrap().is_none());
        tool.gate(true, true, 11);
        assert_eq!(*tool.from.lock().unwrap(), Some(11));
        tool.gate(true, true, 12);
        assert_eq!(*tool.from.lock().unwrap(), Some(11), "the first update stays");
        // The tool let go: forgotten; released: the gate is off.
        tool.gate(true, false, 13);
        assert!(tool.from.lock().unwrap().is_none());
        tool.release();
    }
}
