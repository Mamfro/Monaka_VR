//! A hand's fingers found on a game skeleton from its reference pose, for [`crate::fingers`]: the
//! adapter names the chains (which bones are which finger), and each knuckle's curl axis comes
//! from the mesh's own geometry, not from a game's bone-axis convention (rigs differ per game, per
//! side and even per joint). Fingers curl toward the palm's normal; the thumb's two tracked bends
//! fold it across the palm instead, in their own plane.
//!
//! Every frame is row-major 3x4 in one right-handed space (a left-handed engine mirrors its frames
//! before and after).

use crate::fingers::{self, Finger, FingerJoints, Hand};
use monaka_core::camera::{self, Mat34};
use monaka_core::math::Vec3;
use monaka_core::protocol::FingerPose;

/// Which bones are a skeleton's hands and fingers, by index.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Chains {
    /// The hand bones, left then right.
    pub wrists: [usize; 2],
    /// Per side, the curled bones of each finger (thumb, index, middle, ring, little) from the
    /// hand out: two or three each, each the child of the one before, or none for a finger the
    /// hand lacks (the index, middle and little fingers set the palm, so they must be there). A
    /// finger's first bone may hang from a bone between it and the hand (a metacarpal), which
    /// stays.
    pub fingers: [[Vec<usize>; 5]; 2],
}

/// The fingers of a skeleton, ready to take tracked bends.
pub struct Rig {
    rest: Vec<Mat34>,
    parents: Vec<i32>,
    /// Per side: (finger, which tracked bend, a one-knuckle hand with that knuckle's own axes).
    hands: [Vec<(usize, usize, Hand)>; 2],
}

fn unit(v: Vec3) -> Option<Vec3> {
    let n = v.iter().map(|v| v * v).sum::<f32>().sqrt();
    (n.is_finite() && n > 1e-5).then(|| v.map(|v| v / n))
}

fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn direction(m: &Mat34, v: Vec3) -> Vec3 {
    std::array::from_fn(|r| (0..3).map(|c| m[r * 4 + c] * v[c]).sum())
}

/// A frame's position.
pub fn position(m: &Mat34) -> Vec3 {
    [m[3], m[7], m[11]]
}

/// Anatomical palm axes: back from the middle knuckle toward the wrist, up toward the index (thumb)
/// side of the knuckles, right their cross product; at the wrist. Finger flexion does not set
/// these axes.
pub fn palm_frame(wrist: Mat34, middle: Mat34, index: Mat34, pinky: Mat34) -> Option<Mat34> {
    let sub = |a: Vec3, b: Vec3| std::array::from_fn(|i| a[i] - b[i]);
    let dot = |a: Vec3, b: Vec3| (0..3).map(|i| a[i] * b[i]).sum::<f32>();
    let normalize = |v: Vec3| {
        let n = dot(v, v).sqrt();
        (n.is_finite() && n > 0.001).then(|| v.map(|x| x / n))
    };
    let at = position(&wrist);
    let back = normalize(sub(at, position(&middle)))?;
    let across = sub(position(&index), position(&pinky));
    let up = normalize(std::array::from_fn(|i| across[i] - back[i] * dot(across, back)))?;
    let right = cross(up, back);
    let frame = [right[0], up[0], back[0], at[0], right[1], up[1], back[1], at[1], right[2], up[2], back[2], at[2]];
    camera::is_rigid(&frame, 0.002).then_some(frame)
}

