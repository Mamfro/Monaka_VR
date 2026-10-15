//! Measurements of an alternate-eye site's views, logged at the end of a run ([`report`]): each
//! eye's view centre and yaw frame to frame, which head pose a camera turned ahead of the site
//! carried, what the camera the site read held, and how far back the camera each present shows was
//! chosen. They need nothing from the game but the site's calls: the site notes each view it draws
//! ([`note_step`], [`note_yaw`], [`rendered`]), the loop each present ([`note_label`]).

use monaka_core::alternate::Schedule;
use monaka_core::camera::Mat34;
use monaka_producer::log;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::*};

#[cfg(test)]
mod tests;

/// Each eye's last two view centres and how each was made (true: from a camera turned ahead of the
/// site, such as the game's live camera).
type Steps = [[([f32; 3], bool); 2]; 2];

static STEPS: Mutex<Steps> = Mutex::new([[([f32::NAN; 3], false); 2]; 2]);
/// Frames, by eye, whose view centre stepped back against the eye's own motion (moving over 5 mm
/// a frame): [made like the frame before, made the other way].
static REVERSALS: [[AtomicU64; 2]; 2] = [const { [const { AtomicU64::new(0) }; 2] }; 2];
static MOVING: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];

/// A view centre moving more than this a frame (metres) counts as moving.
const MOVING_STEP: f32 = 0.005;

/// The steps of a view centre at `older`, `last`, then `now`, when both move ([`MOVING_STEP`]):
/// whether the second went back against the first, and both lengths.
pub fn step_back(older: [f32; 3], last: [f32; 3], now: [f32; 3]) -> Option<(bool, f32, f32)> {
    let step = |a: [f32; 3], b: [f32; 3]| [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let (before, after) = (step(older, last), step(last, now));
    let length = |v: [f32; 3]| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if !(length(before) > MOVING_STEP && length(after) > MOVING_STEP) {
        return None;
    }
    let dot = before[0] * after[0] + before[1] * after[1] + before[2] * after[2];
    Some((dot < 0.0, length(before), length(after)))
}

fn made(turned: bool) -> &'static str {
    if turned { "turned live camera" } else { "rebuilt at view setup" }
}

/// Measures eye `eye`'s view centre (`center`, made from a camera turned ahead of the site when
/// `turned`) against its last two: a centre stepping back against its own motion is a jitter (a
/// position going back and forth, seen most on near things).
pub fn note_step(eye: usize, center: &Mat34, turned: bool) {
    let eye = eye.min(1);
    let now = [center[3], center[7], center[11]];
    let Ok(mut steps) = STEPS.lock() else { return };
    let [(older, _), (last, last_turned)] = steps[eye];
    if let Some((back, before, after)) = step_back(older, last, now) {
        MOVING[eye].fetch_add(1, Relaxed);
        if back {
            let switched = turned != last_turned;
            let n = REVERSALS[eye][switched as usize].fetch_add(1, Relaxed);
            if n < 6 {
                log!("eye {eye} view stepped back: {before:.3} m then {after:.3} m against it ({} -> {})", made(last_turned), made(turned));
            }
        }
    }
    steps[eye] = [(last, last_turned), (now, turned)];
}

/// Each eye's last two view yaws (radians, positive left).
static YAWS: Mutex<[[f32; 2]; 2]> = Mutex::new([[f32::NAN; 2]; 2]);
/// Frames, by eye, whose view yaw left the line of the two before by over half a degree (a head
/// turning smoothly turns the view smoothly): [made from a turned camera, rebuilt at the site]; and
/// frames measured.
static YAW_JUMPS: [[AtomicU64; 2]; 2] = [const { [const { AtomicU64::new(0) }; 2] }; 2];
static YAW_FRAMES: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];

/// How far (radians) a yaw `yaw` leaves the line of the two before it (`older`, `last`).
pub fn off_line(older: f32, last: f32, yaw: f32) -> f32 {
    let wrap = monaka_core::math::wrap_radians;
    wrap(yaw - last) - wrap(last - older)
}

/// Measures eye `eye`'s view yaw (`center`'s) against its last two.
pub fn note_yaw(eye: usize, center: &Mat34, turned: bool) {
    let eye = eye.min(1);
    // Camera-to-world: column 2 is the back axis.
    let yaw = center[2].atan2(center[10]);
    let Ok(mut yaws) = YAWS.lock() else { return };
    let [older, last] = yaws[eye];
    let wrap = monaka_core::math::wrap_radians;
    if older.is_finite() {
        YAW_FRAMES[eye].fetch_add(1, Relaxed);
        let off = off_line(older, last, yaw);
        if off.abs() > 0.5f32.to_radians() {
            let n = YAW_JUMPS[eye][(!turned) as usize].fetch_add(1, Relaxed);
            if n < 6 {
                log!(
                    "eye {eye} view yaw left its line by {:.2} degrees (steps {:.2} then {:.2}; {})",
                    off.to_degrees(),
                    wrap(last - older).to_degrees(),
                    wrap(yaw - last).to_degrees(),
                    made(turned)
                );
            }
        }
    }
    yaws[eye] = [last, yaw];
}

/// Eye cameras made from a camera turned ahead of the site, by eye: [turned with this frame's
/// pair's head pose, with an older one]. Both eyes of a pair share the pose sampled at its
/// left-eye present; a left-eye frame whose camera was turned a present earlier carries the
/// previous pair's head.
static TURN_PAIRS: [[AtomicU64; 2]; 2] = [const { [const { AtomicU64::new(0) }; 2] }; 2];

