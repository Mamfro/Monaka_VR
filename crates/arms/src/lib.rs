//! First-person arms on the VR controllers, for any game whose arms are a skeleton of named
//! elements (Dying Light 1 and 2: `r_hand`, `l_upperarm`, `r_handholder`, ...). A producer hooks
//! the moment its game has placed the arms against the player camera, takes a [`Snapshot`] of the
//! animated arms before the game's own adjustments, and calls [`follow`] after them.
//!
//! The aim-side wrist goes into its controller's palm (`HandPose::palm`: the grip pose's position,
//! the aim pose's direction), with the fingers and what they hold animated against it as the game
//! does; a gun instead sits in the palm by its grip, pointing where the controller aims. Each upper
//! arm and forearm reaches its hand from a shoulder under the player's real head
//! ([`monaka_core::reach`]). The off hand rides a weapon the game holds two-handed, else follows its
//! own controller. The rest of the body goes with the head, as animated (moving the whole skeleton
//! with the hand would move the body with it). [`melee`] turns swings into attacks. Climbing and
//! hanging, the arms go back to the game's animation ([`traverse`]). [`roomscale`] walks the
//! character after the player's real steps.

pub mod aim;
pub mod controllers;
pub mod fingers;
pub mod melee;
pub mod mouse_aim;
pub mod roomscale;
pub mod traverse;

use monaka_core::camera::{self as cam, Mat34, apply_head, compose, rigid_inverse, translation};
use monaka_core::math::{Vec3, yaw, yaw_rotation};
use monaka_core::protocol::{HandPose, HeadPose, LEFT_HAND, RIGHT_HAND};
use monaka_core::reach::{apply, arm_moves, spin_about, twist_between};
use monaka_producer::log;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering::*};

/// A game's skeleton: its elements (bones, helpers and, in some games, mesh parts) by index, each
/// with a world matrix (row-major 3x4, camera-to-world convention: columns right, up, back,
/// position).
pub trait Skeleton {
    /// Identifies this skeleton (its address): a different one is looked up again.
    fn id(&self) -> usize;
    fn count(&self) -> i32;
    fn name(&self, element: i32) -> String;
    /// The parent element, or a negative number for a root.
    fn parent(&self, element: i32) -> i32;
    fn world(&self, element: i32) -> Option<Mat34>;
    fn set_world(&self, element: i32, matrix: &Mat34);
    /// Whether the element is a bone (helpers and mesh parts follow their bones and are never
    /// written: moving them leaves the arms where the game put them).
    fn is_bone(&self, element: i32) -> bool;
}

/// Every element's world matrix of a skeleton at one moment. An element the skeleton cannot give
/// as finite numbers is kept as NaN and never written: Dying Light 2 can leave an outfit's glove
/// bone so for good after a cutscene, and refusing the whole skeleton for it would leave the arms
/// to the game.
pub struct Snapshot {
    pub skeleton: usize,
    pub worlds: Vec<Mat34>,
}

static SPARE: Mutex<Vec<Mat34>> = Mutex::new(Vec::new());

impl Snapshot {
    pub fn take(skeleton: &dyn Skeleton) -> Option<Self> {
        let mut worlds = SPARE.lock().ok().map(|mut spare| std::mem::take(&mut *spare)).unwrap_or_default();
        worlds.clear();
        for element in 0..skeleton.count() {
            worlds.push(skeleton.world(element).filter(|w| w.iter().all(|v| v.is_finite())).unwrap_or([f32::NAN; 12]));
        }
        worlds.iter().any(|w| w[0].is_finite()).then_some(Self { skeleton: skeleton.id(), worlds })
    }
}

impl Drop for Snapshot {
    fn drop(&mut self) {
        // Keep the allocation for the next call.
        if let Ok(mut spare) = SPARE.lock() {
            *spare = std::mem::take(&mut self.worlds);
        }
    }
}

/// Which arm part each element belongs to (with the arm's side, `LEFT_HAND` or `RIGHT_HAND`).
#[derive(Clone, Copy, PartialEq)]
enum Part {
    Body,
    /// A clavicle: carried with the body under the real head, unsquashed, so the shoulder joins the
    /// moved upper arm (left squashed, the skin between them stretches across the view).
    Clavicle(usize),
    UpperArm(usize),
    Forearm(usize),
    /// A forearm twist bone (named `*twist*`), with its share of the hand's roll: how far along
    /// the forearm it sits, 0 at the elbow, 1 at the wrist (Dying Light 1: `l_foretwist` at the
    /// wrist, `l_foretwist1` six tenths down). The skin there (and the watch) turns with the hand.
    Twist(usize, f32),
    Hand(usize),
}

/// One arm's upper arm, forearm and wrist elements, and its weapon holder (a helper under the wrist).
#[derive(Clone, Copy)]
struct ArmJoints {
    joints: [i32; 3],
    holder: Option<i32>,
}

/// Both arms of one skeleton and each element's part.
struct Arms {
    skeleton: usize,
    /// Indexed by side.
    sides: [ArmJoints; 2],
    parts: Vec<Part>,
    /// Mirrored bone pairs (left, right) posed alike, for how the skeleton mirrors left against
    /// right ([`mirror_flip`]): the clavicles, thighs and calves (the arms are not posed alike).
    pairs: Vec<[i32; 2]>,
}

/// How far along its forearm (elbow 0, wrist 1) `element` sits, as the skeleton stands now; a
/// twist bone at the wrist when that cannot be read.
fn along_forearm(skeleton: &dyn Skeleton, joints: [i32; 3], element: i32) -> f32 {
    let at = |e: i32| skeleton.world(e).map(|w| position(&w));
    let (Some(elbow), Some(wrist), Some(bone)) = (at(joints[1]), at(joints[2]), at(element)) else { return 1.0 };
    let forearm = [wrist[0] - elbow[0], wrist[1] - elbow[1], wrist[2] - elbow[2]];
    let length2 = forearm.iter().map(|v| v * v).sum::<f32>();
    let along = (0..3).map(|i| (bone[i] - elbow[i]) * forearm[i]).sum::<f32>() / length2;
    if along.is_finite() { along.clamp(0.0, 1.0) } else { 1.0 }
}

/// The skeleton's mirroring (left bone axes = mirror(right bone axes) * flip): declared by the
/// game ([`declare_mirror`]), or else measured once from pairs posed alike and kept for poses where
/// no pair is (a rifle hold).
static FLIP: Mutex<Option<[f32; 9]>> = Mutex::new(None);

