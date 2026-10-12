//! What every producer injected into a game shares.
//!
//! The loader copies a producer DLL into a fresh run folder, writes option files beside it,
//! injects it and calls its `MonakaStart`, later `MonakaStop`, and `MonakaStatus` to learn whether an
//! older copy is still active. A game crate exports those three and hands them to [`Lifecycle`];
//! [`Session`] gives it the run folder, its options and a log.
//!
//! Nothing here unwinds into the game: failures are return codes and log lines.
#![forbid(unsafe_code)]

use monaka_core::options::Options;
pub use monaka_core::{manifest, pack};
use monaka_core::protocol::ChannelName;
use monaka_hook::module::Module;
use monaka_hook::{Hooks, InFlight};
use std::fmt;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use monaka_core::sync::Locked;

pub mod game;
pub mod research;
pub mod ledger;
pub mod situation;

/// Win32 codes the loader understands.
pub mod code {
    pub const SUCCESS: u32 = 0;
    pub const NOT_READY: u32 = 21;
    pub const NOT_SUPPORTED: u32 = 50;
    pub const OPEN_FAILED: u32 = 110;
    pub const ALREADY_EXISTS: u32 = 183;
    pub const WAIT_TIMEOUT: u32 = 258;
    pub const REVISION_MISMATCH: u32 = 1306;
}

/// `MonakaStatus` values; the loader stops an older copy that reports [`State::Running`].
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    New = 0,
    Starting = 1,
    Running = 2,
    Stopped = 3,
    Rejected = 4,
    ShutdownFailed = 5,
    Stopping = 6,
}

/// Why a start was refused: a Win32 code for the loader and a reason for the log.
#[derive(Debug)]
pub struct Rejection {
    pub code: u32,
    pub reason: String,
}

impl Rejection {
    pub fn new(code: u32, reason: impl Into<String>) -> Self {
        Self { code, reason: reason.into() }
    }

    /// The game build or a required piece of it is not the one this producer was made for.
    pub fn revision(reason: impl Into<String>) -> Self {
        Self::new(code::REVISION_MISMATCH, reason)
    }

    pub fn unsupported(reason: impl Into<String>) -> Self {
        Self::new(code::NOT_SUPPORTED, reason)
    }

    pub fn not_ready(reason: impl Into<String>) -> Self {
        Self::new(code::NOT_READY, reason)
    }
}

/// A hook that cannot go in: unexpected code at its target means another game build.
impl From<monaka_hook::Error> for Rejection {
    fn from(e: monaka_hook::Error) -> Self {
        match e {
            monaka_hook::Error::Signature(_) => Rejection::revision(e.to_string()),
            _ => Rejection::unsupported(e.to_string()),
        }
    }
}

/// Switches prepared hooks on and keeps them in `slot` for [`remove_hooks`]. If they cannot all
/// go in, none stay in.
pub fn enable_hooks(slot: &Mutex<Option<Hooks>>, mut hooks: Hooks) -> Result<(), Rejection> {
    if let Err(e) = hooks.enable() {
        let _ = hooks.disable();
        return Err(e.into());
    }
    *slot.locked() = Some(hooks);
    Ok(())
}

/// Switches the hooks in `slot` off, then waits up to `idle_ms` for detours still running. A hook
/// someone else stacked on top of ours stays in their chain and only forwards from then on
/// (logged, not a failure); anything else, or detours that keep running, fails the stop.
pub fn remove_hooks(slot: &Mutex<Option<Hooks>>, idle_ms: u64) -> Result<(), Rejection> {
    if let Some(mut hooks) = slot.locked().take()
        && let Err(e) = hooks.disable()
    {
        match e {
            monaka_hook::Error::Refused(why) => log(format_args!("left in place: {why}")),
            other => return Err(Rejection::new(code::NOT_SUPPORTED, other.to_string())),
        }
    }
    if !InFlight::wait_idle(idle_ms) {
        return Err(Rejection::new(code::WAIT_TIMEOUT, "detours still running after the hooks were removed"));
    }
    Ok(())
}

