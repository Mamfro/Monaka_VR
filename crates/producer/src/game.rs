//! A game's producer as Monaka runs it. A game crate is an **adapter**: it says which build it is
//! ([`Game::prepare`]) and lists its **components** in the order they start ([`Adapter`]). A
//! component is how one provided method of a feature, or one of the adapter's extras, runs: the
//! features it provides (`monaka_core::feature`), the options its code honours, when it is on, its
//! install, its work each frame, and its part in each phase of the stop.
//!
//! The run is Monaka's ([`start`], [`stop`], from `producer!`):
//!
//! - **start**: the manifest the launcher meant, the shared settings, the build; each component's
//!   condition decided in order; the channel opened if a component that is on needs it; each on
//!   component installed in order into one set of hooks, with a [`Ledger`] of its own for what it
//!   changes in the game (a component this build cannot carry is left off, its features lost with
//!   the reason and its changes put back; one that cannot install at all refuses the start and the
//!   others are undone); the hooks enabled together; each component armed.
//! - **stop**, in phases, each over the components that are on in reverse order: **quiet** (the
//!   component is off from here: [`on`] answers no and its frame work stops; it stops starting new
//!   work), **give back** (each component's ledger put back, then its own state, while the hooks
//!   are in: a setting goes back when the component that changed it stops, so one listed before
//!   another outlasts its give-back), **unhook**, **release**. A component that may not have let go (a device the
//!   engine still calls) is asked after its give-back ([`Component::let_go`]); if it has not, the
//!   stop ends there, the hooks kept, as a failed hook removal does.
//!
//! Every step is logged with its component's name, so a start or a stop reads in order in the log.
//! The features a game has, and its manifest's options, are read from its components at compile
//! time ([`features`], [`options`]), and the rules about which component may offer which option are
//! checked as they are built: a game that breaks one does not build, with a message naming the
//! component and the option.

use crate::ledger::Ledger;
use crate::{Rejection, Session, enable_hooks, log, remove_hooks};
use monaka_core::feature::{Available, Feature, Features, Message, fault, repeated, same_str};
use monaka_contract::host;
use monaka_contract::manifest::{EXTRA, Kind, Manifest, OTHER_BUILDS, Opt, opt};
use monaka_core::options::{Options, Shared};
use monaka_core::rendering::StereoMethod;
use monaka_contract::protocol::ChannelName;
use monaka_core::situation::Situation;
use monaka_core::run::RunCell;
use monaka_hook::Hooks;
use std::any::Any;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::*};
use std::time::{Duration, Instant};
use monaka_core::sync::Locked;

/// A game's producer: its manifests, its components, and which build this process is.
pub trait Game: Sized + 'static {
    const MANIFESTS: &'static [Manifest];
    const ADAPTER: Adapter<Self>;
    /// The host features (`monaka_contract::host`) this game cannot work without: a Monaka without
    /// one refuses the pack and names it. Those it does more with are worked out from its
    /// components ([`host_uses`]).
    const REQUIRES: &'static [&'static str] = &[];
    /// This build's facts (addresses, layouts, the engine's objects), from [`Game::prepare`]; a
    /// detour reads them with [`build`].
    type Build: Send + Sync + 'static;
    /// What the game's per-frame hook hands the components' `frame` ([`frame`]).
    type Frame;

    /// Which build this process is (the module's fingerprint checked) and its facts, for
    /// `manifest` (the one the launcher meant). A build this producer does not know refuses the
    /// start.
    fn prepare(session: &'static Session, manifest: &'static Manifest, options: &Options) -> Result<Self::Build, Rejection>;
}

/// A game's components, in the order they start. The list is the adapter's table of contents.
pub struct Adapter<G: Game> {
    pub components: &'static [&'static Component<G>],
}

/// Whether a component is on for this run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum On {
    Yes,
    /// Off, and why (for the log: "aim is not motion controls").
    No(&'static str),
}

impl On {
    /// On when `on`, else off for `why_not`.
    pub const fn when(on: bool, why_not: &'static str) -> On {
        if on { On::Yes } else { On::No(why_not) }
    }
}

/// Why a component did not install.
#[derive(Debug)]
pub enum Lack {
    /// This build cannot carry it (a layout missing from its table, code not where it was
    /// measured): it stays off, its features lost for the run, and the run goes on.
    Missing(String),
    /// Nothing can run (a hook that would not go in): the start is refused.
    Fatal(Rejection),
}

impl From<Rejection> for Lack {
    fn from(rejection: Rejection) -> Self {
        Lack::Fatal(rejection)
    }
}

impl From<monaka_hook::Error> for Lack {
    fn from(error: monaka_hook::Error) -> Self {
        Lack::Fatal(error.into())
    }
}

/// A result a component can do without: an error leaves it off for this build, the error its
/// reason (`result.missing()?` in an install).
pub trait Missing<T> {
    fn missing(self) -> Result<T, Lack>;
}

impl<T> Missing<T> for Result<T, Rejection> {
    fn missing(self) -> Result<T, Lack> {
        self.map_err(|why| Lack::Missing(why.reason))
    }
}

impl<T> Missing<T> for Result<T, String> {
    fn missing(self) -> Result<T, Lack> {
        self.map_err(Lack::Missing)
    }
}

impl<T> Missing<T> for Result<T, monaka_hook::Error> {
    fn missing(self) -> Result<T, Lack> {
        self.map_err(|e| Lack::Missing(e.to_string()))
    }
}

