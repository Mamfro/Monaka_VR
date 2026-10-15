//! The arms found on a skeleton of named elements (each element's part: body, clavicle, upper arm,
//! forearm, twist bone, hand; [`Arms`]), how its left bones mirror the right ones ([`FLIP`]), and
//! the hands' calibrations: the wrist in the palm ([`wrist_in_palm`]), a gun's holder in it, a
//! melee weapon's blade along it ([`MELEE_BLADE`]), each skeleton's bone axes against Dying Light
//! 1's ([`BoneAxes`]).

use crate::skeleton::Skeleton;
use monaka_core::camera::{Mat34, position};
use monaka_core::math::Vec3;
use monaka_contract::protocol::LEFT_HAND;
use monaka_core::sync::Locked;
use monaka_producer::log;
use std::sync::Mutex;

/// Which arm part each element belongs to (with the arm's side, `LEFT_HAND` or `RIGHT_HAND`).
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Part {
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
pub(super) struct ArmJoints {
    pub(super) joints: [i32; 3],
    pub(super) holder: Option<i32>,
}

/// Both arms of one skeleton and each element's part.
pub(super) struct Arms {
    pub(super) skeleton: usize,
    /// Indexed by side.
    pub(super) sides: [ArmJoints; 2],
    pub(super) parts: Vec<Part>,
    /// Mirrored bone pairs (left, right) posed alike, for how the skeleton mirrors left against
    /// right ([`mirror_flip`]): the clavicles, thighs and calves (the arms are not posed alike).
    pub(super) pairs: Vec<[i32; 2]>,
}

/// How far along its forearm (elbow 0, wrist 1) `element` sits, as the skeleton stands now; a
/// twist bone at the wrist when that cannot be read.
pub(super) fn along_forearm(skeleton: &dyn Skeleton, joints: [i32; 3], element: i32) -> f32 {
    let at = |e: i32| skeleton.world(e).map(|w| position(&w));
    let (Some(elbow), Some(wrist), Some(bone)) = (at(joints[1]), at(joints[2]), at(element)) else { return 1.0 };
    monaka_core::math::segment_share(bone, elbow, wrist).map_or(1.0, |along| along.clamp(0.0, 1.0))
}

/// The skeleton's mirroring (left bone axes = mirror(right bone axes) * flip): declared by the
/// game ([`declare_mirror`]), or else measured once from pairs posed alike and kept for poses where
/// no pair is (a rifle hold).
pub(super) static FLIP: Mutex<Option<[f32; 9]>> = Mutex::new(None);

/// Declares how the game's skeleton mirrors its left bones from the right ones (row-major 3x3, as
/// the measurement logs it): a fact of the rig, so nothing is measured. A game that starts in a
/// pose without left and right alike (Dishonored 2 with the pistol drawn) never measures it, and
/// its off hand stays where the game animates it.
pub fn declare_mirror(flip: [f32; 9]) {
    *FLIP.locked() = Some(flip);
}
/// A measurement of [`FLIP`] and how many updates in a row gave it: it is kept after [`FLIP_AGREEING`].
pub(super) static FLIP_VOTE: Mutex<([f32; 9], u32)> = Mutex::new(([0.0; 9], 0));
/// Updates in a row whose every posed-alike pair gives the same mirroring before it is kept: a
/// single update mid-animation can give a quarter turn, which would leave the left wrist tilted up
/// for good.
pub(super) const FLIP_AGREEING: u32 = 10;
/// Each axis of a pair must line up with its mirrored twin this well (cosine; 25 degrees).
pub(super) const MIRROR_FIT: f32 = 0.9;

pub(super) static ARMS: Mutex<Option<Arms>> = Mutex::new(None);

impl Arms {
    pub(super) fn find(skeleton: &dyn Skeleton) -> Option<Self> {
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
pub(super) fn rotation_of(m: &Mat34) -> [f32; 9] {
    let o = orthonormal(m);
    [o[0], o[1], o[2], o[4], o[5], o[6], o[8], o[9], o[10]]
}

/// How the left bones' axes relate to the right ones', from a mirrored pair of bones posed alike
/// (the clavicles, in the camera's space where the body is mirrored across x = 0): with M the
/// mirror (x negated), left = M right S, so S = rightᵀ M left, rounded to the signed permutation it
/// must be (each column one axis, sign kept). None unless it is a mirror (determinant -1).
pub(super) fn mirror_flip(right: &[f32; 9], left: &[f32; 9]) -> Option<[f32; 9]> {
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
pub(super) const RIGHT_WRIST_IN_PALM: Mat34 = [
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
pub(super) fn turned(m: &Mat34, turn: &[f32; 9]) -> Mat34 {
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
pub(super) fn wrist_in_palm(side: usize, flip: Option<&[f32; 9]>, turn: &[f32; 9]) -> Option<Mat34> {
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
pub(super) fn orthonormal(m: &Mat34) -> Mat34 {
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
pub(super) const GUN_HOLDER_IN_PALM: Mat34 = [-1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0];

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
