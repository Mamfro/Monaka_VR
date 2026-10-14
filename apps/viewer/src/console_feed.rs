//! The game's probe console as the Console panel sees it (`monaka_channel::console`): its status
//! and log read a few times a second, and the commands its buttons ask sent to it.

use monaka_channel::ChannelName;
use monaka_channel::console::ConsoleChannel;
use monaka_overlay::console_view::ConsoleView;
use std::time::{Duration, Instant};

/// How often the console is read, and how long its producer may go without writing before the
/// panel says there is none (it writes its status twice a second).
const READ_EVERY: Duration = Duration::from_millis(200);
const SILENT: Duration = Duration::from_secs(3);

pub struct ConsoleFeed {
    name: ChannelName,
    channel: Option<ConsoleChannel>,
    view: ConsoleView,
    generation: u64,
    /// When the console was last read, and when its producer last wrote.
    read_at: Option<Instant>,
    written_at: Option<Instant>,
}

impl ConsoleFeed {
    pub fn new(name: &ChannelName) -> Self {
        Self { name: name.clone(), channel: None, view: ConsoleView::default(), generation: 0, read_at: None, written_at: None }
    }

    /// The view as of now (read again when due).
    pub fn view(&mut self, now: Instant) -> &ConsoleView {
        if self.read_at.is_none_or(|at| now.duration_since(at) >= READ_EVERY) {
            self.read_at = Some(now);
            if self.channel.is_none() {
                self.channel = ConsoleChannel::open(&self.name).ok();
            }
            if let Some((generation, status, log)) = self.channel.as_mut().and_then(ConsoleChannel::view)
                && generation != self.generation
            {
                self.generation = generation;
                self.written_at = Some(now);
                self.view.status = status;
                self.view.log = log;
            }
            self.view.open = self.written_at.is_some_and(|at| now.duration_since(at) < SILENT);
        }
        &self.view
    }

    /// Asks `command` (a button's); the answer shows in the log.
    pub fn ask(&mut self, command: &str) {
        if let Some(channel) = self.channel.as_mut() {
            channel.ask(command);
        }
    }
}