/// Something a component needs that this build may not have: without it the component is off for
/// this build, for `why` (`facts.gamedll.as_ref().or_missing("no game DLL")?`).
pub trait OrMissing<T> {
    fn or_missing(self, why: &str) -> Result<T, Lack>;
}

impl<T> OrMissing<T> for Option<T> {
    fn or_missing(self, why: &str) -> Result<T, Lack> {
        self.ok_or_else(|| Lack::Missing(why.to_owned()))
    }
}

/// One piece of a run. A `const` value; [`Component::NONE`] gives every default, so a component
/// states only what it does (struct update syntax).
pub struct Component<G: Game> {
    /// Its name in the log and in compile-time messages; unique in its adapter.
    pub name: &'static str,
    /// The features it provides (none for an extra, or for infrastructure several features use).
    pub features: &'static [Feature],
    /// The situations its code reports (`crate::situation::report`).
    pub reports: &'static [Situation],
    /// The options its code honours, as the manifest offers them.
    pub options: &'static [Opt],
    /// The games (manifest ids) it serves; empty: every game of the producer.
    pub games: &'static [&'static str],
    /// The stereo methods it serves (`monaka_core::rendering`); empty: every method. Off for a run
    /// of another method ([`method`]).
    pub methods: &'static [StereoMethod],
    /// What it works on top of, each resolved for the running game to one earlier component
    /// ([`Need`]): off when any of them is.
    pub needs: &'static [Need<G>],
    /// It needs the run's channel (publishing, the pose channel, the controllers).
    pub needs_channel: bool,
    /// Reads its own options (also what the manifest test calls, to prove each option is read).
    pub settings: fn(&Options, &Manifest),
    /// The switch that turns it off ([`Switch`]), before its own condition is asked.
    pub switch: Option<Switch>,
    /// Whether it is on for this run (its `needs` already on).
    pub on: fn(&Run<'_, G>) -> On,
    /// Makes it for this run: its state, its hooks into `start.hooks`, its game changes into
    /// `start.ledger`.
    pub install: fn(&mut Start<'_, G>) -> Result<(), Lack>,
    /// Every hook is in: it starts acting.
    pub arm: fn(),
    /// Its work in the game's per-frame hook ([`frame`]).
    pub frame: Option<fn(&G::Frame)>,
    /// Its work on a timer while it is on, off the game's threads ([`Every`]: counters logged,
    /// probes polled).
    pub every: &'static [Every],
    /// The stop's first phase, once it is off ([`on`] answers no): what else stops new work.
    pub quiet: fn(),
    /// The stop's second phase: what it changed in the game put back, the hooks still in.
    pub give_back: fn(),
    /// Asked after its give-back: whether it let go (a device the engine still calls detached).
    /// Refused, the stop ends there and the hooks stay in.
    pub let_go: Option<fn() -> Result<(), Rejection>>,
    /// After the hooks are out (and after a start undone): what it holds freed.
    pub release: fn(),
    /// A line (or several) for the stop's log; an empty one is left out.
    pub summary: Option<fn() -> String>,
}

/// What a component works on top of. A ready-made says its needs by feature, once, so a game lists
/// it without restating them; a game whose need differs replaces them by struct update.
///
/// Each need resolves, for every game the component serves, to one earlier component serving that
/// game; one that does not resolve does not build, the message naming the components:
///
/// ```
/// use monaka_producer::game::*;
/// use monaka_producer::{Rejection, Session};
/// use monaka_contract::manifest::{Manifest, Opt};
/// use monaka_core::feature::Feature;
/// use monaka_core::options::Options;
///
/// struct Good;
/// const RIG: Component<Good> = Component { name: "rig", features: &[Feature::HandRig], ..Component::NONE };
/// const FINGERS: Component<Good> = Component { name: "fingers", needs: &[Need::Feature(Feature::HandRig)], ..Component::NONE };
/// impl Game for Good {
///     const MANIFESTS: &'static [Manifest] = &[];
///     const ADAPTER: Adapter<Good> = Adapter { components: &[&RIG, &FINGERS] };
///     type Build = ();
///     type Frame = ();
///     fn prepare(_: &'static Session, _: &'static Manifest, _: &Options) -> Result<(), Rejection> { Ok(()) }
/// }
/// const OPTIONS: [Opt; options_len::<Good>("good")] = options::<Good, { options_len::<Good>("good") }>("good");
/// ```
///
/// No earlier component provides the feature (here: the rig is listed after):
///
/// ```compile_fail
/// # use monaka_producer::game::*;
/// # use monaka_producer::{Rejection, Session};
/// # use monaka_contract::manifest::{Manifest, Opt};
/// # use monaka_core::feature::Feature;
/// # use monaka_core::options::Options;
/// struct Bad;
/// const RIG: Component<Bad> = Component { name: "rig", features: &[Feature::HandRig], ..Component::NONE };
/// const FINGERS: Component<Bad> = Component { name: "fingers", needs: &[Need::Feature(Feature::HandRig)], ..Component::NONE };
/// impl Game for Bad {
///     const MANIFESTS: &'static [Manifest] = &[];
///     const ADAPTER: Adapter<Bad> = Adapter { components: &[&FINGERS, &RIG] };
///     type Build = ();
///     type Frame = ();
///     fn prepare(_: &'static Session, _: &'static Manifest, _: &Options) -> Result<(), Rejection> { Ok(()) }
/// }
/// const OPTIONS: [Opt; options_len::<Bad>("bad")] = options::<Bad, { options_len::<Bad>("bad") }>("bad");
/// ```
///
/// Two earlier components provide it: the game names one with [`Need::On`]:
///
/// ```compile_fail
/// # use monaka_producer::game::*;
/// # use monaka_producer::{Rejection, Session};
/// # use monaka_contract::manifest::{Manifest, Opt};
/// # use monaka_core::feature::Feature;
/// # use monaka_core::options::Options;
/// struct Bad;
/// const GUNS: Component<Bad> = Component { name: "guns", features: &[Feature::HandRig], ..Component::NONE };
/// const RIG: Component<Bad> = Component { name: "rig", features: &[Feature::HandRig], ..Component::NONE };
/// const FINGERS: Component<Bad> = Component { name: "fingers", needs: &[Need::Feature(Feature::HandRig)], ..Component::NONE };
/// impl Game for Bad {
///     const MANIFESTS: &'static [Manifest] = &[];
///     const ADAPTER: Adapter<Bad> = Adapter { components: &[&GUNS, &RIG, &FINGERS] };
///     type Build = ();
///     type Frame = ();
///     fn prepare(_: &'static Session, _: &'static Manifest, _: &Options) -> Result<(), Rejection> { Ok(()) }
/// }
/// const OPTIONS: [Opt; options_len::<Bad>("bad")] = options::<Bad, { options_len::<Bad>("bad") }>("bad");
/// ```
///
/// The provider serves fewer games than the component on top of it:
///
/// ```compile_fail
/// # use monaka_producer::game::*;
/// # use monaka_producer::{Rejection, Session};
/// # use monaka_contract::manifest::{Manifest, Opt};
/// # use monaka_core::feature::Feature;
/// # use monaka_core::options::Options;
/// struct Bad;
/// const RIG: Component<Bad> = Component { name: "rig", features: &[Feature::HandRig], games: &["two"], ..Component::NONE };
/// const FINGERS: Component<Bad> = Component { name: "fingers", needs: &[Need::Feature(Feature::HandRig)], ..Component::NONE };
/// impl Game for Bad {
///     const MANIFESTS: &'static [Manifest] = &[];
///     const ADAPTER: Adapter<Bad> = Adapter { components: &[&RIG, &FINGERS] };
///     type Build = ();
///     type Frame = ();
///     fn prepare(_: &'static Session, _: &'static Manifest, _: &Options) -> Result<(), Rejection> { Ok(()) }
/// }
/// const OPTIONS: [Opt; options_len::<Bad>("one")] = options::<Bad, { options_len::<Bad>("one") }>("one");
/// ```
///
/// A named need listed after:
///
/// ```compile_fail
/// # use monaka_producer::game::*;
/// # use monaka_producer::{Rejection, Session};
/// # use monaka_contract::manifest::{Manifest, Opt};
/// # use monaka_core::options::Options;
/// struct Bad;
/// const A: Component<Bad> = Component { name: "a", ..Component::NONE };
/// const B: Component<Bad> = Component { name: "b", needs: &[Need::On(&A)], ..Component::NONE };
/// impl Game for Bad {
///     const MANIFESTS: &'static [Manifest] = &[];
///     const ADAPTER: Adapter<Bad> = Adapter { components: &[&B, &A] };
///     type Build = ();
///     type Frame = ();
///     fn prepare(_: &'static Session, _: &'static Manifest, _: &Options) -> Result<(), Rejection> { Ok(()) }
/// }
/// const OPTIONS: [Opt; options_len::<Bad>("bad")] = options::<Bad, { options_len::<Bad>("bad") }>("bad");
/// ```
pub enum Need<G: Game> {
    /// Whichever earlier component provides this feature for the running game; exactly one must.
    Feature(Feature),
    /// This component (a need the feature does not say, or a pick between two providers).
    On(&'static Component<G>),
}

/// Why a need does not resolve (each a build error through [`options`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unresolved {
    /// No component in the list provides the feature for the game.
    NoProvider,
    /// The one that provides it, or the named one, is listed at this place, after the component.
    ListedAfter(usize),
    /// Both these places provide it.
    TwoProviders(usize, usize),
    /// The named component, at this place, does not serve the game.
    NotServing(usize),
    /// The adapter does not list the named component.
    NotListed,
}

/// Where the need `need` of the component at `at` in `G`'s adapter resolves for `game`: the place
/// of the one component it means.
pub const fn resolve<G: Game>(game: &str, at: usize, need: &Need<G>) -> Result<usize, Unresolved> {
    let list = G::ADAPTER.components;
    match need {
        Need::On(wanted) => {
            let mut k = 0;
            while k < list.len() {
                if same_str(list[k].name, wanted.name) {
                    return if k >= at {
                        Err(Unresolved::ListedAfter(k))
                    } else if !list[k].serves(game) {
                        Err(Unresolved::NotServing(k))
                    } else {
                        Ok(k)
                    };
                }
                k += 1;
            }
            Err(Unresolved::NotListed)
        }
        Need::Feature(feature) => {
            let mut found = None;
            let mut k = 0;
            while k < at {
                if list[k].serves(game) && list[k].provides(*feature) {
                    if let Some(first) = found {
                        return Err(Unresolved::TwoProviders(first, k));
                    }
                    found = Some(k);
                }
                k += 1;
            }
            if let Some(place) = found {
                return Ok(place);
            }
            let mut later = at + 1;
            while later < list.len() {
                if list[later].serves(game) && list[later].provides(*feature) {
                    return Err(Unresolved::ListedAfter(later));
                }
                later += 1;
            }
            Err(Unresolved::NoProvider)
        }
    }
}

/// The build error for a need of the component at `at` that does not resolve for `game`.
const fn unresolved<G: Game>(game: &str, at: usize, need: &Need<G>, why: Unresolved) -> ! {
    let list = G::ADAPTER.components;
    let named = match need {
        Need::Feature(feature) => Message::new().push("component `").push(list[at].name).push("` needs ").push(feature.name()),
        Need::On(wanted) => Message::new().push("component `").push(list[at].name).push("` needs `").push(wanted.name).push("`"),
    };
    let message = match why {
        Unresolved::NoProvider => named.push(", which no component provides for game `").push(game).push("`"),
        Unresolved::ListedAfter(k) => match need {
            Need::Feature(_) => named.push(", which `").push(list[k].name).push("` provides but is listed after it"),
            Need::On(_) => named.push(", which is listed after it"),
        },
        Unresolved::TwoProviders(a, b) => named.push(", which both `").push(list[a].name).push("` and `").push(list[b].name).push("` provide for game `").push(game).push("`: name one with Need::On"),
        Unresolved::NotServing(k) => named.push(": `").push(list[k].name).push("` does not serve game `").push(game).push("`; say `games` on this one too"),
        Unresolved::NotListed => named.push(", which the adapter does not list"),
    };
    panic!("{}", message.text())
}

/// A summary of several lines ([`Component::summary`]): the non-empty ones, one per line.
pub fn lines(lines: &[String]) -> String {
    lines.iter().filter(|line| !line.is_empty()).map(String::as_str).collect::<Vec<_>>().join("\n")
}

fn always<G: Game>(_: &Run<'_, G>) -> On {
    On::Yes
}

fn nothing<G: Game>(_: &mut Start<'_, G>) -> Result<(), Lack> {
    Ok(())
}

impl<G: Game> Component<G> {
    pub const NONE: Self = Component {
        name: "",
        features: &[],
        reports: &[],
        options: &[],
        games: &[],
        methods: &[],
        needs: &[],
        needs_channel: false,
        settings: |_, _| {},
        switch: None,
        on: always::<G>,
        install: nothing::<G>,
        arm: || {},
        frame: None,
        every: &[],
        quiet: || {},
        give_back: || {},
        let_go: None,
        release: || {},
        summary: None,
    };

    const fn serves(&self, game: &str) -> bool {
        let mut i = 0;
        while i < self.games.len() {
            if same_str(self.games[i], game) {
                return true;
            }
            i += 1;
        }
        self.games.is_empty()
    }

    const fn provides(&self, feature: Feature) -> bool {
        let mut i = 0;
        while i < self.features.len() {
            if self.features[i].same(feature) {
                return true;
            }
            i += 1;
        }
        false
    }
}

/// Timed work of a component: `tick(seconds since its last tick)` every `period`, on the run's one
/// timer thread, from its arm until the stop begins.
#[derive(Clone, Copy)]
pub struct Every {
    pub period: Duration,
    pub tick: fn(f64),
}

impl Every {
    pub const fn millis(period: u64, tick: fn(f64)) -> Self {
        Self { period: Duration::from_millis(period), tick }
    }
}

/// The timer's finest step: a period shorter than this ticks this often.
const TIMER_STEP: Duration = Duration::from_millis(50);

/// The run's timer: each armed component's [`Every`], ticked while it is on.
struct Timer {
    stopping: Arc<AtomicBool>,
    thread: std::thread::JoinHandle<()>,
}

impl Timer {
    /// Starts the timer for the components armed in `bits`; none when none has timed work.
    fn start<G: Game>(bits: u64) -> Option<Timer> {
        let work: Vec<(usize, Every)> = armed::<G>(bits).into_iter().flat_map(|(place, c)| c.every.iter().map(move |e| (place, *e))).collect();
        if work.is_empty() {
            return None;
        }
        let stopping = Arc::new(AtomicBool::new(false));
        let stop = stopping.clone();
        let thread = std::thread::Builder::new().name("monaka timer".into()).spawn(move || {
            let mut last: Vec<Instant> = vec![Instant::now(); work.len()];
            while !stop.load(Acquire) {
                std::thread::sleep(TIMER_STEP);
                for ((place, every), last) in work.iter().zip(&mut last) {
                    let since = last.elapsed();
                    if since >= every.period && on::<G>(*place) {
                        (every.tick)(since.as_secs_f64());
                        *last = Instant::now();
                    }
                }
            }
        });
        match thread {
            Ok(thread) => Some(Timer { stopping, thread }),
            Err(e) => {
                log!("no timer thread ({e}): timed work off");
                None
            }
        }
    }

    fn stop(self) {
        self.stopping.store(true, Release);
        let _ = self.thread.join();
    }
}

/// An option that turns a component off: `id` (its default the manifest's when the manifest offers
/// it, else `default`), and why the component is off when it is.
#[derive(Clone, Copy, Debug)]
pub struct Switch {
    pub id: &'static str,
    pub default: bool,
    pub off: &'static str,
}

impl Switch {
    pub const fn new(id: &'static str, default: bool, off: &'static str) -> Self {
        Self { id, default, off }
    }

    /// Whether it is on for `options` of `manifest`.
    pub fn on(&self, options: &Options, manifest: &Manifest) -> bool {
        options.switch(self.id, manifest.switch_default(self.id).unwrap_or(self.default))
    }
}

/// The research probes as a ready-made component: the options every game's probes ride on (other
/// builds, extra key=value options). A game adds its probes' hooks, threads and reports by struct
/// update (`Component { install: install_probes, ..probes() }`); each probe checks its own option
/// ([`crate::research::Research`]).
pub const fn probes<G: Game>() -> Component<G> {
    Component { name: "probes", options: &[OTHER_BUILDS, EXTRA], ..Component::NONE }
}

/// What a component's condition sees.
pub struct Run<'a, G: Game> {
    pub manifest: &'static Manifest,
    pub options: &'a Options,
    pub shared: &'a Shared,
    pub build: &'a G::Build,
    /// The run's stereo method ([`method`]).
    pub method: Option<StereoMethod>,
    /// The components decided so far (earlier in the list), and whether each is on.
    decided: &'a [(&'static str, bool)],
}