/// Eye `eye` of the pair from present `pair` was drawn from a camera turned with the head pose of
/// the pair from present `turned_for`.
pub fn note_turn_pair(eye: usize, pair: u64, turned_for: u64) {
    let older = turned_for != pair;
    TURN_PAIRS[eye.min(1)][older as usize].fetch_add(1, Relaxed);
    static LOGGED: AtomicU64 = AtomicU64::new(0);
    if older && LOGGED.fetch_add(1, Relaxed) < 12 {
        log!("eye {eye} of the pair from present {pair} was drawn from a live camera turned with the head pose of the pair from present {turned_for}");
    }
}

/// The choice the latest eye camera written was rendered from: the present it was made after
/// (`<< 1`) and the eye, `u64::MAX` before the first; and the thread that wrote it.
static RENDERED: AtomicU64 = AtomicU64::new(u64::MAX);
static RENDER_THREAD: AtomicU32 = AtomicU32::new(0);
/// Presents by how many presents after its choice the latest eye camera written was (0..=4, then
/// 5 for later); the schedule assumes `latency`.
static LABEL_LAGS: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];
static LABEL_MISSES: AtomicU64 = AtomicU64::new(0);

/// The site wrote eye `eye`'s camera, chosen after present `chosen_at`, on this thread.
pub fn rendered(chosen_at: u64, eye: usize) {
    RENDERED.store((chosen_at << 1) | eye as u64, Release);
    RENDER_THREAD.store(monaka_hook::thread_id(), Relaxed);
}

/// Measures the schedule against what was rendered: at present `n` the image is the eye the
/// schedule says only if the latest eye camera written was chosen `latency` presents earlier. An
/// image drawn from the other eye's position makes near things jitter.
pub fn note_label(n: u64, schedule: Schedule) {
    let rendered = RENDERED.load(Acquire);
    if rendered == u64::MAX || schedule.shown_eye(n).is_none() {
        return;
    }
    let (chosen, eye) = (rendered >> 1, (rendered & 1) as usize);
    let lag = n.saturating_sub(chosen);
    LABEL_LAGS[lag.min(5) as usize].fetch_add(1, Relaxed);
    if lag != schedule.latency && LABEL_MISSES.fetch_add(1, Relaxed) < 12 {
        log!(
            "present {n} shows the eye-{eye} camera chosen after present {chosen} ({lag} presents back, not {}): the schedule calls it eye {}; render thread {}, present thread {}",
            schedule.latency,
            schedule.eye_at(n),
            RENDER_THREAD.load(Relaxed),
            monaka_hook::thread_id()
        );
    }
}

/// Site calls by what the camera the site read held: [a turn of ours, the game's own camera,
/// neither (a camera the site did not see: another camera rendering)], and the largest distance of
/// a match.
static CAMERA_MATCHES: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];
static CAMERA_MATCH_WORST: AtomicU32 = AtomicU32::new(0);

/// Which camera the site read: 0 a turn of ours, 1 the game's own, 2 neither; `to_turned` from the
/// nearest turn of ours, `to_base` from the nearest game camera, a match within `within`.
pub fn camera_kind(to_turned: f32, to_base: f32, within: f32) -> usize {
    match to_turned.min(to_base) >= within {
        true => 2,
        false if to_turned <= to_base => 0,
        false => 1,
    }
}

/// What the camera the site read held (`known`: turns of ours are remembered): `to_turned` from
/// the nearest turn of ours, `to_base` from the nearest game camera, a match within `within`;
/// `cutscene` whether another camera has the view now (for the log).
pub fn note_camera_match(known: bool, to_turned: f32, to_base: f32, within: f32, eye: usize, cutscene: bool) {
    if !known {
        return;
    }
    let kind = camera_kind(to_turned, to_base, within);
    if kind != 2 {
        CAMERA_MATCH_WORST.fetch_max(to_turned.min(to_base).to_bits(), Relaxed);
    }
    let n = CAMERA_MATCHES[kind].fetch_add(1, Relaxed);
    if kind == 2 && log::seldom(n + 1, 6) {
        log!(
            "eye {eye}: the render cache holds a camera the view did not see ({} so far; {to_turned:.3} from the nearest turn, {to_base:.3} from the nearest game camera; cutscene {cutscene}): made from it as the game set it",
            n + 1,
        );
    }
}

/// The measurements, at the end of a run.
pub fn report() {
    log!("presents by how far back the camera they show was chosen [0, 1, 2, 3, 4, 5+]: {:?}", LABEL_LAGS.each_ref().map(|c| c.load(Relaxed)));
    log!(
        "view centre steps back while moving, by eye [made like the frame before, made the other way] of frames moving: left {:?} of {}, right {:?} of {}",
        REVERSALS[0].each_ref().map(|c| c.load(Relaxed)),
        MOVING[0].load(Relaxed),
        REVERSALS[1].each_ref().map(|c| c.load(Relaxed)),
        MOVING[1].load(Relaxed)
    );
    log!(
        "view yaw jumps over half a degree, by eye [turned live camera, rebuilt at view setup]: left {:?} of {}, right {:?} of {}",
        YAW_JUMPS[0].each_ref().map(|c| c.load(Relaxed)),
        YAW_FRAMES[0].load(Relaxed),
        YAW_JUMPS[1].each_ref().map(|c| c.load(Relaxed)),
        YAW_FRAMES[1].load(Relaxed)
    );
    log!("eye cameras from a turned live camera [this pair's head, an older one]: left {:?}, right {:?}", TURN_PAIRS[0].each_ref().map(|c| c.load(Relaxed)), TURN_PAIRS[1].each_ref().map(|c| c.load(Relaxed)));
    log!(
        "render cache cameras [a turn of ours, the game's own, neither]: {:?}; the farthest match {:.5}",
        CAMERA_MATCHES.each_ref().map(|c| c.load(Relaxed)),
        f32::from_bits(CAMERA_MATCH_WORST.load(Relaxed))
    );
}
