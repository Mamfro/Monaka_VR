//! The scene's size (the render size the game's scene passes are told by) and each frame's eye and
//! pose ([`labelled_frame`]: the frame labels, foreseen where the game labels a frame late).

use super::*;


/// The render size the scene was last seen drawn at (width in the high half, 0 unknown).
pub(super) static SCENE: AtomicU64 = AtomicU64::new(0);
/// The game renders at its output size: no pass is told apart by size.
pub(super) static SAME_SIZE: AtomicBool = AtomicBool::new(false);

pub(super) fn pack(size: (u32, u32)) -> u64 {
    ((size.0 as u64) << 32) | size.1 as u64
}

/// The size the scene renders at (the game's frame inputs: its upscaler's input), unless that is the
/// size it is upscaled to. A change is logged.
pub(super) fn scene() -> Option<(u32, u32)> {
    let (scene, output) = monaka_framegen::inputs::render_sizes()?;
    let same = output == Some(scene);
    if SAME_SIZE.swap(same, Relaxed) != same && same {
        log!("foveation waits: the scene renders at the output size {}x{}, so its passes cannot be told from the HUD's", scene.0, scene.1);
    }
    if SCENE.swap(pack(scene), Relaxed) != pack(scene) {
        log!("foveation: the scene renders at {}x{} (output {output:?})", scene.0, scene.1);
    }
    (!same).then_some(scene)
}

pub(super) struct Track {
    /// The present the frame being recorded follows, and its eye and pose as decided.
    pub(super) present: u64,
    pub(super) frame: Option<Pending>,
    /// Decided before its label came (and whether the label has been compared since).
    pub(super) predicted: bool,
    pub(super) checked: bool,
    /// The latest label seen, and the present its frame followed.
    pub(super) last: Option<(u64, Label)>,
}

impl Track {
    pub(super) const NEW: Track = Track { present: u64::MAX, frame: None, predicted: false, checked: false, last: None };
}

pub(super) static TRACK: Mutex<Track> = Mutex::new(Track::NEW);

/// [`crate::providers::FrameLabels::rendering`] for a game whose frames are labelled by their camera
/// (`Driver::label_frame`): the frame's own label once the renderer has given it; before that,
/// the eye after the last labelled frame's (the eyes alternate), with that pair's pose for a right
/// eye and the newest head pose for a new pair's left; with no label lately, the eye the next
/// present shows. A label that comes after the frame's first scene pass corrects its later passes.
pub fn labelled_frame() -> Option<Pending> {
    if !DRIVER.running() {
        return None;
    }
    let present = DRIVER.presents();
    let label = DRIVER.peek_label();
    let mut track = TRACK.locked();
    let new_frame = track.present != present;
    if let Some(label) = label {
        if !new_frame && track.predicted && !track.checked {
            track.checked = true;
            let right = track.frame.is_some_and(|f| f.eye == label.eye);
            TALLY.predicted_right[right as usize].fetch_add(1, Relaxed);
        }
        if new_frame {
            TALLY.from_label.fetch_add(1, Relaxed);
            track.predicted = false;
        }
        (track.present, track.frame, track.last) = (present, Some(Pending { eye: label.eye.min(1), pose: label.pose }), Some((present, label)));
        return track.frame;
    }
    if new_frame {
        TALLY.predicted.fetch_add(1, Relaxed);
        let frame = predict(track.last, present);
        (track.present, track.frame, track.predicted, track.checked) = (present, frame, true, false);
    }
    track.frame
}

pub(super) fn predict(last: Option<(u64, Label)>, present: u64) -> Option<Pending> {
    match last {
        Some((at, label)) if at + 1 == present => {
            let eye = 1 - label.eye.min(1);
            // A pair's right eye is drawn from its left's pose; a new pair from the newest head.
            let pose = if eye == 1 { label.pose } else { DRIVER.head().current().unwrap_or(label.pose) };
            Some(Pending { eye, pose })
        }
        _ => {
            let eye = DRIVER.schedule()?.shown_eye(DRIVER.drawing_present()?)?;
            Some(Pending { eye, pose: DRIVER.head().current().unwrap_or_default() })
        }
    }
}