impl<G: Game> Run<'_, G> {
    /// Whether `component`, earlier in the list, is on.
    pub fn is_on(&self, component: &Component<G>) -> bool {
        self.decided.iter().any(|(name, on)| *on && *name == component.name)
    }
}

/// What a component has at its install.
pub struct Start<'a, G: Game> {
    pub session: &'static Session,
    pub manifest: &'static Manifest,
    pub options: &'a Options,
    pub shared: &'a Shared,
    pub build: &'a G::Build,
    pub hooks: &'a mut Hooks,
    /// What it changes in the game, put back at its own give-back (on the loader's thread; a
    /// change the game's own thread must undo is the component's own give-back).
    pub ledger: &'a mut Ledger,
    channel: Option<&'a ChannelName>,
    installed: &'a [&'static str],
}

impl<'a, G: Game> Start<'a, G> {
    /// The run's channel (a component that uses it says `needs_channel`).
    pub fn channel(&self) -> Result<&'a ChannelName, Lack> {
        self.channel.ok_or_else(|| Lack::Fatal(Rejection::unsupported("this component needs the run's channel and does not say so (needs_channel)")))
    }

    /// Whether `component`, earlier in the list, installed.
    pub fn is_on(&self, component: &Component<G>) -> bool {
        self.installed.contains(&component.name)
    }
}

