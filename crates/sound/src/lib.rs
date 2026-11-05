//! Sound in the headset: a recording looped through XAudio2 (Windows' own), each of its channels
//! coming from a place in the room. Every frame the viewer says where each place is from the head
//! ([`hearing`]); a thread of its own mixes what each ear hears ([`mix`]: through the ear
//! responses measured on a dummy head for that direction, and echoes), so a stereo recording of a beach, its left channel out past the
//! room's left wall and its right past the right, sounds out there, and stays there as the player
//! turns.

pub mod mix;
#[cfg(test)]
mod tests;

pub use mix::{Hearing, hearing};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use windows::Win32::Media::Audio::XAudio2::*;
use windows::Win32::Media::Audio::{AudioCategory_GameEffects, WAVEFORMATEX};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::core::PCWSTR;

/// A recording decoded into memory: 16-bit samples, channels interleaved.
pub struct Clip {
    pub samples: Vec<i16>,
    pub channels: u16,
    pub rate: u32,
}

/// Decodes the Ogg Vorbis file at `path`.
pub fn decode(path: &Path) -> Result<Clip, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    decode_bytes(&bytes, &path.display().to_string())
}

/// Decodes an Ogg Vorbis file's `bytes` (`what` names it, for what goes wrong).
pub fn decode_bytes(bytes: &[u8], what: &str) -> Result<Clip, String> {
    let failed = |e: &dyn std::fmt::Display| format!("{what}: {e}");
    let mut reader = lewton::inside_ogg::OggStreamReader::new(std::io::Cursor::new(bytes)).map_err(|e| failed(&e))?;
    let (channels, rate) = (u16::from(reader.ident_hdr.audio_channels), reader.ident_hdr.audio_sample_rate);
    if !(1..=2).contains(&channels) {
        return Err(failed(&format!("{channels} channels; only mono and stereo are played")));
    }
    let mut samples = Vec::new();
    while let Some(packet) = reader.read_dec_packet_itl().map_err(|e| failed(&e))? {
        samples.extend(packet);
    }
    if samples.is_empty() {
        return Err(failed(&"no sound in it"));
    }
    Ok(Clip { samples, channels, rate })
}

/// The mix goes out in blocks this long (seconds), this many waiting at a time: about 30 ms ahead.
const BLOCK: f32 = 0.01;
const AHEAD: u32 = 3;

/// The voice the mixing thread hands its blocks to.
struct Voice(IXAudio2SourceVoice);

// SAFETY: XAudio2's voices may be used from any thread; the mixing thread only queues buffers and
// reads how many wait, and the voice is destroyed only once that thread has ended.
unsafe impl Send for Voice {}

/// A clip looping on the default audio device (the headset's, while SteamVR has it), mixed for
/// the ears on a thread of its own.
pub struct Ambience {
    // Destroyed in [`Drop`], after the mixing thread has ended and before its blocks go.
    engine: IXAudio2,
    master: IXAudio2MasteringVoice,
    voice: IXAudio2SourceVoice,
    hearing: Arc<Mutex<[Hearing; 2]>>,
    stop: Arc<AtomicBool>,
    /// The mixing thread; it hands back its blocks when it ends.
    mixing: Option<std::thread::JoinHandle<Vec<Vec<f32>>>>,
}