impl Rig {
    /// The rig of a skeleton: `parents` (-1 for a root, else a lower index), `rest` each bone's
    /// reference pose against its parent, and the hand and finger `chains`. None when the
    /// skeleton does not hold the chains as given, or a hand's palm cannot be told.
    pub fn discover(parents: &[i32], rest: Vec<Mat34>, chains: &Chains) -> Option<Self> {
        if rest.len() != parents.len() || rest.is_empty() || !rest.iter().all(|m| camera::is_rigid(m, 0.002)) {
            return None;
        }
        let mut world: Vec<Mat34> = Vec::with_capacity(rest.len());
        for (i, &parent) in parents.iter().enumerate() {
            if parent < -1 || parent >= i as i32 {
                return None;
            }
            world.push(if parent < 0 { rest[i] } else { camera::compose(&world[parent as usize], &rest[i]) });
        }
        let mut hands: [Vec<(usize, usize, Hand)>; 2] = std::array::from_fn(|_| Vec::new());
        for (side, hand) in hands.iter_mut().enumerate() {
            let chains_of = &chains.fingers[side];
            let first = |finger: usize| chains_of[finger].first().and_then(|&b| world.get(b)).copied();
            let palm = palm_frame(*world.get(chains.wrists[side])?, first(2)?, first(1)?, first(4)?)?;
            // The palm's normal: its right axis on the left hand, mirrored on the right.
            let sign = if side == 0 { 1.0 } else { -1.0 };
            let inward = [palm[0] * sign, palm[4] * sign, palm[8] * sign];
            for (finger, chain) in chains_of.iter().enumerate() {
                if chain.is_empty() {
                    continue;
                }
                if !(2..=3).contains(&chain.len()) || chain.iter().any(|&b| b >= parents.len()) || chain.windows(2).any(|w| parents[w[1]] != w[0] as i32) {
                    return None;
                }
                // The tracked thumb bends cover its last two joints (a three-bone thumb's first
                // keeps its rest pose); a finger's cover its joints from the hand out.
                let start = if finger == 0 { chain.len() - 2 } else { 0 };
                for k in start..chain.len() {
                    let bone = chain[k];
                    let child = chain[(k + 1).min(chain.len() - 1)];
                    let along = unit(position(&rest[child]))?;
                    let along_world = direction(&world[bone], along);
                    // Fingers flex into the palm's normal; the thumb across the palm, away from its
                    // own edge.
                    let toward = if finger == 0 { [-palm[1], -palm[5], -palm[9]] } else { inward };
                    let curl_world = unit(cross(along_world, toward))?;
                    let curl = direction(&camera::rigid_inverse(&world[bone]), curl_world);
                    let mut fingers: [Option<Finger>; 5] = Default::default();
                    fingers[finger] = Some(Finger { base: parents[bone], knuckles: vec![bone as i32], followers: Vec::new() });
                    hand.push((finger, k - start, Hand { fingers, along, curl }));
                }
            }
        }
        Some(Self { rest, parents: parents.to_vec(), hands })
    }

    /// Puts one hand's tracked fingers on `frames` (every bone, in the rig's space): each knuckle
    /// and everything below it moves, the hand does not. False (and `frames` untouched) for an
    /// invalid pose or frames that are not the rig's.
    pub fn apply(&self, frames: &mut [Mat34], side: usize, pose: FingerPose) -> bool {
        let Some(hand) = self.hands.get(side) else { return false };
        if pose.valid == 0 || frames.len() != self.parents.len() || !pose.fingers.iter().flatten().all(|v| v.is_finite()) || !frames.iter().all(|m| camera::is_rigid(m, 0.002)) {
            return false;
        }
        let mut joints = Joints { frames, parents: &self.parents, rest: &self.rest };
        for (finger, bend, mapping) in hand {
            let mut sample = pose;
            sample.fingers[*finger][0] = pose.fingers[*finger][*bend];
            fingers::apply(&mut joints, mapping, &sample);
        }
        true
    }
}

struct Joints<'a> {
    frames: &'a mut [Mat34],
    parents: &'a [i32],
    rest: &'a [Mat34],
}

