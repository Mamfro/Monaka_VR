//! Frames labelled by their camera (the camera hook notes each eye camera it writes, the renderer
//! labels the frame it renders from one: [`Driver::camera_written`], [`Driver::label_frame`]) and
//! eye choices taken in turn by the camera updates ([`Driver::take_in_turn`], [`Sequencer`]).

use super::{Driver, Publisher};
use monaka_core::alternate::Schedule;
use monaka_core::math::Vec3;
use monaka_contract::protocol::HeadPose;
use monaka_core::sync::Locked;
use std::sync::atomic::Ordering::*;

/// A frame recognised by its camera: its eye and the pose it was drawn from.
#[derive(Clone, Copy, Debug)]
pub struct Label {
    pub eye: usize,
    pub pose: HeadPose,
}

/// A frame taken in turn ([`Driver::take_in_turn`]): the present it follows, its eye, its pose.
#[derive(Clone, Copy, Debug)]
pub struct Taken {
    pub n: u64,
    pub eye: usize,
    pub pose: HeadPose,
}

/// The eye cameras written lately.
pub(super) const WRITTEN_KEPT: usize = 8;
/// A rendered camera this close (world units, metres) to a written one is that one.
pub(super) const SAME_CAMERA: f32 = 0.005;

/// What the labels and the choices in turn keep.
pub(super) struct Frames {
    written: [Option<(Vec3, Label)>; WRITTEN_KEPT],
    next: usize,
    pub(super) label: Option<Label>,
    /// Frames have been labelled this run, so a frame without a label is not guessed.
    pub(super) labelling: bool,
    sequencer: Sequencer,
    pair_pose: HeadPose,
}

impl Frames {
    pub(super) const NEW: Frames = Frames { written: [None; WRITTEN_KEPT], next: 0, label: None, labelling: false, sequencer: Sequencer::NEW, pair_pose: HeadPose::EMPTY };

    /// The label of the camera written nearest `position`, within [`SAME_CAMERA`].
    fn written_at(&self, position: Vec3) -> Option<Label> {
        let distance = |p: &Vec3| (0..3).map(|i| (p[i] - position[i]).abs()).fold(0.0, f32::max);
        self.written.iter().flatten().filter(|(p, _)| distance(p) < SAME_CAMERA).min_by(|a, b| distance(&a.0).total_cmp(&distance(&b.0))).map(|(_, label)| *label)
    }
}

impl Driver {
    /// The camera hook wrote eye `eye`'s camera at `position`, drawn from `pose`: a frame rendered
    /// from it can be recognised ([`Driver::label_frame`]).
    pub fn camera_written(&self, position: Vec3, eye: usize, pose: HeadPose) {
        let mut frames = self.frames.locked();
        let slot = frames.next % WRITTEN_KEPT;
        frames.written[slot] = Some((position, Label { eye, pose }));
        frames.next += 1;
    }

    /// The renderer reports the camera position it renders the coming frame with: the frame is
    /// labelled with the eye and pose of the camera written there. Returns the eye, if recognised.
    pub fn label_frame(&self, position: Vec3) -> Option<usize> {
        if !self.capture.load(Acquire) || !position.iter().all(|v| v.is_finite()) {
            return None;
        }
        let mut frames = self.frames.locked();
        match frames.written_at(position) {
            Some(label) => {
                frames.label = Some(label);
                frames.labelling = true;
                self.tally.labelled.fetch_add(1, Relaxed);
                Some(label.eye)
            }
            None => {
                self.tally.unmatched.fetch_add(1, Relaxed);
                None
            }
        }
    }

    /// The eye of the camera written at `position`, if one was lately (nothing labelled: the
    /// renderer asking before the frame's own label).
    pub fn eye_at(&self, position: Vec3) -> Option<usize> {
        if !position.iter().all(|v| v.is_finite()) {
            return None;
        }
        self.frames.locked().written_at(position).map(|label| label.eye)
    }