/// Declares how the game's skeleton mirrors its left bones from the right ones (row-major 3x3, as
/// the measurement logs it): a fact of the rig, so nothing is measured. A game that starts in a
/// pose without left and right alike (Dishonored 2 with the pistol drawn) never measures it, and
/// its off hand stays where the game animates it.
pub fn declare_mirror(flip: [f32; 9]) {
    *FLIP.lock().unwrap_or_else(|e| e.into_inner()) = Some(flip);
}
/// A measurement of [`FLIP`] and how many updates in a row gave it: it is kept after [`FLIP_AGREEING`].
static FLIP_VOTE: Mutex<([f32; 9], u32)> = Mutex::new(([0.0; 9], 0));
/// Updates in a row whose every posed-alike pair gives the same mirroring before it is kept: a
/// single update mid-animation can give a quarter turn, which would leave the left wrist tilted up
/// for good.
const FLIP_AGREEING: u32 = 10;
/// Each axis of a pair must line up with its mirrored twin this well (cosine; 25 degrees).
const MIRROR_FIT: f32 = 0.9;

static ARMS: Mutex<Option<Arms>> = Mutex::new(None);

impl Arms {
    fn find(skeleton: &dyn Skeleton) -> Option<Self> {
        // Names compared without case: Dying Light 2 spells some `R_Forearm`, `L_HandHolder`.
        let names: Vec<String> = (0..skeleton.count()).map(|e| skeleton.name(e).to_ascii_lowercase()).collect();
        let index = |name: String| names.iter().position(|n| *n == name).map(|i| i as i32);
        let side = |prefix: &str| -> Option<ArmJoints> {
            let joints = [index(format!("{prefix}upperarm"))?, index(format!("{prefix}forearm"))?, index(format!("{prefix}hand"))?];
            Some(ArmJoints { joints, holder: index(format!("{prefix}handholder")) })
        };
        let sides = [side("l_")?, side("r_")?];
        let clavicles = [index("l_clavicle".into()), index("r_clavicle".into())];
        // Each element's part: the nearest of the six joints up its parent chain.
        let parts: Vec<Part> = (0..skeleton.count())
            .map(|element| {
                let mut at = element;
                for _ in 0..64 {
                    for (s, arm) in sides.iter().enumerate() {
                        match arm.joints.iter().position(|&j| j == at) {
                            Some(0) => return Part::UpperArm(s),
                            Some(1) if names[element as usize].contains("twist") => return Part::Twist(s, along_forearm(skeleton, arm.joints, element)),
                            Some(1) => return Part::Forearm(s),
                            Some(_) => return Part::Hand(s),
                            None => {}
                        }
                    }
                    at = skeleton.parent(at);
                    if at < 0 {
                        break;
                    }
                }
                match clavicles.iter().position(|&c| c == Some(element)) {
                    Some(side) => Part::Clavicle(side),
                    None => Part::Body,
                }
            })
            .collect();
        let twists: Vec<String> = parts.iter().zip(&names).filter_map(|(p, n)| if let Part::Twist(_, share) = p { Some(format!("{n} {share:.2}")) } else { None }).collect();
        log!("hand rig: forearm twist bones (share of the hand's roll): {}", if twists.is_empty() { "none".to_string() } else { twists.join(", ") });
        let pairs = ["clavicle", "thigh", "calf"].iter().filter_map(|bone| Some([index(format!("l_{bone}"))?, index(format!("r_{bone}"))?])).collect();
        Some(Self { skeleton: skeleton.id(), sides, parts, pairs })
    }
}

/// The 3x3 rotation of a 3x4 (rows), columns normalised.
fn rotation_of(m: &Mat34) -> [f32; 9] {
    let o = orthonormal(m);
    [o[0], o[1], o[2], o[4], o[5], o[6], o[8], o[9], o[10]]
}

/// How the left bones' axes relate to the right ones', from a mirrored pair of bones posed alike
/// (the clavicles, in the camera's space where the body is mirrored across x = 0): with M the
/// mirror (x negated), left = M right S, so S = rightᵀ M left, rounded to the signed permutation it
/// must be (each column one axis, sign kept). None unless it is a mirror (determinant -1).
fn mirror_flip(right: &[f32; 9], left: &[f32; 9]) -> Option<[f32; 9]> {
    // M left: the first row negated.
    let ml = [-left[0], -left[1], -left[2], left[3], left[4], left[5], left[6], left[7], left[8]];
    let mut s = [0.0f32; 9];
    for row in 0..3 {
        for col in 0..3 {
            s[row * 3 + col] = (0..3).map(|k| right[k * 3 + row] * ml[k * 3 + col]).sum();
        }
    }
    let mut rounded = [0.0f32; 9];
    for col in 0..3 {
        let row = (0..3).max_by(|&a, &b| s[a * 3 + col].abs().total_cmp(&s[b * 3 + col].abs()))?;
        if s[row * 3 + col].abs() < MIRROR_FIT {
            return None;
        }
        rounded[row * 3 + col] = s[row * 3 + col].signum();
    }
    let det = rounded[0] * (rounded[4] * rounded[8] - rounded[5] * rounded[7]) - rounded[1] * (rounded[3] * rounded[8] - rounded[5] * rounded[6])
        + rounded[2] * (rounded[3] * rounded[7] - rounded[4] * rounded[6]);
    (det < -0.5).then_some(rounded)
}

/// The right wrist (`r_hand`) in the palm's frame (origin in the palm, -Z where the hand points,
/// +Y up): the game's own hold of a gun at the hip, which points along its camera as the palm
/// points along the controller's aim. (Pointing along the grip pose's own -Z, the straightened
/// index finger, makes the player bend the wrist up to aim level.) Dying Light 1's hold with the
/// pistol drawn: the wrist's animated frame against the camera, placed so the weapon holder
/// (`r_handholder`, the gun's grip) is the palm's centre, 8 cm ahead of the wrist.
const RIGHT_WRIST_IN_PALM: Mat34 = [
    -0.0780, -0.1988, 0.9769, 0.044, //
    -0.0174, 0.9800, 0.1981, -0.001, //
    -0.9968, -0.0015, -0.0799, 0.083,
];

/// How a game's skeleton orients its bones against Dying Light 1's: each bone's axes are DL1's
/// turned by these (row-major 3x3 rotations, applied in the bone's own frame), so the calibrations
/// measured in DL1 ([`RIGHT_WRIST_IN_PALM`], [`GUN_HOLDER_IN_PALM`]) carry over.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoneAxes {
    /// The right wrist (`r_hand`); the left one follows by the skeleton's mirroring.
    pub wrist: [f32; 9],
    /// The right weapon holder (`r_handholder`).
    pub holder: [f32; 9],
}

impl BoneAxes {
    const IDENTITY: [f32; 9] = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
    pub const DYING_LIGHT_1: Self = Self { wrist: Self::IDENTITY, holder: Self::IDENTITY };
    /// Dying Light 2 and The Beast: the right wrist half turned about its own x (along the
    /// forearm), from the idle melee hold (without it the hands are upside down; The Beast's
    /// matches within about a degree). The gun holder is not calibrated for these games.
    pub const DYING_LIGHT_2: Self = Self { wrist: [1.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, -1.0], holder: Self::IDENTITY };
}

