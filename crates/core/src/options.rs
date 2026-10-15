//! Producer options: `key=value` lines in a text file beside the injected DLL. Blank lines and
//! lines starting with `#` are ignored; a later line overrides an earlier one.
//!
//! The launcher writes the file from the game's manifest (`monaka_contract::manifest`), one line per
//! option, so the ids here are the launcher's own. [`Shared`] reads the options every game means
//! the same way; a game's config reads its own on top. [`Options`] remembers which keys were
//! read, so a producer test can check it reads every option its manifest offers
//! (`manifest::unread_options`).

use crate::alternate::Schedule;
use crate::camera::{Frustum, HeadPosition};
use monaka_contract::manifest::{Manifest, aims, parse_size};
use monaka_contract::protocol::{HeadPose, LEFT_HAND, RIGHT_HAND};
use std::str::FromStr;
use std::sync::Mutex;

#[derive(Debug, Default)]
pub struct Options {
    entries: Vec<(String, String)>,
    /// Every key looked up so far (read or missing), for [`Options::keys_read`].
    read: Mutex<Vec<String>>,
}

impl Clone for Options {
    fn clone(&self) -> Self {
        Self { entries: self.entries.clone(), read: Mutex::new(Vec::new()) }
    }
}

impl PartialEq for Options {
    fn eq(&self, other: &Self) -> bool {
        self.entries == other.entries
    }
}

impl Options {
    pub fn parse(text: &str) -> Self {
        let entries = text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .filter_map(|line| line.split_once('='))
            .map(|(key, value)| (key.trim().to_owned(), value.trim().to_owned()))
            .collect();
        Self { entries, read: Mutex::new(Vec::new()) }
    }

    pub fn text(&self, key: &str) -> Option<&str> {
        if let Ok(mut read) = self.read.lock()
            && !read.iter().any(|k| k == key)
        {
            read.push(key.to_owned());
        }
        self.entries.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    /// The keys looked up so far, in first-read order.
    pub fn keys_read(&self) -> Vec<String> {
        self.read.lock().map(|r| r.clone()).unwrap_or_default()
    }

    pub fn value<T: FromStr>(&self, key: &str) -> Option<T> {
        self.text(key)?.parse().ok()
    }

    /// A finite number inside `min..=max`, else `default`.
    pub fn number(&self, key: &str, min: f32, max: f32, default: f32) -> f32 {
        self.value::<f32>(key).filter(|v| v.is_finite() && (min..=max).contains(v)).unwrap_or(default)
    }

    /// An image size (`WxH`, see [`parse_size`]).
    pub fn size(&self, key: &str) -> Option<(u32, u32)> {
        parse_size(self.text(key)?)
    }

    /// An angle in degrees, finite and at most `limit` either way.
    pub fn angle(&self, key: &str, limit: f32) -> Option<f32> {
        self.value::<f32>(key).filter(|a| a.is_finite() && a.abs() <= limit)
    }

    /// `1`/`true`/`yes`/`on` or `0`/`false`/`no`/`off`, else `default`.
    pub fn switch(&self, key: &str, default: bool) -> bool {
        self.text(key).and_then(monaka_contract::manifest::parse_switch).unwrap_or(default)
    }

    /// One of `choices` by its id (any case), else `default`.
    pub fn choice<T: Copy>(&self, key: &str, choices: &[(&str, T)], default: T) -> T {
        let text = self.text(key).map(str::trim);
        choices.iter().find(|(id, _)| text.is_some_and(|t| t.eq_ignore_ascii_case(id))).map_or(default, |(_, v)| *v)
    }

    /// `N` comma-separated numbers, each finite and at most `limit` in size (`hand_hold=x,y,z`).
    pub fn floats<const N: usize>(&self, key: &str, limit: f32) -> Option<[f32; N]> {
        let values: Vec<f32> = self.text(key)?.split(',').map(|v| v.trim().parse::<f32>().ok().filter(|v| v.is_finite() && v.abs() <= limit)).collect::<Option<_>>()?;
        values.try_into().ok()
    }

    /// `left` (any case) is the left controller; anything else the right.
    pub fn hand(&self, key: &str) -> usize {
        if self.text(key).is_some_and(|h| h.trim().eq_ignore_ascii_case("left")) { LEFT_HAND } else { RIGHT_HAND }
    }

    /// `off`/`0`/`none`, `full`/`1`, or `lean`, else `default`.
    pub fn head_position(&self, key: &str, default: HeadPosition) -> HeadPosition {
        self.choice(key, &[("off", HeadPosition::Off), ("0", HeadPosition::Off), ("none", HeadPosition::Off), ("full", HeadPosition::Full), ("1", HeadPosition::Full), ("lean", HeadPosition::Lean)], default)
    }
}

/// The options the launcher's defaults would make, and once more for each value another option
/// waits on (`Opt::when`: the FSR settings behind `fsr=1`, the aiming hand behind
/// `aim=controller`). With `read` having parsed each, the ids it never looked at: a producer
/// test's check that it reads every option its manifest offers.
pub fn unread_options(game: &Manifest, mut read: impl FnMut(&Options)) -> Vec<String> {
    let mut variants: Vec<Option<(&str, &str)>> = vec![None];
    for opt in game.options {
        if let Some(when) = opt.when
            && !variants.contains(&Some(when))
        {
            variants.push(Some(when));
        }
    }
    let mut seen: Vec<String> = Vec::new();
    for variant in variants {
        let text: Vec<String> = game
            .options
            .iter()
            .filter(|o| o.producer)
            .map(|o| match variant {
                Some((id, value)) if id == o.id => format!("{id}={value}"),
                _ => format!("{}={}", o.id, o.default_text()),
            })
            .collect();
        let options = Options::parse(&text.join("\n"));
        read(&options);
        seen.extend(options.keys_read());
    }
    game.options.iter().filter(|o| o.producer && !seen.iter().any(|k| k == o.id)).map(|o| o.id.to_owned()).collect()
}

// ---- The settings every game reads alike ----

/// What aims (the launcher's `aim` option: `hand_rig`, `head`, `controller` or `off`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Aim {
    /// The character looks, aims and attacks where the head points.
    pub head: bool,
    /// A controller aims instead of the head (needs head aim); the view stays the head's.
    pub hand: bool,
    /// The first-person arms follow the controllers (turns hand aim on).
    pub rig: bool,
    /// The controller hand aim follows (`aim_hand`: `LEFT_HAND` or `RIGHT_HAND`).
    pub side: usize,
}