impl FingerJoints for Joints<'_> {
    fn world(&self, joint: i32) -> Option<Mat34> {
        self.frames.get(usize::try_from(joint).ok()?).copied()
    }

    fn rest_between(&self, from: i32, to: i32) -> Option<Mat34> {
        let to = usize::try_from(to).ok()?;
        (self.parents.get(to) == Some(&from)).then(|| self.rest[to])
    }

    /// Moves the bone and carries every bone below it along.
    fn set_world(&mut self, joint: i32, matrix: &Mat34) {
        let bone = joint as usize;
        let delta = camera::compose(matrix, &camera::rigid_inverse(&self.frames[bone]));
        self.frames[bone] = *matrix;
        for child in bone + 1..self.frames.len() {
            let mut parent = self.parents[child];
            while parent > bone as i32 {
                parent = self.parents[parent as usize];
            }
            if parent == bone as i32 {
                self.frames[child] = camera::compose(&delta, &self.frames[child]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two hands of five fingers, `joints` curled bones each (an optional metacarpal before them),
    /// with a relaxed rest curl, a watch on the left wrist and a nub on each index tip.
    struct Fixture {
        parents: Vec<i32>,
        rest: Vec<Mat34>,
        world: Vec<Mat34>,
        chains: Chains,
        tip: [usize; 2],
        watch: usize,
    }

    fn push(f: &mut Fixture, parent: usize, rest: Mat34) -> usize {
        f.parents.push(parent as i32);
        f.rest.push(rest);
        f.parents.len() - 1
    }

    fn fixture(joints: usize, metacarpals: bool) -> Fixture {
        let mut f = Fixture { parents: vec![-1], rest: vec![camera::translation([0.0; 3])], world: Vec::new(), chains: Chains::default(), tip: [0; 2], watch: 0 };
        for side in 0..2 {
            let wrist = push(&mut f, 0, camera::translation([if side == 0 { -0.2 } else { 0.2 }, 0.0, 0.0]));
            f.chains.wrists[side] = wrist;
            for finger in 0..5 {
                let x = [0.06, 0.04, 0.0, -0.02, -0.04][finger] * if side == 0 { 1.0 } else { -1.0 };
                let mut parent = wrist;
                if metacarpals {
                    parent = push(&mut f, wrist, camera::translation([x * 0.2, 0.0, -0.005]));
                }
                for k in 0..joints {
                    let offset = if k == 0 { [if metacarpals { x * 0.8 } else { x }, 0.0, -0.08] } else { [0.0, 0.0, -0.04] };
                    // A relaxed reference curl; a tracked zero takes it out.
                    let a = -20f32.to_radians();
                    let rest = camera::apply_head(&camera::translation(offset), [(a / 2.0).sin(), 0.0, 0.0, (a / 2.0).cos()], [0.0; 3]);
                    let bone = push(&mut f, parent, rest);
                    f.chains.fingers[side][finger].push(bone);
                    parent = bone;
                }
                if finger == 1 {
                    f.tip[side] = parent;
                    push(&mut f, parent, camera::translation([0.0, 0.0, -0.02]));
                }
            }
        }
        let left_wrist = f.chains.wrists[0];
        f.watch = push(&mut f, left_wrist, camera::translation([0.0, 0.02, 0.0]));
        for i in 0..f.rest.len() {
            let world = if f.parents[i] < 0 { f.rest[i] } else { camera::compose(&f.world[f.parents[i] as usize], &f.rest[i]) };
            f.world.push(world);
        }
        f
    }

    fn open() -> FingerPose {
        FingerPose { valid: 1, ..FingerPose::default() }
    }

    #[test]
    fn mirrored_fingers_curl_toward_palms_without_moving_wrists_watch_or_other_hand() {
        for (joints, metacarpals) in [(3, false), (3, true), (2, false)] {
            let f = fixture(joints, metacarpals);
            let rig = Rig::discover(&f.parents, f.rest.clone(), &f.chains).unwrap();
            for side in 0..2 {
                let mut opened = f.world.clone();
                assert!(rig.apply(&mut opened, side, open()));
                let mut closed = f.world.clone();
                assert!(rig.apply(&mut closed, side, FingerPose { fingers: [[0.7, 0.8, 0.6, 0.0]; 5], ..open() }));
                let tip = f.tip[side];
                assert!(position(&closed[tip])[1] < position(&opened[tip])[1] - 0.02, "{joints} joints, metacarpals {metacarpals}");
                // The nub rides its knuckle.
                let (nub, knuckle) = (tip + 1, tip);
                let riding = camera::compose(&camera::rigid_inverse(&closed[knuckle]), &closed[nub]);
                assert!(camera::distance(&riding, &f.rest[nub]) < 1e-4);
                let wrist = f.chains.wrists[side];
                assert_eq!(opened[wrist], f.world[wrist]);
                assert_eq!(closed[f.watch], f.world[f.watch]);
                let other = f.chains.wrists[1 - side];
                assert_eq!(closed[other], f.world[other]);
                // Segments keep their reference lengths.
                let chain = &f.chains.fingers[side][1];
                for w in chain.windows(2) {
                    let (a, b) = (position(&closed[w[1]]), position(&closed[w[0]]));
                    let length = (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f32>().sqrt();
                    assert!((length - 0.04).abs() < 1e-5);
                }
            }
        }
    }

    #[test]
    fn invalid_tracking_and_wrong_hierarchies_preserve_animation() {
        let mut f = fixture(3, false);
        let rig = Rig::discover(&f.parents, f.rest.clone(), &f.chains).unwrap();
        let mut frames = f.world.clone();
        assert!(!rig.apply(&mut frames, 0, FingerPose::default()));
        let mut bad = open();
        bad.fingers[1][1] = f32::NAN;
        assert!(!rig.apply(&mut frames, 0, bad));
        assert_eq!(frames, f.world);
        let bone = f.chains.fingers[0][1][1];
        f.parents[bone] = 0;
        assert!(Rig::discover(&f.parents, f.rest.clone(), &f.chains).is_none());
        f.parents[bone] = bone as i32;
        assert!(Rig::discover(&f.parents, f.rest.clone(), &f.chains).is_none());
        let mut short = fixture(3, false);
        short.chains.fingers[0][2].truncate(1);
        assert!(Rig::discover(&short.parents, short.rest.clone(), &short.chains).is_none());
        // A hand without a thumb or ring finger still has a palm; one without a middle finger not.
        let mut fewer = fixture(3, false);
        fewer.chains.fingers[0][0].clear();
        fewer.chains.fingers[1][3].clear();
        assert!(Rig::discover(&fewer.parents, fewer.rest.clone(), &fewer.chains).is_some());
        fewer.chains.fingers[0][2].clear();
        assert!(Rig::discover(&fewer.parents, fewer.rest.clone(), &fewer.chains).is_none());
    }

    #[test]
    fn individual_fingers_and_two_thumb_bends_are_independent_and_bounded() {
        for joints in [3, 2] {
            let f = fixture(joints, false);
            let rig = Rig::discover(&f.parents, f.rest.clone(), &f.chains).unwrap();
            let mut opened = f.world.clone();
            assert!(rig.apply(&mut opened, 0, open()));
            let mut pose = open();
            pose.fingers[1] = [0.8, 0.4, 0.3, 0.0];
            let mut index = f.world.clone();
            assert!(rig.apply(&mut index, 0, pose));
            for finger in [0, 2, 3, 4] {
                for &bone in &f.chains.fingers[0][finger] {
                    assert_eq!(index[bone], opened[bone]);
                }
            }
            let thumb = &f.chains.fingers[0][0];
            let mut pose = open();
            pose.fingers[0] = [0.4, 0.9, 0.0, 0.0];
            let mut tracked = f.world.clone();
            assert!(rig.apply(&mut tracked, 0, pose));
            // A three-bone thumb keeps its first bone; its last two (and a two-bone thumb's both)
            // take the bends.
            if joints == 3 {
                assert_eq!(tracked[thumb[0]], opened[thumb[0]]);
            }
            assert_ne!(tracked[thumb[joints - 2]], opened[thumb[joints - 2]]);
            assert_ne!(tracked[thumb[joints - 1]], opened[thumb[joints - 1]]);
            let mut huge = open();
            huge.fingers[1] = [100.0; 4];
            let mut bounded = open();
            bounded.fingers[1] = [1.9; 4];
            let (mut a, mut b) = (f.world.clone(), f.world.clone());
            assert!(rig.apply(&mut a, 0, huge));
            assert!(rig.apply(&mut b, 0, bounded));
            assert_eq!(a, b);
        }
    }

    #[test]
    fn applying_twice_from_the_same_hand_gives_the_same_fingers() {
        let f = fixture(3, true);
        let rig = Rig::discover(&f.parents, f.rest.clone(), &f.chains).unwrap();
        let pose = FingerPose { fingers: [[0.5, 0.6, 0.4, 0.0]; 5], ..open() };
        let mut once = f.world.clone();
        assert!(rig.apply(&mut once, 1, pose));
        let mut twice = once.clone();
        assert!(rig.apply(&mut twice, 1, pose));
        for (a, b) in once.iter().zip(&twice) {
            assert!(camera::distance(a, b) < 1e-5);
        }
    }
}