/// The host features (`monaka_contract::host`) `G` does more with, from the features its
/// components provide in any of its games, each with whether it does: the pack's `uses`.
pub const fn host_uses<G: Game>() -> [(&'static str, bool); 4] {
    let mut has = Features::NONE;
    let mut i = 0;
    while i < G::MANIFESTS.len() {
        let game = features::<G>(G::MANIFESTS[i].id);
        let mut f = 0;
        while f < Feature::ALL.len() {
            if game.has(Feature::ALL[f]) {
                has = has.with(Feature::ALL[f]);
            }
            f += 1;
        }
        i += 1;
    }
    [
        (host::DEPTH, has.has(Feature::DepthLayer)),
        (host::PANELS, has.has(Feature::HudPanels)),
        (host::FINGERS, has.has(Feature::FingerRig)),
        (host::RETICLES, has.has(Feature::Bow)),
    ]
}

/// The features `G` provides for `game` (a manifest id): those of the components that serve it.
pub const fn features<G: Game>(game: &str) -> Features {
    let mut has = Features::NONE;
    let mut i = 0;
    while i < G::ADAPTER.components.len() {
        let component = G::ADAPTER.components[i];
        if component.serves(game) {
            has = has.with_all(component.features);
        }
        i += 1;
    }
    has
}

/// How many options `G`'s components offer `game`: the size of its manifest's option array.
pub const fn options_len<G: Game>(game: &str) -> usize {
    let mut n = 0;
    let mut i = 0;
    while i < G::ADAPTER.components.len() {
        if G::ADAPTER.components[i].serves(game) {
            n += G::ADAPTER.components[i].options.len();
        }
        i += 1;
    }
    n
}

/// `game`'s manifest options: those of `G`'s components that serve it, in their order. `N` is
/// [`options_len`]:
///
/// `const KCD2: [Opt; options_len::<Kcd>("kcd2")] = options::<Kcd, { options_len::<Kcd>("kcd2") }>("kcd2");`
///
/// Built at compile time, it checks the components as it goes, and a game that breaks a rule
/// does not build, the message naming the component and the option
/// ([`monaka_core::feature::fault`]): a shared option offered by a component that does not provide
/// its feature, a game's own option under a shared option's id, an `aim` choice without the feature
/// that carries it, one option offered twice, two components of one name.
///
/// ```compile_fail
/// use monaka_producer::game::*;
/// use monaka_producer::{Rejection, Session};
/// use monaka_contract::manifest::{HUD_SIZE, Manifest, Opt};
/// use monaka_core::options::Options;
///
/// struct Bad;
/// const MINE: Component<Bad> = Component { name: "mine", options: &[HUD_SIZE], ..Component::NONE };
/// impl Game for Bad {
///     const MANIFESTS: &'static [Manifest] = &[];
///     const ADAPTER: Adapter<Bad> = Adapter { components: &[&MINE] };
///     type Build = ();
///     type Frame = ();
///     fn prepare(_: &'static Session, _: &'static Manifest, _: &Options) -> Result<(), Rejection> { Ok(()) }
/// }
/// const OPTIONS: [Opt; options_len::<Bad>("bad")] = options::<Bad, { options_len::<Bad>("bad") }>("bad");
/// ```
pub const fn options<G: Game, const N: usize>(game: &str) -> [Opt; N] {
    let has = features::<G>(game);
    let components = G::ADAPTER.components;
    assert!(components.len() <= 64, "an adapter lists at most 64 components");
    let mut i = 0;
    while i < components.len() {
        let component = components[i];
        if component.serves(game)
            && let Some(fault) = fault(component.name, component.features, component.options, has)
        {
            panic!("{}", fault.text());
        }
        let mut j = i + 1;
        while j < components.len() {
            if same_str(component.name, components[j].name) {
                panic!("{}", Message::new().push("two components are named `").push(component.name).push("`").text());
            }
            j += 1;
        }
        if component.serves(game) {
            let mut n = 0;
            while n < component.needs.len() {
                if let Err(why) = resolve::<G>(game, i, &component.needs[n]) {
                    unresolved::<G>(game, i, &component.needs[n], why);
                }
                n += 1;
            }
        }
        i += 1;
    }
    const BLANK: Opt = opt("", "", "", Kind::Text);
    let mut every: [&[Opt]; 64] = [&[]; 64];
    let mut out = [BLANK; N];
    let mut at = 0;
    i = 0;
    while i < components.len() {
        if components[i].serves(game) {
            every[i] = components[i].options;
            let mut k = 0;
            while k < components[i].options.len() {
                out[at] = components[i].options[k];
                at += 1;
                k += 1;
            }
        }
        i += 1;
    }
    if let Some(id) = repeated(&every) {
        panic!("{}", Message::new().push("the option `").push(id).push("` is offered by two components").text());
    }
    assert!(at == N, "options::<G, N>(game): N must be options_len::<G>(game)");
    out
}

/// `options` with `changed` in place of the option of its id: one game's own presentation of an
/// option its components offer every game they serve (a default, a range, a help line), never
/// another owner:
///
/// `const DOO: [Opt; N] = adjust(options::<Dishonored, N>("dis_doo"), LATENCY.default_number(1.0));`
pub const fn adjust<const N: usize>(mut options: [Opt; N], changed: Opt) -> [Opt; N] {
    let mut i = 0;
    while i < N {
        if same_str(options[i].id, changed.id) {
            if options[i].shared != changed.shared {
                panic!("{}", Message::new().push("adjust: `").push(changed.id).push("` changes whether it is shared").text());
            }
            options[i] = changed;
            return options;
        }
        i += 1;
    }
    panic!("{}", Message::new().push("adjust: no option `").push(changed.id).push("` is offered").text());
}

/// [`adjust`] for each of `changed` in turn: one game's presentation of several options.
///
/// `const ASCENT: [Opt; N] = adjust_all(options::<Ascent, N>("ascent"), &[HEADSET.default_switch(true), PUBLISH.default_switch(true)]);`
pub const fn adjust_all<const N: usize>(mut options: [Opt; N], changed: &[Opt]) -> [Opt; N] {
    let mut i = 0;
    while i < changed.len() {
        options = adjust(options, changed[i]);
        i += 1;
    }
    options
}

/// Reads every option `G`'s components that serve `manifest` read: the shared settings and each
/// component's own (`producer!`'s manifest test: each offered option is read by something).
pub fn read_all<G: Game>(options: &Options, manifest: &Manifest) {
    Shared::from_options(options, manifest);
    for component in G::ADAPTER.components.iter().filter(|c| c.serves(manifest.id)) {
        (component.settings)(options, manifest);
        if let Some(switch) = component.switch {
            switch.on(options, manifest);
        }
    }
}

/// Which components are on, one bit each by their place in the adapter.
static ON: AtomicU64 = AtomicU64::new(0);

/// The place of `component` in `G`'s adapter, found at compile time (one the adapter does not
/// list does not build), for [`on`]:
///
/// `const FINGERS_AT: usize = place::<Kcd>(&FINGERS);`
pub const fn place<G: Game>(component: &Component<G>) -> usize {
    place_named::<G>(component.name)
}

/// [`place`] by name, for a component listed inline in the adapter (`&fingers()`):
///
/// `const FINGERS_AT: usize = place_named::<Dl2>("fingers");`
pub const fn place_named<G: Game>(name: &str) -> usize {
    let mut i = 0;
    while i < G::ADAPTER.components.len() {
        if same_str(G::ADAPTER.components[i].name, name) {
            return i;
        }
        i += 1;
    }
    panic!("{}", Message::new().push("the adapter does not list `").push(name).push("`").text());
}

/// Whether the component at `place` ([`place`]) is on in this run: one atomic, for a detour or a
/// frame's work that must ask.
pub fn on<G: Game>(place: usize) -> bool {
    place < G::ADAPTER.components.len() && ON.load(Acquire) & (1 << place) != 0
}

/// Whether a component providing `feature` is on in this run (a ready-made asking whether another
/// feature it works with runs: the hand rig, for one).
pub fn feature_on<G: Game>(feature: Feature) -> bool {
    provided_by_on(G::ADAPTER.components, ON.load(Acquire), feature)
}

/// Whether one of `components` providing `feature` is on, by the bits of those on (`on`).
fn provided_by_on<G: Game>(components: &[&Component<G>], on: u64, feature: Feature) -> bool {
    components.iter().enumerate().any(|(place, component)| component.features.contains(&feature) && on & (1 << place) != 0)
}

/// The stereo methods, in [`METHOD`]'s numbering (0: none).
const METHODS: [StereoMethod; 4] = [StereoMethod::SameFrame, StereoMethod::Aer, StereoMethod::Afw, StereoMethod::DepthStereo];

/// The run's stereo method, by [`METHODS`] place plus one (0: the game offers no choice).
static METHOD: AtomicUsize = AtomicUsize::new(0);

/// The stereo method the run's options pick among those the manifest's `stereo` option offers
/// (`monaka_core::rendering::stereo_method`); none for a game without that choice.
fn chosen_method(manifest: &Manifest, options: &Options) -> Option<StereoMethod> {
    let opt = manifest.option("stereo")?;
    matches!(opt.kind, Kind::Choice { .. }).then(|| StereoMethod::chosen(opt, options))
}

/// This run's stereo method, from anywhere (a detour, a frame's work); none before the start reads
/// it, or for a game that offers no choice.
pub fn method() -> Option<StereoMethod> {
    METHODS.get(METHOD.load(Acquire).checked_sub(1)?).copied()
}

/// The running game's build, kept for its run: one DLL is one [`Game`], so one untyped cell
/// serves, and [`build`] checks the type.
static BUILD: RunCell<dyn Any + Send + Sync> = RunCell::new();

/// This run's build ([`Game::prepare`]'s facts), from anywhere: a detour, a timer, a frame. None
/// before the start has prepared it, or for another game's type.
pub fn build<G: Game>() -> Option<Arc<G::Build>> {
    BUILD.get()?.downcast::<G::Build>().ok()
}

/// The game's per-frame hook: each component that is on does its frame's work, in order.
pub fn frame<G: Game>(frame: &G::Frame) {
    let bits = ON.load(Acquire);
    for (i, component) in G::ADAPTER.components.iter().enumerate() {
        if bits & (1 << i) != 0
            && let Some(work) = component.frame
        {
            work(frame);
        }
    }
}

/// A run in progress: what the stop needs.
struct Running {
    hooks: Mutex<Option<Hooks>>,
    /// Each armed component's changes to the game, by its place.
    ledgers: Vec<(usize, Ledger)>,
    available: Available,
    /// The components armed at the start (their [`ON`] bits then), whatever is on now.
    armed: u64,
    timer: Option<Timer>,
}

static RUN: Mutex<Option<Running>> = Mutex::new(None);

/// The manifest the launcher meant: the one whose options file it wrote beside the DLL (several
/// games served by one producer each have their own), else the first.
fn meant<G: Game>(session: &Session) -> &'static Manifest {
    G::MANIFESTS.iter().find(|m| session.has(m.options_file)).unwrap_or(&G::MANIFESTS[0])
}