/// `m`'s rotation turned by `turn` in its own frame (`m * turn`), the position kept.
fn turned(m: &Mat34, turn: &[f32; 9]) -> Mat34 {
    let mut out = *m;
    for row in 0..3 {
        for col in 0..3 {
            out[row * 4 + col] = (0..3).map(|k| m[row * 4 + k] * turn[k * 3 + col]).sum();
        }
    }
    out
}

/// The wrist in the palm's frame for `side`, for a skeleton whose right wrist is DL1's turned by
/// `turn`. The left hand is the right one mirrored across the palm's x, its bone axes then turned
/// by the skeleton's own mirroring `flip` ([`mirror_flip`]); without it, the left wrist is not
/// placed.
fn wrist_in_palm(side: usize, flip: Option<&[f32; 9]>, turn: &[f32; 9]) -> Option<Mat34> {
    let right = turned(&orthonormal(&RIGHT_WRIST_IN_PALM), turn);
    if side != LEFT_HAND {
        return Some(right);
    }
    let s = flip?;
    let r = [right[0], right[1], right[2], right[4], right[5], right[6], right[8], right[9], right[10]];
    // M r S, M = diag(-1, 1, 1).
    let mut out = [0.0f32; 12];
    for row in 0..3 {
        for col in 0..3 {
            let v: f32 = (0..3).map(|k| r[row * 3 + k] * s[k * 3 + col]).sum();
            out[row * 4 + col] = if row == 0 { -v } else { v };
        }
    }
    (out[3], out[7], out[11]) = (-right[3], right[7], right[11]);
    Some(out)
}

/// The rotation part made exactly orthonormal (Gram-Schmidt on its columns), the position kept.
fn orthonormal(m: &Mat34) -> Mat34 {
    let column = |c: usize| [m[c], m[4 + c], m[8 + c]];
    let unit = |v: [f32; 3]| {
        let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        [v[0] / l, v[1] / l, v[2] / l]
    };
    let x = unit(column(0));
    let y0 = column(1);
    let d = x[0] * y0[0] + x[1] * y0[1] + x[2] * y0[2];
    let y = unit([y0[0] - d * x[0], y0[1] - d * x[1], y0[2] - d * x[2]]);
    let z = [x[1] * y[2] - x[2] * y[1], x[2] * y[0] - x[0] * y[2], x[0] * y[1] - x[1] * y[0]];
    [x[0], y[0], z[0], m[3], x[1], y[1], z[1], m[7], x[2], y[2], z[2], m[11]]
}

/// A gun's holder (`r_handholder`, where the gun hangs) in the palm's frame: in the palm, turned
/// half round about up, so the gun points where the palm does. Dying Light holds every gun at the
/// hip that way against its camera (a long gun within 1.3 degrees, the pistol within 1.4), but
/// each gun with its own wrist: pinning the wrist (as for melee) leaves the pistol pointing up and
/// right of the long guns' line.
const GUN_HOLDER_IN_PALM: Mat34 = [-1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0];

/// A sword's blade in the palm's frame (x right, y up, -z where the hand aims): a hilt held in the
/// fist runs out of its top, along the controller's grip, half a right angle above the aim (along
/// the aim it would point forward like a rifle). The one direction both a game's
/// weapon lock and its swing reading take for the blade.
pub const MELEE_BLADE: Vec3 = [0.0, std::f32::consts::FRAC_1_SQRT_2, -std::f32::consts::FRAC_1_SQRT_2];
/// The turn about the palm's x that takes the aim (-z) to [`MELEE_BLADE`] (and the palm's up to the
/// edge's way): a weapon calibrated to run along the aim, turned by this, runs along the blade.
pub const MELEE_GRIP: Mat34 = [
    1.0, 0.0, 0.0, 0.0, //
    0.0, std::f32::consts::FRAC_1_SQRT_2, -std::f32::consts::FRAC_1_SQRT_2, 0.0, //
    0.0, std::f32::consts::FRAC_1_SQRT_2, std::f32::consts::FRAC_1_SQRT_2, 0.0,
];

/// A gun's visual class (both Dying Lights); melee weapons are its base, `WeaponVis`.
pub const GUN_VIS_CLASS: &str = ".?AVFireWeaponVis@@";

/// Whether the weapon visual `weapon` is a gun (`FireWeaponVis`, by its vtable once named).
pub fn is_gun(weapon: usize) -> bool {
    static GUN_VTABLE: AtomicU64 = AtomicU64::new(0);
    let Some(vtable) = monaka_hook::mem::read::<usize>(weapon).filter(|_| weapon != 0) else { return false };
    let known = GUN_VTABLE.load(Relaxed);
    if known != 0 {
        return known == vtable as u64;
    }
    let gun = monaka_hook::probe::class_name(weapon).as_deref() == Some(GUN_VIS_CLASS);
    if gun {
        GUN_VTABLE.store(vtable as u64, Relaxed);
    }
    gun
}

/// The game holds what is in the aim hand with both hands when the other wrist sits this close to
/// it (metres, its weapon holder, as animated): a rifle's front hand is about 0.25 m from the grip,
/// a free hand at rest 0.8 m away. The two thresholds and the hold
/// time keep a hand passing by, or an animation's moment, from flipping it.
const TWO_HANDED_WITHIN: f32 = 0.40;
const ONE_HANDED_BEYOND: f32 = 0.55;
const HOLD_SWITCH_MS: u64 = 300;

/// Set by [`declare_one_handed`]: the game never holds anything with both hands.
static ONE_HANDED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Declares that the game holds nothing two-handed (Dishonored: a weapon or power per hand), so the
/// off hand always follows its own controller: with the rule on, a left wrist passing within 0.4 m
/// of the right hand's sword would hand the left arm the right controller's move.
pub fn declare_one_handed() {
    ONE_HANDED.store(true, Relaxed);
}

/// Whether the off hand rides a two-handed weapon (as of the latest update), and since when.
static TWO_HANDED: Mutex<(bool, u64)> = Mutex::new((false, 0));

/// Whether the off hand rides a weapon held two-handed, as of the latest [`follow`]: its fingers
/// then grip as the game animates them.
pub fn off_hand_on_weapon() -> bool {
    TWO_HANDED.lock().is_ok_and(|s| s.0)
}

