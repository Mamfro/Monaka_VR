//! A game's research probes' options: read once at the start from the same options file as the
//! adapter's (never shown by the launcher), logged, then asked by each probe, which returns at once
//! when its own option is off. A game's `research` module keeps them in one static:
//!
//! `pub static PROBES: Research<Options> = Research::new(Options::from_options);`

use crate::log;
use monaka_core::options::Options;
use std::fmt::Debug;
use std::sync::OnceLock;

pub struct Research<O: 'static> {
    options: OnceLock<O>,
    read: fn(&Options) -> O,
}

impl<O: Debug + Default + Sync + Send> Research<O> {
    pub const fn new(read: fn(&Options) -> O) -> Self {
        Self { options: OnceLock::new(), read }
    }

    /// Reads this run's probe options (once, in the game's `prepare`).
    pub fn configure(&self, options: &Options) {
        let read = (self.read)(options);
        log!("research: {read:?}");
        let _ = self.options.set(read);
    }

    /// This run's probes (none before [`Research::configure`]).
    pub fn get(&self) -> &O {
        self.options.get_or_init(O::default)
    }
}