impl Ambience {
    /// Starts `clip` looping, silent until [`Ambience::set`] says how loud.
    pub fn play(clip: Clip) -> Result<Self, String> {
        let failed = |what: &'static str| move |e: windows::core::Error| format!("{what}: {e}");
        // SAFETY: COM on this thread, as XAudio2 asks; a thread already set up another way keeps it.
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let mut engine = None;
        // SAFETY: a local out-pointer; the version is Windows 10's.
        unsafe { XAudio2CreateWithVersionInfo(&mut engine, 0, XAUDIO2_DEFAULT_PROCESSOR, 0x0A00_0000) }.map_err(failed("XAudio2"))?;
        let engine = engine.ok_or("no XAudio2")?;
        let mut master = None;
        // SAFETY: the engine is live; the default device, its own channels and rate.
        unsafe { engine.CreateMasteringVoice(&mut master, 0, 0, 0, PCWSTR::null(), None, AudioCategory_GameEffects) }.map_err(failed("audio device"))?;
        let master = master.ok_or("no audio device")?;
        // The mix: stereo, 32-bit float, at the ear responses' rate.
        let format = WAVEFORMATEX {
            wFormatTag: 3, // IEEE float
            nChannels: 2,
            nSamplesPerSec: mix::RATE,
            nAvgBytesPerSec: mix::RATE * 8,
            nBlockAlign: 8,
            wBitsPerSample: 32,
            cbSize: 0,
        };
        let mut voice = None;
        // SAFETY: the engine is live, the format a local, no callback.
        unsafe { engine.CreateSourceVoice(&mut voice, &format, 0, XAUDIO2_DEFAULT_FREQ_RATIO, None, None, None) }.map_err(failed("sound voice"))?;
        let voice = voice.ok_or("no sound voice")?;
        // Left to the device's front left, right to its front right (the first two of its
        // channels); a device of one channel gets both, halved.
        // SAFETY: a live voice.
        let outputs = unsafe { master.GetVoiceDetails().InputChannels } as usize;
        let matrix = if outputs >= 2 {
            let mut matrix = vec![0.0f32; 2 * outputs];
            // The volume of mix channel `s` in output `d` is at `2 * d + s`.
            matrix[0] = 1.0;
            matrix[2 + 1] = 1.0;
            matrix
        } else {
            vec![0.5, 0.5]
        };
        // SAFETY: a live voice sending to the live device voice, the matrix its shape.
        unsafe {
            voice.SetOutputMatrix(&master, 2, outputs as u32, matrix.as_ptr(), XAUDIO2_COMMIT_NOW).map_err(failed("sound matrix"))?;
            voice.SetVolume(0.0, XAUDIO2_COMMIT_NOW).map_err(failed("sound volume"))?;
            voice.Start(0, XAUDIO2_COMMIT_NOW).map_err(failed("sound start"))?;
        }
        let hearing = Arc::new(Mutex::new([Hearing { direction: [-1.0, 0.0, 0.0], gain: 1.0 }, Hearing { direction: [1.0, 0.0, 0.0], gain: 1.0 }]));
        let stop = Arc::new(AtomicBool::new(false));
        let mixing = {
            let (hearing, stop, voice) = (hearing.clone(), stop.clone(), Voice(voice.clone()));
            std::thread::Builder::new().name("monaka sound".into()).spawn(move || mix_loop(clip, &voice, &hearing, &stop)).map_err(|e| format!("sound thread: {e}"))?
        };
        Ok(Self { engine, master, voice, hearing, stop, mixing: Some(mixing) })
    }

    /// Where each of the clip's channels is heard from now ([`hearing`]), and how loud the whole
    /// is (0 to 1).
    pub fn set(&self, hearing: [Hearing; 2], volume: f32) {
        if let Ok(mut heard) = self.hearing.lock() {
            *heard = hearing;
        }
        // SAFETY: a live voice; XAudio2 eases the change in.
        let _ = unsafe { self.voice.SetVolume(volume.clamp(0.0, 1.0), XAUDIO2_COMMIT_NOW) };
    }
}

/// The mixing thread: keeps [`AHEAD`] blocks waiting on `voice`, each mixed as `hearing` says when
/// it is made, until told to `stop`; then hands its blocks back (the voice may still read them).
fn mix_loop(clip: Clip, voice: &Voice, hearing: &Mutex<[Hearing; 2]>, stop: &AtomicBool) -> Vec<Vec<f32>> {
    let frames = ((mix::RATE as f32 * BLOCK) as usize).max(64);
    let mut blocks = vec![vec![0.0f32; frames * 2]; AHEAD as usize + 1];
    let mut mixer = mix::Mixer::new(clip.rate);
    let channels = usize::from(clip.channels);
    let mut next = 0;
    while !stop.load(Ordering::Relaxed) {
        let mut state = XAUDIO2_VOICE_STATE::default();
        // SAFETY: a live voice and a local out-pointer.
        unsafe { voice.0.GetState(&mut state, XAUDIO2_VOICE_NOSAMPLESPLAYED) };
        if state.BuffersQueued >= AHEAD {
            std::thread::sleep(Duration::from_millis(2));
            continue;
        }
        let heard = hearing.lock().map(|h| *h).unwrap_or_default();
        // With fewer waiting than there are blocks, this one has been played.
        let block = &mut blocks[next];
        mixer.render(&clip.samples, channels, heard, block);
        let buffer = XAUDIO2_BUFFER { AudioBytes: (block.len() * 4) as u32, pAudioData: block.as_ptr().cast(), ..Default::default() };
        // SAFETY: the block stays alive and unchanged until the voice has played it: it is not
        // written again until [`AHEAD`] newer ones were queued after it, and the blocks outlive
        // the voice ([`Ambience`]'s drop).
        if unsafe { voice.0.SubmitSourceBuffer(&buffer, None) }.is_err() {
            break;
        }
        next = (next + 1) % blocks.len();
    }
    blocks
}

impl Drop for Ambience {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // The blocks the voice may still read are kept until it is gone.
        let blocks = self.mixing.take().and_then(|thread| thread.join().ok());
        // SAFETY: the voices are ours and live; destroyed (which waits for XAudio2 to be done with
        // them) before the blocks and the engine go.
        unsafe {
            self.voice.DestroyVoice();
            self.master.DestroyVoice();
            self.engine.StopEngine();
        }
        drop(blocks);
    }
}