/// Whether the game holds the weapon with both hands now, from where the off wrist is against the
/// aim hand's holder, debounced.
fn two_handed(distance: f32) -> bool {
    if ONE_HANDED.load(Relaxed) {
        return false;
    }
    let Ok(mut state) = TWO_HANDED.lock() else { return false };
    let now = monaka_channel::tick();
    let wants = if state.0 { distance < ONE_HANDED_BEYOND } else { distance < TWO_HANDED_WITHIN };
    if wants == state.0 {
        state.1 = now;
    } else if now.saturating_sub(state.1) >= HOLD_SWITCH_MS {
        *state = (wants, now);
        note_two_handed(wants, distance);
    }
    state.0
}

fn note_two_handed(both: bool, distance: f32) {
    static SWITCHES: AtomicU64 = AtomicU64::new(0);
    if SWITCHES.fetch_add(1, Relaxed) < 40 {
        log!("hand rig: {} (off wrist {distance:.2} m from the weapon)", if both { "two-handed: the off hand rides the weapon" } else { "one-handed: the off hand follows its own controller" });
    }
}

/// How much of the elbow's bend direction comes from the hand's pose ([`monaka_core::reach::hand_pole`]);
/// the rest is the default, down and out.
const ELBOW_FROM_HAND: f32 = 0.75;

fn unit(v: Vec3) -> Option<Vec3> {
    let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    (length.is_finite() && length > 1e-6).then(|| v.map(|x| x / length))
}

fn position(m: &Mat34) -> [f32; 3] {
    [m[3], m[7], m[11]]
}

/// What one update puts the arms on.
pub struct Pose {
    /// The player camera as the game set it (camera-to-world).
    pub game: Mat34,
    /// Where the headset's tracking space sits in the world: the same origin the view is drawn
    /// from, so a hand lands where its controller is seen.
    pub origin: Mat34,
    pub head: HeadPose,
    /// Each controller's palm in tracking space (`HandPose::palm`), by side, when tracked.
    pub palms: [Option<HandPose>; 2],
    /// The controller that aims (`LEFT_HAND` or `RIGHT_HAND`).
    pub aim_side: usize,
    /// The wrist moved this far in the palm's space (metres; x right, y up, z back).
    pub hold: [f32; 3],
    /// A gun is drawn (it sits in the aim palm by its grip).
    pub gun: bool,
    /// A melee weapon locked to the aim controller: its holder (`r_handholder`) in the palm's frame
    /// (the game's own bone axes). The hand goes around it as the game animates the grip, and the
    /// game's guards no longer turn the weapon in the hand (Kingdom Come II re-grips the sword at
    /// every guard change). None: the wrist is pinned and the game holds the weapon.
    pub melee_holder: Option<Mat34>,
    /// The game's bone axes against DL1's.
    pub bones: BoneAxes,
}

/// What [`follow`] placed, for a producer's diagnostics.
#[derive(Clone, Copy)]
pub struct Placed {
    /// The aim palm in the world.
    pub palm: Mat34,
}