    /// The label of the frame about to present, left for the present loop.
    pub fn peek_label(&self) -> Option<Label> {
        self.frames.locked().label
    }

    /// The label of the frame about to present, taken (a game's own publishing path).
    pub fn take_label(&self) -> Option<Label> {
        self.frames.locked().label.take()
    }

    /// For [`Install::in_turn`]: the next camera update's frame, taking the choices made at the
    /// presents one by one ([`Sequencer`]); its pose sampled now for a pair's left eye (or every
    /// frame, [`Install::pose_each_frame`]), recorded for the pair it completes. None while not
    /// running.
    pub fn take_in_turn(&self) -> Option<Taken> {
        if !self.capture.load(Acquire) || self.eye.load(Acquire) < 0 {
            return None;
        }
        let (choice, taken) = self.frames.locked().sequencer.next(self.choices.load(Acquire));
        let counter = match taken {
            Turn::InStep => None,
            Turn::Early => Some(&self.tally.early),
            Turn::Late => Some(&self.tally.late),
            Turn::Resynced => Some(&self.tally.resynced),
        };
        if let Some(counter) = counter {
            counter.fetch_add(1, Relaxed);
        }
        // The camera written for choice `choice` is the one drawn after present `choice - 1`.
        let n = choice.saturating_sub(1);
        let mut cell = self.state.locked();
        let state = cell.0.as_mut()?;
        let each_frame = state.pose_each_frame.is_some_and(|f| f());
        let pose = {
            let mut frames = self.frames.locked();
            if Schedule::starts_pair(n) || each_frame || self.own_schedule() {
                let tracked = state.shared.head_tracking || state.shared.synthetic.is_some();
                let mut pose = if tracked { self.head.current().unwrap_or_default() } else { HeadPose::default() };
                if let Some(adjust) = state.adjust_pose {
                    adjust(n, &mut pose);
                }
                frames.pair_pose = pose;
            }
            frames.pair_pose
        };
        match &mut state.publisher {
            Publisher::D3D11(publisher) => publisher.record_pose(n, pose),
            Publisher::D3D12(publisher) => publisher.record_pose(n, pose),
        }
        Some(Taken { n, eye: Schedule::camera_eye(n), pose })
    }
}

/// Hands eye choices to the camera updates strictly in turn. The game thread's updates and the
/// presents match one to one, but which comes first wobbles: an update can arrive before the
/// present that makes its choice, or after the next one. Taking the choice after the last one used,
/// rather than the newest made, keeps every frame on its own eye through that wobble; only a
/// lasting offset (an update or a present that had no partner) moves it back into step.
pub(super) struct Sequencer {
    used: u64,
    run: u32,
    run_sign: i64,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Turn {
    InStep,
    Early,
    Late,
    Resynced,
}

impl Sequencer {
    const NEW: Sequencer = Sequencer { used: 0, run: 0, run_sign: 0 };
    /// Updates in a row offset the same way before the sequence follows the presents again.
    const RESYNC_AFTER: u32 = 8;

    /// The choice (1-based) for the next camera update, `made` choices having been made so far.
    fn next(&mut self, made: u64) -> (u64, Turn) {
        if self.used == 0 {
            self.used = made;
            return (made, Turn::InStep);
        }
        let choice = self.used + 1;
        let offset = (made as i64 - choice as i64).signum();
        if offset != 0 && offset == self.run_sign {
            self.run += 1;
        } else {
            self.run = u32::from(offset != 0);
            self.run_sign = offset;
        }
        if self.run >= Self::RESYNC_AFTER {
            *self = Self { used: made, run: 0, run_sign: 0 };
            return (made, Turn::Resynced);
        }
        self.used = choice;
        (choice, if offset < 0 { Turn::Early } else if offset > 0 { Turn::Late } else { Turn::InStep })
    }
}

#[cfg(test)]
mod tests;