/// Runs one step of a component, logged with its name and, when slow, how long it took.
fn step<R>(component: &str, what: &str, work: impl FnOnce() -> R) -> R {
    let started = Instant::now();
    let result = work();
    let took = started.elapsed().as_millis();
    if took >= 20 {
        log!("{component}: {what} ({took} ms)");
    } else {
        log!("{component}: {what}");
    }
    result
}

/// The names of the components the component at `place` needs in `game` (each need resolved; one
/// that does not resolve cannot build, and reads here as needing what it named).
fn needed<G: Game>(game: &'static str, place: usize, component: &'static Component<G>) -> impl Iterator<Item = &'static str> {
    component.needs.iter().map(move |need| match resolve::<G>(game, place, need) {
        Ok(k) => G::ADAPTER.components[k].name,
        Err(_) => match need {
            Need::Feature(feature) => feature.name(),
            Need::On(wanted) => wanted.name,
        },
    })
}

/// The components that were armed in this run, with their places, in order.
fn armed<G: Game>(bits: u64) -> Vec<(usize, &'static Component<G>)> {
    G::ADAPTER.components.iter().copied().enumerate().filter(|(i, _)| bits & (1 << i) != 0).collect()
}

/// Starts `G`'s run (`producer!`'s `MonakaStart`).
pub fn start<G: Game>(session: &'static Session) -> Result<(), Rejection> {
    // A new run: what the last one left in run cells and states reads as fresh from here.
    monaka_core::run::begin();
    let manifest = meant::<G>(session);
    let options = session.manifest_options(manifest);
    let shared = Shared::from_options(&options, manifest);
    let build = Arc::new(G::prepare(session, manifest, &options)?);
    BUILD.set_arc(build.clone());
    let build: &G::Build = &build;
    let serving: Vec<(usize, &'static Component<G>)> = G::ADAPTER.components.iter().copied().enumerate().filter(|(_, c)| c.serves(manifest.id)).collect();
    let reports: Vec<Situation> = serving.iter().flat_map(|(_, c)| c.reports.iter().copied()).collect();
    let mut available = Available::new(features::<G>(manifest.id), &reports);
    crate::situation::accept(reports);

    let method = chosen_method(manifest, &options);
    METHOD.store(method.map_or(0, |m| METHODS.iter().position(|k| *k == m).map_or(0, |i| i + 1)), Release);

    // Each component's condition, in order: a later one may ask about an earlier one.
    let mut decided: Vec<(&'static str, bool)> = Vec::new();
    for (place, component) in &serving {
        let run = Run::<G> { manifest, options: &options, shared: &shared, build, method, decided: &decided };
        let missing = needed::<G>(manifest.id, *place, component).find(|need| !decided.contains(&(need, true)));
        let other_method = !component.methods.is_empty() && !method.is_some_and(|m| component.methods.contains(&m));
        let switched_off = component.switch.filter(|switch| !switch.on(&options, manifest));
        let off = match (missing, other_method, switched_off) {
            (Some(need), _, _) => Some(format!("needs {need}")),
            (None, true, _) => Some(match method {
                Some(m) => format!("the stereo method is {}", m.label()),
                None => "the game has no stereo method".to_owned(),
            }),
            (None, false, Some(switch)) => Some(switch.off.to_owned()),
            (None, false, None) => match (component.on)(&run) {
                On::No(why) => Some(why.to_owned()),
                On::Yes => None,
            },
        };
        if let Some(why) = &off {
            log!("{}: off ({why})", component.name);
            for feature in component.features {
                available.off(*feature, why.clone());
            }
        }
        decided.push((component.name, off.is_none()));
    }
    let wanted: Vec<(usize, &'static Component<G>)> = serving.iter().zip(&decided).filter(|(_, (_, on))| *on).map(|(c, _)| *c).collect();
    let channel = if wanted.iter().any(|(_, c)| c.needs_channel) { Some(session.require_channel()?) } else { session.channel() };
    if let Some(channel) = &channel {
        crate::debug::open(channel);
        crate::debug::game_settings(&monaka_core::rendering::summary(manifest.options, &options));
    }
    crate::install_crash_log();

    let mut hooks = Hooks::default();
    let mut installed: Vec<(usize, &'static Component<G>, Ledger)> = Vec::new();
    for (index, component) in wanted {
        let names: Vec<&'static str> = installed.iter().map(|(_, c, _)| c.name).collect();
        // A component it needs did not install (this build lacks it): this one stays off too.
        if let Some(need) = needed::<G>(manifest.id, index, component).find(|need| !names.contains(need)) {
            log!("{}: off for this build (needs {}, which is off)", component.name, need);
            for feature in component.features {
                available.lose(*feature, format!("needs {}", need));
            }
            continue;
        }
        let mut ledger = Ledger::new();
        let mut start = Start::<G> { session, manifest, options: &options, shared: &shared, build, hooks: &mut hooks, ledger: &mut ledger, channel: channel.as_ref(), installed: &names };
        match step(component.name, "install", || (component.install)(&mut start)) {
            Ok(()) => installed.push((index, component, ledger)),
            Err(Lack::Missing(why)) => {
                log!("{}: off for this build ({why})", component.name);
                ledger.restore();
                for feature in component.features {
                    available.lose(*feature, why.clone());
                }
            }
            Err(Lack::Fatal(rejection)) => {
                ledger.restore();
                undo(&mut installed);
                crate::debug::close();
                crate::remove_crash_log();
                return Err(rejection);
            }
        }
    }
    let slot = Mutex::new(None);
    if let Err(rejection) = enable_hooks(&slot, hooks) {
        undo(&mut installed);
        crate::debug::close();
        crate::remove_crash_log();
        return Err(rejection);
    }
    let armed = installed.iter().fold(0, |bits, (i, _, _)| bits | (1 << i));
    ON.store(armed, Release);
    for (_, component, _) in &installed {
        step(component.name, "arm", component.arm);
    }
    for line in available.report() {
        log!("features: {line}");
    }
    let timer = Timer::start::<G>(armed);
    // The probe console, with the debug menu's other tools.
    if let Some(channel) = &channel
        && session.has(monaka_contract::manifest::DEBUG_MENU_MARKER)
    {
        crate::console::open(channel);
    }
    let ledgers = installed.into_iter().map(|(place, _, ledger)| (place, ledger)).collect();
    *RUN.locked() = Some(Running { hooks: slot, ledgers, available, armed, timer });
    Ok(())
}

/// A start that failed after components installed: each, newest first, gives the game back what
/// its ledger changed and releases what it made.
fn undo<G: Game>(installed: &mut [(usize, &'static Component<G>, Ledger)]) {
    for (_, component, ledger) in installed.iter_mut().rev() {
        ledger.restore();
        step(component.name, "release (start undone)", component.release);
    }
}

/// Stops the run (`producer!`'s `MonakaStop`).
pub fn stop<G: Game>() -> Result<(), Rejection> {
    let Some(mut run) = RUN.locked().take() else { return Ok(()) };
    let on = armed::<G>(run.armed);
    // Timed work ends before anything stops, and the console's probes come out.
    if let Some(timer) = run.timer.take() {
        timer.stop();
    }
    crate::console::close();
    // Each component off in turn, newest first: from here its detours stop acting and its frame
    // work stops; then whatever else it does to stop new work.
    for (place, component) in on.iter().rev() {
        ON.fetch_and(!(1 << place), AcqRel);
        step(component.name, "quiet", component.quiet);
    }
    // Each gives back, newest first: the settings it changed, then its own state (waiting on a
    // render thread to free its objects, a window size switched back).
    for (place, component) in on.iter().rev() {
        if let Some((_, ledger)) = run.ledgers.iter_mut().find(|(p, _)| p == place) {
            let gone = ledger.restore();
            if gone > 0 {
                log!("{}: {gone} changes not put back: what they changed is gone", component.name);
            }
        }
        step(component.name, "give back", component.give_back);
        // One that has not let go refuses: the run stays as it is, hooks in, for another try.
        if let Some(let_go) = component.let_go
            && let Err(refused) = let_go()
        {
            log!("{}: has not let go: {}", component.name, refused.reason);
            *RUN.locked() = Some(run);
            return Err(refused);
        }
    }
    for (_, component) in &on {
        if let Some(summary) = component.summary {
            let lines = summary();
            if !lines.is_empty() {
                log!("{lines}");
            }
        }
    }
    log!("{}", crate::situation::summary());
    let report = run.available.report();
    for line in &report {
        log!("features: {line}");
    }
    if let Some(session) = Session::get() {
        let _ = std::fs::write(session.path("features.txt"), report.join("\n"));
    }
    // Hooks that would not come out may still run a detour: nothing they use is freed then.
    remove_hooks(&run.hooks, 2000)?;
    for (_, component) in on.iter().rev() {
        step(component.name, "release", component.release);
    }
    crate::debug::close();
    crate::remove_crash_log();
    Ok(())
}

#[cfg(test)]
mod tests;