impl Aim {
    pub const OFF: Aim = Aim { head: false, hand: false, rig: false, side: RIGHT_HAND };

    /// From `aim=` (the manifest's default when missing), or the older `head_aim`, `hand_aim`
    /// and `hand_rig` switches when there is no `aim` at all; `aim_hand=left|right`.
    pub fn from_options(options: &Options, manifest: &Manifest) -> Self {
        let side = options.hand("aim_hand");
        let choice = options.text("aim").map(|t| t.trim().to_ascii_lowercase());
        let older = ["head_aim", "hand_aim", "hand_rig"].iter().any(|key| options.text(key).is_some());
        let chosen = choice.as_deref().or_else(|| if older { None } else { manifest.choice_default("aim") });
        let (head, hand, rig) = match chosen {
            Some(aims::OFF) => (false, false, false),
            Some(aims::HEAD) => (true, false, false),
            Some(aims::CONTROLLER) => (true, true, false),
            Some(aims::HAND_RIG) => (true, true, true),
            _ => {
                let head = options.switch("head_aim", true);
                let rig = head && options.switch("hand_rig", false);
                (head, head && (options.switch("hand_aim", false) || rig), rig)
            }
        };
        Self { head, hand, rig, side }
    }
}

/// The HUD in front of the eyes, and its pieces on the hands.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hud {
    /// Place the HUD and menus in front of the eyes at all (`hud=0`: as the game draws them).
    pub shown: bool,
    /// The HUD's pieces on the hands instead of in front of the eyes (`world_hud`).
    pub world: bool,
    /// The HUD's size, 1 being its size on a monitor (`hud_scale`).
    pub scale: f32,
    /// How far away the HUD appears, metres (`hud_distance`).
    pub distance: f32,
    /// Markers (objectives, waypoints) at their targets' distance instead of in the flat HUD
    /// (`world_markers`).
    pub markers: bool,
}

/// The free hands' fingers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fingers {
    /// Follow the headset's finger tracking, when the viewer has any (`finger_tracking`).
    pub tracking: bool,
    /// Each tracked finger either open or a tight fist (`finger_binary`).
    pub binary: bool,
}

/// The settings every producer reads the same way, with the game's own defaults from its
/// manifest where the manifest offers the option.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shared {
    /// Presents between choosing an eye and presenting it (`latency`).
    pub schedule: Schedule,
    /// Metres between the eyes when the headset reports none plausible (`eye_separation`).
    pub eye_separation: f32,
    /// PC checks: a fixed eye distance in metres, any size, overriding the headset's (an
    /// exaggerated one makes swapped eyes obvious in saved pairs; `test_separation`).
    pub test_separation: Option<f32>,
    /// Follow the viewer's head pose (`head_tracking`).
    pub head_tracking: bool,
    /// How the head's position moves the view (`head_position`).
    pub head_position: HeadPosition,
    /// Render each eye with the headset's own field of view (`headset_fov`, or the older
    /// `headset_projection`); otherwise the game's own.
    pub headset_fov: bool,
    /// PC checks without a headset: a fixed pose turned `synthetic_yaw` degrees (positive left)
    /// and pitched `synthetic_pitch` degrees (positive up).
    pub synthetic: Option<(f32, f32)>,
    pub aim: Aim,
    pub fingers: Fingers,
    pub hud: Hud,
    /// The VR controllers' buttons, triggers and sticks reach the game as its gamepad (pad 0),
    /// merged with a real pad (`controller_pad`).
    pub controller_pad: bool,
    /// How fast the right stick turns the view at full tilt, degrees a second, yaw and pitch
    /// (`stick_yaw_speed`, `stick_pitch_speed`; pitch 0: the stick turns only left and right,
    /// and only the head looks up and down).
    pub stick_speed: [f32; 2],
    /// `other_builds`: the build check lets an unknown build through (applied by the producer's
    /// session before the game's start; here so it is read with the rest).
    pub other_builds: bool,
}