/// Puts the arms, as animated (`before`), on the controllers, each upper arm and forearm reaching
/// its hand from a shoulder under the real head. The aim hand: with a gun, the gun in the palm
/// pointing where the controller aims; otherwise the wrist in the palm. The off hand: on the weapon
/// as the game holds it when it holds it two-handed; otherwise its wrist in the off controller's
/// palm; carried with the body when that controller is not tracked. Every bone is written: setting
/// one marks its children for recomputation, and the ones the game set itself must lose its own
/// adjustments (Dying Light 1 squashes the whole rig toward the camera for the flat screen; left to
/// the game, the body stays flattened front to back). Err (with why) when there is no aim
/// controller or no arm to move.
pub fn follow(skeleton: &dyn Skeleton, before: &Snapshot, pose: &Pose) -> Result<Placed, &'static str> {
    // Poses that are not a tracked hand or head (not finite, no rotation, kilometres out) are
    // taken as untracked: placed on them, the bones would go wherever they said.
    let palms = pose.palms.map(|p| p.filter(plausible_hand));
    let Some(hand) = palms.get(pose.aim_side).copied().flatten() else { return Err("no controller") };
    let (aim_side, off_side) = (pose.aim_side.min(1), 1 - pose.aim_side.min(1));
    if before.skeleton != skeleton.id() || skeleton.count() as usize != before.worlds.len() {
        return Err("skeleton changed");
    }
    // The arms as the game animated them. A game that did not animate them again since the last
    // move (paused, or calling twice in one frame) hands back what was written: moved again, the
    // move would pile up every call until the wrists were far out and the arms flickered. Then the
    // bones are put back exactly as written, nothing solved: solving on through a pause from the
    // last animation ends in an arm that cannot reach its hand, and Dying Light 1 then reads a null
    // in a nearest-of-a-list search that finds nothing for a position not finite or far out.
    let Ok(mut memory) = MEMORY.lock() else { return Err("lock") };
    let Memory { skeleton: remembered, animated, written, palm: last_palm } = &mut *memory;
    // Within a sway of it counts too: holding a rifle, the game's own small motion (its sway) goes
    // on in the pause menu, on top of the bones as written; taken for an animation, the arms would
    // be moved again every call and fly off along the look.
    let same = *remembered == before.skeleton && written.len() == before.worlds.len() && written.iter().any(|w| w[0].is_finite());
    let apart = same.then(|| written_apart(written, &before.worlds));
    let ours = apart.is_some_and(|(turn, shift)| turn <= SWAY_TURN && shift <= SWAY_SHIFT);
    if let Some((turn, shift)) = apart.filter(|&(turn, shift)| ours && (turn > 0.0 || shift > 0.0)) {
        note_swayed(turn, shift);
    }
    if !ours && same {
        note_mostly_ours(skeleton, written, &before.worlds);
    }
    if ours {
        note_reanimation_missed();
        for (element, placed) in written.iter().enumerate().filter(|(_, w)| w[0].is_finite()) {
            skeleton.set_world(element as i32, placed);
        }
        return Ok(Placed { palm: *last_palm });
    }
    // Arms handed back nowhere near the camera are not the player's arms in play (a cutscene, a
    // load): left to the game.
    if !near(&before.worlds, &pose.game) {
        return Err("arms far from the camera");
    }
    *remembered = before.skeleton;
    animated.clear();
    animated.extend_from_slice(&before.worlds);
    let animated: &[Mat34] = animated;
    let Ok(mut arms) = ARMS.lock() else { return Err("lock") };
    if !arms.as_ref().is_some_and(|a| a.skeleton == before.skeleton && a.parts.len() == before.worlds.len()) {
        *arms = Arms::find(skeleton);
        if arms.is_none() {
            log_first_missing_arm();
        }
    }
    let Some(arms) = arms.as_mut() else { return Err("no arm") };
    let world = |element: i32| &animated[element as usize];
    let game = pose.game;
    let view = rigid_inverse(&game);
    let flip = {
        let Ok(mut known) = FLIP.lock() else { return Err("lock") };
        if known.is_none() {
            let readings: Vec<Option<[f32; 9]>> = arms.pairs.iter().map(|&[l, r]| mirror_flip(&rotation_of(&compose(&view, world(r))), &rotation_of(&compose(&view, world(l))))).collect();
            // Pairs not posed alike (legs mid-stride) read nothing; at least two must read, alike.
            let read: Vec<[f32; 9]> = readings.iter().flatten().copied().collect();
            let agreed = read.first().copied().filter(|first| read.len() >= 2 && read.iter().all(|r| r == first));
            if let Ok(mut vote) = FLIP_VOTE.lock() {
                *vote = match agreed {
                    Some(flip) if vote.0 == flip => (flip, vote.1 + 1),
                    Some(flip) => (flip, 1),
                    None => ([0.0; 9], 0),
                };
                if vote.1 >= FLIP_AGREEING {
                    log!("hand rig: left bones mirror the right ones with {:?} ({} of {} pairs agreeing, {FLIP_AGREEING} updates running)", vote.0, read.len(), readings.len());
                    *known = Some(vote.0);
                }
            }
        }
        *known
    };
    // Everything under a wrist moves with what is pinned.
    let origin = pose.origin;
    let palm = compose(&apply_head(&origin, hand.orientation, hand.position), &translation(pose.hold));
    let aim = arms.sides[aim_side];
    melee::note_animated_wrist(position(&compose(&view, world(aim.joints[2]))));
    // The rig's share of the arms: none while the game's animation climbs or hangs with them.
    let rig = traverse::update([LEFT_HAND, RIGHT_HAND].map(|side| position(&compose(&view, world(arms.sides[side].joints[2])))), &game);
    let holder = aim.holder.filter(|_| aim_side == RIGHT_HAND);
    let held = match (holder, pose.gun, pose.melee_holder) {
        (Some(holder), true, _) => Some((holder, turned(&GUN_HOLDER_IN_PALM, &pose.bones.holder), Hold::Gun)),
        (Some(holder), false, Some(in_palm)) => Some((holder, in_palm, Hold::Melee)),
        _ => None,
    };
    note_hold(held.map_or(Hold::Wrist, |h| h.2));
    let (pinned, in_palm) = match held {
        Some((holder, in_palm, _)) => (world(holder), in_palm),
        None => (world(aim.joints[2]), wrist_in_palm(aim_side, flip.as_ref(), &pose.bones.wrist).ok_or("no wrist calibration")?),
    };
    // From the rigid part of what the game animated: a scaled wrist inverted as if rigid would make
    // the move scale whatever it moved (the hands several times their size).
    note_scaled("aim wrist", pinned);
    let aim_move = compose(&compose(&palm, &in_palm), &rigid_inverse(&orthonormal(pinned)));
    if !aim_move.iter().all(|v| v.is_finite()) {
        static LOGGED: AtomicU64 = AtomicU64::new(0);
        if LOGGED.fetch_add(1, Relaxed) < 3 {
            log!("hand rig: not finite: palm {palm:?} pinned {pinned:?} game {game:?} hand {hand:?}");
        }
        return Err("not finite");
    }
    // The off hand: on the weapon when the game holds it two-handed, else on its own controller.
    let off = arms.sides[off_side];
    let anchor = position(world(aim.holder.unwrap_or(aim.joints[2])));
    let off_wrist = position(world(off.joints[2]));
    let both_hands = two_handed(monaka_core::math::distance(anchor, off_wrist));
    let off_move = if both_hands {
        Some(aim_move)
    } else {
        palms[off_side].and_then(|off_hand| {
            let off_palm = compose(&apply_head(&origin, off_hand.orientation, off_hand.position), &translation(pose.hold));
            let in_palm = wrist_in_palm(off_side, flip.as_ref(), &pose.bones.wrist)?;
            note_scaled("off wrist", world(off.joints[2]));
            Some(compose(&compose(&off_palm, &in_palm), &rigid_inverse(&orthonormal(world(off.joints[2])))))
        })
    }
    .filter(|m| m.iter().all(|v| v.is_finite()));
    // The shoulders where the game animates them against its levelled camera, carried under the
    // real head (yaw only: nodding does not swing the shoulders).
    let Some(flat_game) = cam::level(&game) else { return Err("camera not levelled") };
    let head = pose.head;
    let body = if head.valid != 0 && plausible_head(&head) { apply_head(&origin, yaw_rotation(yaw(head.orientation)), head.position) } else { flat_game };
    let carry = compose(&body, &rigid_inverse(&flat_game));
    let mut moves: [Option<[Mat34; 3]>; 2] = [None, None];
    // Each forearm's elbow, its axis toward the wrist and how far the hand rolls about it.
    let mut twists: [Option<(Vec3, Vec3, f32)>; 2] = [None, None];
    for (side, hand_move) in [(aim_side, Some(aim_move)), (off_side, off_move)] {
        let Some(hand_move) = hand_move else { continue };
        let arm = arms.sides[side];
        let joint = |i: usize| position(world(arm.joints[i]));
        let target = apply(&hand_move, joint(2));
        let shoulder = apply(&carry, joint(0));
        // The elbow bends down and out to the arm's side; a hand on a two-handed weapon's front
        // keeps its elbow down under it.
        let out = match side {
            _ if side == off_side && both_hands => 0.0,
            LEFT_HAND => -0.6,
            _ => 0.6,
        };
        let base = [body[0] * out, body[4] * out - 1.0, body[8] * out];
        // Mostly toward where the hand's own pose puts the forearm (a hand turned to read a watch
        // lifts the elbow out and up), with the default keeping it steady; not for a hand on a
        // two-handed weapon's front, whose controller is elsewhere.
        let forearm = monaka_core::math::distance(joint(1), joint(2));
        let hand_pole = palms[side]
            .filter(|_| side == aim_side || !both_hands)
            .and_then(|h| monaka_core::reach::hand_pole(shoulder, target, &apply_head(&origin, h.orientation, h.position), forearm));
        let pole = match (hand_pole, unit(base)) {
            (Some(hand), Some(base)) => [0, 1, 2].map(|i| hand[i] * ELBOW_FROM_HAND + base[i] * (1.0 - ELBOW_FROM_HAND)),
            _ => base,
        };
        // An arm the reach cannot solve this update (a degenerate animated pose) keeps its last
        // solution for a moment, then goes rigidly with the hand: skipping the move would draw the
        // arm where the game put it for that frame, a flicker.
        let [upper, lower] = match arm_moves([joint(0), joint(1), joint(2)], shoulder, target, pole) {
            Some(solved) => {
                if let Ok(mut last) = LAST_REACH.lock() {
                    last[side] = Some((solved, monaka_channel::tick()));
                }
                solved
            }
            None => {
                note_reach_failure();
                let recent = LAST_REACH.lock().ok().and_then(|last| last[side]).filter(|(_, at)| monaka_channel::tick().saturating_sub(*at) < 250);
                recent.map_or([hand_move; 2], |(solved, _)| solved)
            }
        };
        moves[side] = Some([upper, lower, hand_move]);
        // The hand's roll about the forearm beyond the animation's, for the twist bones.
        let elbow_at = apply(&lower, joint(1));
        let axis = [target[0] - elbow_at[0], target[1] - elbow_at[1], target[2] - elbow_at[2]];
        twists[side] = twist_between(&lower, &hand_move, axis).map(|angle| (elbow_at, axis, angle));
    }
    // Every bone from the snapshot. Elements that are not bones stay: the helpers (`r_handholder`,
    // which the weapon hangs on, ...) follow their moved bones, and moving the helper `hspine1`
    // (the root Dying Light 1 squashes) or the mesh parts (`player_6_handr_fpp`, ...) leaves the
    // arms where the game put them, out of view, with only the weapon at the controller.
    // Climbing, every bone goes where the game animated it in the world, blended: the hands stay
    // on the ledge. (Carried under the head, the arms would turn with the player's sight while the
    // game, left alone on a ledge, does not turn the character: the hands would come off it.)
    let in_place = translation([0.0; 3]);
    let pivot = position(&game);
    let body_move = blend(&in_place, &carry, rig, pivot);
    let arm = |side: usize, part: usize| moves[side].map_or(body_move, |m| blend(&in_place, &m[part], rig, pivot));
    let mut placements = Vec::with_capacity(animated.len());
    for (element, world) in (0..skeleton.count()).zip(animated) {
        let mover = match arms.parts[element as usize] {
            Part::Body | Part::Clavicle(_) => body_move,
            Part::UpperArm(side) => arm(side, 0),
            Part::Forearm(side) => arm(side, 1),
            Part::Twist(side, share) => match (moves[side], twists[side]) {
                (Some(m), Some((elbow, axis, angle))) => {
                    let twisted = spin_about(elbow, axis, angle * share).map_or(m[1], |spin| compose(&spin, &m[1]));
                    blend(&in_place, &twisted, rig, pivot)
                }
                _ => arm(side, 1),
            },
            Part::Hand(side) => arm(side, 2),
        };
        // A bone the game left not finite stays as it is (a Dying Light 2 glove after a cutscene).
        if world.iter().all(|v| v.is_finite()) && skeleton.is_bone(element) {
            placements.push((element, compose(&mover, world), world));
        }
    }
    // All or nothing: a skeleton half moved is torn, and one bone far out is enough for the game
    // to look up nothing near it.
    if let Some(&(element, placed, _)) = placements.iter().find(|(_, placed, world)| !plausible_placement(placed, world, &game)) {
        note_implausible(&skeleton.name(element), &placed, &game);
        return Err("implausible placement");
    }
    written.clear();
    written.resize(animated.len(), [f32::NAN; 12]);
    for (element, placed, _) in placements {
        skeleton.set_world(element, &placed);
        written[element as usize] = placed;
    }
    *last_palm = palm;
    Ok(Placed { palm })
}

