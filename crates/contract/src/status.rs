//! What a producer's entry points answer the loader (part of the pack contract,
//! [`crate::pack::CONTRACT`]): `MonakaStart` and `MonakaStop` return a Win32 [`code`],
//! `MonakaStatus` a [`State`]. The producer's lifecycle writes them and the loader reads them, so
//! both take them from here.

/// Win32 codes a producer's `MonakaStart` and `MonakaStop` return; [`code::SUCCESS`] is success.
pub mod code {
    pub const SUCCESS: u32 = 0;
    pub const NOT_READY: u32 = 21;
    pub const NOT_SUPPORTED: u32 = 50;
    pub const OPEN_FAILED: u32 = 110;
    pub const ALREADY_EXISTS: u32 = 183;
    pub const WAIT_TIMEOUT: u32 = 258;
    pub const REVISION_MISMATCH: u32 = 1306;
}

/// What starts the producer's log line giving why `MonakaStart` refused; the loader shows the
/// player the reason after it.
pub const REJECTED_LINE: &str = "rejected: ";

/// `MonakaStatus` values; the loader stops an older copy that reports [`State::Running`] before
/// another starts.
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

impl State {
    pub const ALL: [State; 7] = [State::New, State::Starting, State::Running, State::Stopped, State::Rejected, State::ShutdownFailed, State::Stopping];

    /// The state a `MonakaStatus` value names, if it names one.
    pub fn from_u32(value: u32) -> Option<State> {
        Self::ALL.into_iter().find(|state| *state as u32 == value)
    }
}

#[cfg(test)]
mod tests;