impl Shared {
    pub fn from_options(options: &Options, manifest: &Manifest) -> Self {
        let number = |key: &str, min: f32, max: f32, default: f32| options.number(key, min, max, manifest.number_default(key).map_or(default, |d| d as f32));
        // A switch another option turns off (the manifest's rule) reads off.
        let switch = |key: &str, default: bool| {
            options.switch(key, manifest.switch_default(key).unwrap_or(default)) && manifest.turned_off(key, |cause| options.text(cause).map(str::to_owned)).is_none()
        };
        let latency = manifest.number_default("latency").map_or(2, |d| d as u64);
        let synthetic = match (options.angle("synthetic_yaw", 180.0), options.angle("synthetic_pitch", 89.0)) {
            (None, None) => None,
            (yaw, pitch) => Some((yaw.unwrap_or(0.0), pitch.unwrap_or(0.0))),
        };
        Self {
            schedule: Schedule::from_options(options, latency),
            eye_separation: number("eye_separation", 0.03, 0.09, 0.064),
            test_separation: options.value::<f32>("test_separation").filter(|v| v.is_finite() && (0.0..=5.0).contains(v)),
            head_tracking: switch("head_tracking", true),
            head_position: options.head_position(
                "head_position",
                match manifest.choice_default("head_position") {
                    Some("off") => HeadPosition::Off,
                    Some("full") => HeadPosition::Full,
                    _ => HeadPosition::Lean,
                },
            ),
            headset_fov: options.switch("headset_fov", options.switch("headset_projection", true)),
            synthetic,
            aim: Aim::from_options(options, manifest),
            fingers: Fingers { tracking: switch("finger_tracking", true), binary: switch("finger_binary", false) },
            hud: Hud {
                shown: switch("hud", true),
                world: switch("world_hud", true),
                scale: number("hud_scale", 0.1, 1.0, 0.5),
                distance: number("hud_distance", 0.5, 20.0, 2.0),
                markers: switch("world_markers", true),
            },
            controller_pad: switch("controller_pad", true),
            stick_speed: [options.number("stick_yaw_speed", 10.0, 720.0, 150.0), options.number("stick_pitch_speed", 0.0, 720.0, 0.0)],
            other_builds: switch("other_builds", false),
        }
    }

    /// Metres between the eyes for a frame drawn from `pose`: the test separation, the headset's
    /// IPD when plausible, else the configured one.
    pub fn separation_for(&self, pose: &HeadPose) -> f32 {
        self.test_separation.unwrap_or_else(|| pose.ipd_or(self.eye_separation))
    }

    /// A headset-like pose turned `yaw` degrees and pitched `pitch`, for PC checks.
    pub fn synthetic_pose(&self, yaw: f32, pitch: f32) -> HeadPose {
        HeadPose::synthetic(yaw, pitch, HeadPose::SYNTHETIC_FOV, self.eye_separation)
    }

    /// The fixed head of a PC check, when the options ask for one.
    pub fn synthetic_head(&self) -> Option<HeadPose> {
        self.synthetic.map(|(yaw, pitch)| self.synthetic_pose(yaw, pitch))
    }

    /// The frustum eye `eye` is drawn with for `pose`: the headset's own for that eye (`centred`:
    /// the smallest centred one containing it, for engines that take only a field of view), or
    /// `game_fov` (full horizontal and vertical angles, radians) without a headset pose or with
    /// `headset_fov` off; none without either.
    pub fn rendered_frustum(&self, pose: &HeadPose, eye: usize, centred: bool, game_fov: Option<[f32; 2]>) -> Option<Frustum> {
        if self.headset_fov
            && pose.valid != 0
            && let Some(frustum) = Frustum::from_fov(pose.fov[eye])
        {
            return Some(if centred { frustum.symmetric() } else { frustum });
        }
        let [h, v] = game_fov?.map(|full| full * 0.5);
        Frustum::from_fov([-h, h, v, -v])
    }
}

/// Full horizontal and vertical angles (radians) of a centred frustum.
pub fn full_angles(frustum: &Frustum) -> [f32; 2] {
    frustum.half_extent().map(|tangent| 2.0 * tangent.atan())
}

#[cfg(test)]
mod tests;