/// A controller pose that can be a tracked hand: finite, a rotation, within a room.
fn plausible_hand(hand: &HandPose) -> bool {
    plausible_pose(hand.orientation, hand.position)
}

fn plausible_head(head: &HeadPose) -> bool {
    plausible_pose(head.orientation, head.position)
}

/// Tracking space is the room: nothing tracked is further than this from its origin (metres).
const ROOM: f32 = 10.0;

fn plausible_pose(orientation: [f32; 4], position: [f32; 3]) -> bool {
    let norm = orientation.iter().map(|v| v * v).sum::<f32>().sqrt();
    let distance = position.iter().map(|v| v * v).sum::<f32>().sqrt();
    (0.9..1.1).contains(&norm) && distance < ROOM
}

/// A placed bone is never further than this from the camera (metres): the arms reach about one,
/// the feet stand under it, and a climb backs the view off the wall.
const BONE_REACH: f32 = 4.0;

/// A bone placed from `world` (as animated) by a rigid move: finite, near the camera `game`, and
/// scaled as the animation scaled it (column lengths, within half and double).
fn plausible_placement(placed: &Mat34, world: &Mat34, game: &Mat34) -> bool {
    let column = |m: &Mat34, c: usize| (m[c] * m[c] + m[4 + c] * m[4 + c] + m[8 + c] * m[8 + c]).sqrt();
    placed.iter().all(|v| v.is_finite())
        && monaka_core::math::distance(position(placed), position(game)) < BONE_REACH
        && (0..3).all(|c| {
            let ratio = column(placed, c) / column(world, c);
            (0.5..2.0).contains(&ratio)
        })
}

fn note_implausible(name: &str, placed: &Mat34, game: &Mat34) {
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let n = COUNT.fetch_add(1, Relaxed) + 1;
    if n <= 5 || n.is_power_of_two() {
        log!("hand rig: {n}x a bone would have been placed implausibly; nothing written (first: {name} at {placed:?}, camera {game:?})");
    }
}

/// The arms the game last animated, the bones as [`follow`] last wrote them (NaN for the rest) and
/// the aim palm they were put on.
struct Memory {
    skeleton: usize,
    animated: Vec<Mat34>,
    written: Vec<Mat34>,
    palm: Mat34,
}

static MEMORY: Mutex<Memory> = Mutex::new(Memory { skeleton: 0, animated: Vec::new(), written: Vec::new(), palm: [0.0; 12] });

/// Arms handed back further than this from the camera are not animated arms (metres).
const ARMS_REACH: f32 = 20.0;

/// Whether every readable element is within [`ARMS_REACH`] of the camera `game`.
fn near(worlds: &[Mat34], game: &Mat34) -> bool {
    let camera = position(game);
    worlds.iter().filter(|w| w[0].is_finite()).all(|w| monaka_core::math::distance(position(w), camera) < ARMS_REACH)
}

/// Logs the first few animated wrists that are not rigid (column lengths off 1 by more than 5%).
fn note_scaled(what: &str, m: &Mat34) {
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let lengths: [f32; 3] = std::array::from_fn(|c| (m[c] * m[c] + m[4 + c] * m[4 + c] + m[8 + c] * m[8 + c]).sqrt());
    if lengths.iter().any(|l| (l - 1.0).abs() > 0.05) {
        let n = COUNT.fetch_add(1, Relaxed) + 1;
        if n <= 5 || n.is_power_of_two() {
            log!("hand rig: {n}x an animated wrist not rigid ({what}: axes {lengths:.2?})");
        }
    }
}