/// Polls `done` every millisecond for up to `ms`; whether it became true.
pub fn wait_until(ms: u64, mut done: impl FnMut() -> bool) -> bool {
    let started = std::time::Instant::now();
    loop {
        if done() {
            return true;
        }
        if started.elapsed().as_millis() as u64 >= ms {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

/// Logs panics and the first faults anywhere in the game to the session log
/// ([`monaka_hook::crash`]); [`remove_crash_log`] at stop (a rejected start removes it itself).
pub fn install_crash_log() {
    monaka_hook::crash::install(log);
}

pub fn remove_crash_log() {
    monaka_hook::crash::remove();
}

/// The start/stop state machine behind the three exports.
pub struct Lifecycle(AtomicU32);

impl Lifecycle {
    pub const fn new() -> Self {
        Self(AtomicU32::new(State::New as u32))
    }

    pub fn status(&self) -> u32 {
        self.0.load(Ordering::Acquire)
    }

    fn advance(&self, from: State, to: State) -> bool {
        self.0.compare_exchange(from as u32, to as u32, Ordering::AcqRel, Ordering::Acquire).is_ok()
    }

    fn set(&self, state: State) {
        self.0.store(state as u32, Ordering::Release);
    }

    /// `MonakaStart`: opens the session (log file `log_name` in the run folder) and runs `start`
    /// once. A rejection is logged and leaves the copy inert, without the crash log `start` may
    /// have installed before it gave up.
    pub fn start(&self, log_name: &str, start: impl FnOnce(&'static Session) -> Result<(), Rejection>) -> u32 {
        if !self.advance(State::New, State::Starting) {
            return code::ALREADY_EXISTS;
        }
        let Some(session) = Session::open(log_name) else {
            self.set(State::Rejected);
            return code::OPEN_FAILED;
        };
        match start(session) {
            Ok(()) => {
                session.log(format_args!("started"));
                self.set(State::Running);
                code::SUCCESS
            }
            Err(rejection) => {
                remove_crash_log();
                session.log(format_args!("rejected: {}", rejection.reason));
                self.set(State::Rejected);
                rejection.code
            }
        }
    }

    /// `MonakaStop`: runs `stop` once from the running state. A failure means hooks may still be
    /// live, so this copy must never be started again.
    pub fn stop(&self, stop: impl FnOnce() -> Result<(), Rejection>) -> u32 {
        if !self.advance(State::Running, State::Stopping) {
            return code::NOT_READY;
        }
        match stop() {
            Ok(()) => {
                log(format_args!("stopped"));
                self.set(State::Stopped);
                code::SUCCESS
            }
            Err(failure) => {
                log(format_args!("stop failed: {}", failure.reason));
                self.set(State::ShutdownFailed);
                failure.code
            }
        }
    }
}

impl Default for Lifecycle {
    fn default() -> Self {
        Self::new()
    }
}

/// The run folder this DLL copy was started from, its option files and its log.
pub struct Session {
    dir: PathBuf,
    log: Mutex<Option<File>>,
    started: std::time::Instant,
}

static SESSION: OnceLock<Session> = OnceLock::new();

fn this_module_dir() -> Option<PathBuf> {
    let module = Module::containing(this_module_dir as fn() -> Option<PathBuf> as usize)?;
    Some(module.path()?.parent()?.to_path_buf())
}

impl Session {
    fn open(log_name: &str) -> Option<&'static Session> {
        Self::open_in(this_module_dir()?, log_name)
    }

    /// The session in `dir` (the DLL copy's own folder in a run; a test's folder in tests).
    fn open_in(dir: PathBuf, log_name: &str) -> Option<&'static Session> {
        let file = File::create(dir.join(log_name)).ok()?;
        let session = SESSION.get_or_init(|| Session { dir, log: Mutex::new(Some(file)), started: std::time::Instant::now() });
        Some(session)
    }

    /// The session, once a start has opened it.
    pub fn get() -> Option<&'static Session> {
        SESSION.get()
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// Whether the launcher left a marker file `name` (for example `something.enable`).
    pub fn has(&self, name: &str) -> bool {
        self.path(name).is_file()
    }

    pub fn text(&self, name: &str) -> Option<String> {
        std::fs::read_to_string(self.path(name)).ok()
    }

    /// `key=value` options from file `name`; missing means all defaults.
    pub fn options(&self, name: &str) -> Options {
        self.text(name).map(|text| Options::parse(&text)).unwrap_or_default()
    }

    /// The options the launcher wrote for `manifest` (its options file).
    pub fn manifest_options(&self, manifest: &manifest::Manifest) -> Options {
        self.options(manifest.options_file)
    }

    /// The GPU channel the launcher chose (`live-channel.txt`, a `Local\monaka-` name).
    pub fn channel(&self) -> Option<ChannelName> {
        let text = self.text("live-channel.txt")?;
        let name = text.trim();
        name.starts_with(r"Local\monaka-").then(|| ChannelName::new(name)).flatten()
    }

    /// [`Session::channel`], or the rejection a producer gives without one.
    pub fn require_channel(&self) -> Result<ChannelName, Rejection> {
        self.channel().ok_or_else(|| Rejection::unsupported(r"live-channel.txt names no Local\monaka- channel"))
    }

    pub fn log(&self, args: fmt::Arguments<'_>) {
        if let Ok(mut file) = self.log.lock()
            && let Some(file) = file.as_mut()
        {
            let _ = writeln!(file, "{:>9.3} {args}", self.started.elapsed().as_secs_f64());
            let _ = file.flush();
        }
    }
}

/// Writes one line to the session log, if a session is open.
pub fn log(args: fmt::Arguments<'_>) {
    if let Some(session) = Session::get() {
        session.log(args);
    }
}

/// `log!("...", args)`: one line in the session log.
#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => { $crate::log(format_args!($($arg)*)) };
}

/// `log_first!(limit, "...", args)`: logs only the first `limit` times this site runs.
#[macro_export]
macro_rules! log_first {
    ($limit:expr, $($arg:tt)*) => {{
        static N: ::std::sync::atomic::AtomicU32 = ::std::sync::atomic::AtomicU32::new(0);
        if N.fetch_add(1, ::std::sync::atomic::Ordering::Relaxed) < $limit {
            $crate::log!($($arg)*);
        }
    }};
}

/// Each counter's rate per second since the last call: `now` are the counters' values, `last`
/// what they were (updated to `now`), over `seconds`. For the periodic report lines.
pub fn rates<const N: usize>(now: [u64; N], last: &[std::sync::atomic::AtomicU64; N], seconds: f64) -> [f64; N] {
    std::array::from_fn(|i| now[i].saturating_sub(last[i].swap(now[i], std::sync::atomic::Ordering::Relaxed)) as f64 / seconds)
}

/// Declares a game crate's producer exports from its [`game::Game`]:
///
/// ```ignore
/// monaka_producer::producer!(game: crate::MyGame);
/// ```
///
/// generates `MonakaStart`, `MonakaStop` and `MonakaStatus` running the game through
/// [`game::start`] and [`game::stop`] in a [`Lifecycle`], refuses to start in any executable but
/// the manifests', logs to the manifests' log file, and exports `MONAKA_HOST` (the executables,
/// `;`-separated, NUL-terminated), `MONAKA_CONTRACT` and `MONAKA_MANIFEST`, which the loader and
/// the launcher read from the DLL file.
#[macro_export]
macro_rules! producer {
    (game: $game:ty) => {
        static LIFECYCLE: $crate::Lifecycle = $crate::Lifecycle::new();

        /// The game executables this producer belongs to (read by monaka_loader from the file).
        #[unsafe(no_mangle)]
        pub static MONAKA_HOST: [u8; $crate::manifest::host_len(<$game as $crate::game::Game>::MANIFESTS)] =
            $crate::manifest::host_bytes(<$game as $crate::game::Game>::MANIFESTS);

        /// The pack contract this producer was built against (`monaka_core::pack::CONTRACT`), read
        /// by the launcher from the file.
        #[unsafe(no_mangle)]
        pub static MONAKA_CONTRACT: u32 = $crate::pack::CONTRACT.packed();

        /// The manifests as text (`monaka_core::pack`), NUL-terminated, read by the launcher from
        /// the file: the game list, options and tabs come from the pack, and who made it from the
        /// game crate's `authors` and `repository`.
        #[unsafe(no_mangle)]
        pub static MONAKA_MANIFEST: [u8; $crate::pack::text_len(<$game as $crate::game::Game>::MANIFESTS, env!("CARGO_PKG_AUTHORS"), env!("CARGO_PKG_REPOSITORY"))] =
            $crate::pack::text_bytes(<$game as $crate::game::Game>::MANIFESTS, env!("CARGO_PKG_AUTHORS"), env!("CARGO_PKG_REPOSITORY"));

        /// Called by the loader on a new thread in the game.
        #[unsafe(no_mangle)]
        pub extern "system" fn MonakaStart(_: *mut ::core::ffi::c_void) -> u32 {
            let manifests = <$game as $crate::game::Game>::MANIFESTS;
            LIFECYCLE.start(manifests[0].log, |session| {
                $crate::require_host(&$crate::manifest::host_text(manifests))?;
                $crate::allow_other_builds(session.manifest_options(&manifests[0]).switch("other_builds", false));
                $crate::game::start::<$game>(session)
            })
        }

        #[unsafe(no_mangle)]
        pub extern "system" fn MonakaStop(_: *mut ::core::ffi::c_void) -> u32 {
            LIFECYCLE.stop($crate::game::stop::<$game>)
        }

        #[unsafe(no_mangle)]
        pub extern "system" fn MonakaStatus(_: *mut ::core::ffi::c_void) -> u32 {
            LIFECYCLE.status()
        }

        #[cfg(test)]
        mod producer_tests {
            /// The manifests are sound and the exported text reads back as the manifests (what
            /// each part offers is checked as the options are built, at compile time).
            #[test]
            fn manifests_are_sound() {
                let manifests = <$game as $crate::game::Game>::MANIFESTS;
                for manifest in manifests {
                    manifest.check().unwrap_or_else(|why| panic!("{why}"));
                    // Each option the manifest offers is read by the shared settings or a component.
                    let unread = $crate::manifest::unread_options(manifest, |options| $crate::game::read_all::<$game>(options, manifest));
                    assert!(unread.is_empty(), "{}: offered but read by nothing: {unread:?}", manifest.id);
                }
                let host = ::std::str::from_utf8(&super::MONAKA_HOST[..super::MONAKA_HOST.len() - 1]).unwrap();
                assert_eq!(host, $crate::manifest::host_text(manifests));
                let text = ::std::str::from_utf8(&super::MONAKA_MANIFEST[..super::MONAKA_MANIFEST.len() - 1]).unwrap();
                let read = $crate::pack::parse(text).unwrap_or_else(|why| panic!("{why}"));
                assert_eq!(read.contract, $crate::pack::CONTRACT);
                assert_eq!(read.about.source, env!("CARGO_PKG_REPOSITORY"));
                assert!(read.manifests.iter().eq(manifests.iter()), "the manifest text does not read back as the manifests");
            }
        }
    };
}

/// Checks a game module is the exact build this producer's offsets belong to.
pub fn require_build(name: &str, sha256: &str) -> Result<Module, Rejection> {
    let module = Module::find(name).ok_or_else(|| Rejection::not_ready(format!("{name} is not loaded")))?;
    let mismatch = match module.file_sha256() {
        Some(actual) if actual == sha256 => return Ok(module),
        Some(actual) => format!("{name} is build {actual}, not the inspected {sha256}"),
        None => format!("{name} could not be fingerprinted"),
    };
    if OTHER_BUILDS.load(std::sync::atomic::Ordering::Relaxed) {
        log!("{mismatch}; running anyway (other_builds): each hook still checks the code it patches, plain offsets are unchecked");
        return Ok(module);
    }
    Err(Rejection::revision(format!("{mismatch} (\"Allow other game versions\" on the Engine tab runs it anyway, at your own risk)")))
}

/// The game module for a producer that finds everything it needs in the code itself (rather than
/// at offsets fixed for one build): any build runs, its fingerprint logged, because finding
/// anything ambiguous refuses the start.
pub fn discovered_build(name: &str) -> Result<Module, Rejection> {
    let module = Module::find(name).ok_or_else(|| Rejection::not_ready(format!("{name} is not loaded")))?;
    log!("{name} build {}; engine functions are found in its code, not at fixed offsets", module.file_sha256().unwrap_or_else(|| "(unreadable)".into()));
    Ok(module)
}

/// Whether [`require_build`] lets an unknown build through (`other_builds=1`).
static OTHER_BUILDS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Sets whether [`require_build`] lets an unknown build through; the producer's start does it from
/// the options file before the game's own start.
pub fn allow_other_builds(allow: bool) {
    OTHER_BUILDS.store(allow, std::sync::atomic::Ordering::Relaxed);
}

/// Refuses to run in any process but the game's executable (`executables`: one name, or several
/// `;`-separated for a producer that serves several games on one engine).
pub fn require_host(executables: &str) -> Result<(), Rejection> {
    let host = monaka_hook::module::host_name();
    if monaka_core::pe::host_listed(executables, &host) { Ok(()) } else { Err(Rejection::unsupported(format!("host is {host}, not {executables}"))) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_build_runs_only_when_allowed() {
        let wrong = "0000000000000000000000000000000000000000000000000000000000000000";
        let Err(refused) = require_build("kernel32.dll", wrong) else { panic!("a wrong hash passed") };
        assert!(refused.reason.contains("Allow other game versions"), "{}", refused.reason);
        allow_other_builds(true);
        assert!(require_build("kernel32.dll", wrong).is_ok());
        allow_other_builds(false);
    }
}