/// Records a bone written after [`follow`] placed `skeleton` (a finger put on finger tracking), so
/// a game that hands back the same pose is still known to hand back the rig's own: unrecorded, a
/// paused game's arms would no longer match what [`follow`] wrote, be taken as animated and moved
/// again every call, and the bones would fly.
pub fn note_written(skeleton: usize, element: i32, matrix: &Mat34) {
    let Ok(mut memory) = MEMORY.lock() else { return };
    if memory.skeleton == skeleton
        && let Some(slot) = usize::try_from(element).ok().and_then(|e| memory.written.get_mut(e))
    {
        *slot = *matrix;
    }
}

/// Bones handed back within this of what was written (largest rotation entry; metres) are the
/// rig's own, swayed by the game: a fresh animation puts the arms back in the game's pose, far
/// from the controllers'.
const SWAY_TURN: f32 = 0.02;
const SWAY_SHIFT: f32 = 0.01;

/// How far the bones handed back are from what was written, over the bones written: the largest
/// difference of a rotation entry, and of a position (metres).
fn written_apart(written: &[Mat34], handed: &[Mat34]) -> (f32, f32) {
    let mut apart = (0.0f32, 0.0f32);
    for (w, h) in written.iter().zip(handed).filter(|(w, _)| w[0].is_finite()) {
        for (i, (a, b)) in w.iter().zip(h).enumerate() {
            let d = (a - b).abs();
            let d = if d.is_nan() { f32::INFINITY } else { d };
            if i % 4 == 3 {
                apart.1 = apart.1.max(d);
            } else {
                apart.0 = apart.0.max(d);
            }
        }
    }
    apart
}

fn note_swayed(turn: f32, shift: f32) {
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let n = COUNT.fetch_add(1, Relaxed) + 1;
    if n <= 5 || n.is_power_of_two() {
        log!("hand rig: the arms came back swayed off what was written ({turn:.4} rotation, {:.1} mm) {n}x; put back as written", shift * 1000.0);
    }
}

/// Diagnostic: arms handed back mostly as written, a few bones not: the game changed only those
/// since the last move (paused, something of its own still at work), which defeats the check for
/// arms not animated again. Logs which bones and how far.
fn note_mostly_ours(skeleton: &dyn Skeleton, written: &[Mat34], handed: &[Mat34]) {
    static LOGGED: AtomicU64 = AtomicU64::new(0);
    let finite = written.iter().filter(|w| w[0].is_finite()).count();
    let changed: Vec<(usize, f32)> = written
        .iter()
        .zip(handed)
        .enumerate()
        .filter(|(_, (w, _))| w[0].is_finite())
        .filter_map(|(element, (w, h))| {
            let apart = w.iter().zip(h).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
            (apart > 0.0).then_some((element, apart))
        })
        .collect();
    // A fresh animation changes nearly every bone; a handful changed is the game touching up ours.
    if changed.is_empty() || changed.len() * 5 > finite || LOGGED.fetch_add(1, Relaxed) >= 12 {
        return;
    }
    let shown: Vec<String> = changed.iter().take(8).map(|&(e, apart)| format!("{} {apart:.3}", skeleton.name(e as i32))).collect();
    log!("hand rig: the arms came back as written but for {} of {finite} bones: {}", changed.len(), shown.join(", "));
}

/// Counts the calls that found the arms not animated again since they were moved (paused, or a
/// second call in one frame); logs the first few.
fn note_reanimation_missed() {
    static MISSED: AtomicU64 = AtomicU64::new(0);
    let n = MISSED.fetch_add(1, Relaxed) + 1;
    if n <= 3 || n.is_power_of_two() {
        log!("hand rig: the game had not animated the arms since they were moved {n}x; put back as written");
    }
}

/// `weight` of the way from one rigid move to another (0 `from`, 1 `to`): the matrices mixed and
/// made rigid again about `pivot`, which goes where the mix takes it. Linear in what it does to a
/// point, so an elbow both arm bones move alike stays joined, but for that last rounding, which
/// grows with the distance from `pivot`: made rigid about the world's origin, hundreds of metres
/// off in a game's map, it would throw the arms tens of metres on every climb's blend.
fn blend(from: &Mat34, to: &Mat34, weight: f32, pivot: [f32; 3]) -> Mat34 {
    if weight >= 1.0 {
        return *to;
    }
    if weight <= 0.0 {
        return *from;
    }
    let mixed: Mat34 = std::array::from_fn(|i| from[i] + (to[i] - from[i]) * weight);
    let mut rigid = orthonormal(&mixed);
    let (target, turned) = (apply(&mixed, pivot), apply(&rigid, pivot));
    for (row, (t, r)) in target.iter().zip(turned).enumerate() {
        rigid[row * 4 + 3] += t - r;
    }
    match rigid.iter().all(|v| v.is_finite()) {
        true => rigid,
        false if weight < 0.5 => *from,
        false => *to,
    }
}

/// An arm's last solved upper-arm and forearm moves and when (`monaka_channel::tick`).
type Reach = Option<([Mat34; 2], u64)>;
/// Each arm's, by side.
static LAST_REACH: Mutex<[Reach; 2]> = Mutex::new([None, None]);
static REACH_FAILURES: AtomicU64 = AtomicU64::new(0);

fn note_reach_failure() {
    let n = REACH_FAILURES.fetch_add(1, Relaxed) + 1;
    if n <= 3 || n.is_power_of_two() {
        log!("hand rig: the arm could not reach the hand {n}x (kept the last reach or moved it with the hand)");
    }
}

/// What the aim palm holds on to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Hold {
    Wrist = 1,
    Gun = 2,
    Melee = 3,
}

/// Logs switches between what the aim palm holds on to (the first few).
fn note_hold(hold: Hold) {
    static STATE: AtomicU64 = AtomicU64::new(0);
    static SWITCHES: AtomicU64 = AtomicU64::new(0);
    let now = hold as u64;
    if STATE.swap(now, Relaxed) != now && SWITCHES.fetch_add(1, Relaxed) < 40 {
        log!(
            "hand rig: {}",
            match hold {
                Hold::Gun => "a gun, held by its grip in the palm",
                Hold::Melee => "a melee weapon locked to the controller by its holder",
                Hold::Wrist => "no gun, the wrist in the palm",
            }
        );
    }
}

fn log_first_missing_arm() {
    static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !LOGGED.swap(true, Relaxed) {
        log!("hand rig: the skeleton lacks an upperarm/forearm/hand element; arms left to the game");
    }
}

/// End-of-run tally (the producer logs its own call counts).
pub fn report() {
    let failures = REACH_FAILURES.load(Relaxed);
    if failures > 0 {
        log!("  arm reach failed {failures}x (covered)");
    }
    traverse::report();
    melee::report();
}

#[cfg(test)]
mod tests {
    use super::*;
    use monaka_core::camera::{distance, is_rigid};

    #[test]
    fn the_grip_turns_the_aim_to_the_blade() {
        assert!(is_rigid(&MELEE_GRIP, 1e-5));
        let along = apply(&MELEE_GRIP, [0.0, 0.0, -1.0]);
        assert!(along.iter().zip(MELEE_BLADE).all(|(a, b)| (a - b).abs() < 1e-6), "{along:?}");
    }

    #[test]
    fn a_sway_on_the_written_bones_is_told_from_a_fresh_pose() {
        let written = vec![[1.0, 0.0, 0.0, 340.0, 0.0, 1.0, 0.0, 60.0, 0.0, 0.0, 1.0, 210.0], [f32::NAN; 12]];
        // Not written: whatever the game has there does not count.
        let mut handed = vec![written[0], [5.0; 12]];
        assert_eq!(written_apart(&written, &handed), (0.0, 0.0));
        handed[0][3] += 0.004;
        handed[0][1] += 0.01;
        let (turn, shift) = written_apart(&written, &handed);
        assert!(turn <= SWAY_TURN && shift <= SWAY_SHIFT, "a sway: {turn} {shift}");
        handed[0][11] -= 0.15;
        assert!(written_apart(&written, &handed).1 > SWAY_SHIFT, "the game's own pose, 15 cm off");
        handed[0][0] = f32::NAN;
        assert_eq!(written_apart(&written, &handed).0, f32::INFINITY, "not finite is never ours");
    }

    #[test]
    fn the_wrist_calibration_is_rigid_and_mirrors() {
        let identity = BoneAxes::DYING_LIGHT_1.wrist;
        let right = wrist_in_palm(RIGHT_HAND, None, &identity).unwrap();
        assert!(is_rigid(&right, 1e-4));
        assert!(distance(&right, &RIGHT_WRIST_IN_PALM) < 0.01, "only rounding is corrected");
        // The right wrist's +X runs forward along the fingers (-Z of the palm).
        assert!(right[8] < -0.9, "{right:?}");
        assert!(wrist_in_palm(LEFT_HAND, None, &identity).is_none(), "no left wrist without the skeleton's mirroring");
        // A skeleton whose left bones flip their z axis.
        let flip = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, -1.0];
        let left = wrist_in_palm(LEFT_HAND, Some(&flip), &identity).unwrap();
        assert!(is_rigid(&left, 1e-4), "{left:?}");
        assert_eq!((left[3], left[7], left[11]), (-right[3], right[7], right[11]));
        // Mirrored: the left wrist's +X still runs forward, its z axis the right one's mirrored and flipped.
        assert!((left[8] - right[8]).abs() < 1e-6 && (left[0] + right[0]).abs() < 1e-6);
        assert!((left[2] - right[2]).abs() < 1e-6, "x of z: negated twice");
    }

    #[test]
    fn a_turned_skeleton_turns_the_wrist_in_its_own_frame() {
        // Dying Light 2's wrist: DL1's half turned about its own x (along the forearm).
        let turn = [1.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, -1.0];
        let dl1 = wrist_in_palm(RIGHT_HAND, None, &BoneAxes::DYING_LIGHT_1.wrist).unwrap();
        let dl2 = wrist_in_palm(RIGHT_HAND, None, &turn).unwrap();
        assert!(is_rigid(&dl2, 1e-4));
        for row in 0..3 {
            assert_eq!(dl2[row * 4], dl1[row * 4], "x kept");
            assert_eq!((dl2[row * 4 + 1], dl2[row * 4 + 2]), (-dl1[row * 4 + 1], -dl1[row * 4 + 2]), "y and z flipped");
            assert_eq!(dl2[row * 4 + 3], dl1[row * 4 + 3], "the wrist stays where it was");
        }
    }

    #[test]
    fn mirror_flip_reads_the_skeleton() {
        // A right bone turned 30 degrees about y; its left twin mirrored across x with z flipped.
        let (c, s) = (0.866_025_4f32, 0.5f32);
        let right = [c, 0.0, s, 0.0, 1.0, 0.0, -s, 0.0, c];
        let flip = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, -1.0];
        let mut left = [0.0f32; 9];
        for row in 0..3 {
            for col in 0..3 {
                let v: f32 = (0..3).map(|k| right[row * 3 + k] * flip[k * 3 + col]).sum();
                left[row * 3 + col] = if row == 0 { -v } else { v };
            }
        }
        assert_eq!(mirror_flip(&right, &left), Some(flip));
    }

    #[test]
    fn a_blend_far_from_the_origin_keeps_the_bones_near() {
        // Half way from staying put to a quarter turn about up, 460 m out: a bone a metre from the
        // pivot lands a metre from it, not metres away.
        let pivot = [460.0, 50.0, 190.0];
        let turn = [0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0];
        let about_pivot = compose(&compose(&translation(pivot), &turn), &translation([-pivot[0], -pivot[1], -pivot[2]]));
        let half = blend(&translation([0.0; 3]), &about_pivot, 0.5, pivot);
        let bone = apply(&half, [461.0, 50.0, 190.0]);
        assert!((monaka_core::math::distance(bone, pivot) - 1.0).abs() < 1e-3, "{bone:?}");
        assert!(monaka_core::math::distance(apply(&half, pivot), pivot) < 1e-3);
    }

    #[test]
    fn only_plausible_poses_and_placements_pass() {
        let level = [0.0, 0.0, 0.0, 1.0];
        assert!(plausible_pose(level, [0.3, 1.2, -0.4]));
        assert!(!plausible_pose([0.0; 4], [0.3, 1.2, -0.4]), "no rotation");
        assert!(!plausible_pose(level, [f32::NAN, 0.0, 0.0]));
        assert!(!plausible_pose(level, [0.0, 1e20, 0.0]), "far out");
        let camera = [1.0, 0.0, 0.0, 10.0, 0.0, 1.0, 0.0, 2.0, 0.0, 0.0, 1.0, -5.0];
        let bone = [0.0, -1.0, 0.0, 10.3, 1.0, 0.0, 0.0, 1.5, 0.0, 0.0, 1.0, -5.4];
        assert!(plausible_placement(&bone, &bone, &camera));
        let far = [0.0, -1.0, 0.0, 10.3, 1.0, 0.0, 0.0, 1.5, 0.0, 0.0, 1.0, 400.0];
        assert!(!plausible_placement(&far, &bone, &camera));
        let flat = [0.0, -1.0, 0.0, 10.3, 0.0, 0.0, 0.0, 1.5, 0.0, 0.0, 1.0, -5.4];
        assert!(!plausible_placement(&flat, &bone, &camera), "a column squashed to nothing");
    }
}
