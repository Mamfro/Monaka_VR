//! The probe console beside a channel: text commands from a tool (the 2D window, `monaka_play
//! console`) to the producer, which answers each in text. One command at a time: a tool asks
//! ([`ConsoleBlock::ask`]), the producer takes it ([`ConsoleBlock::pending`]) and answers it
//! ([`ConsoleBlock::answer`]), the tool reads the answer to its own question
//! ([`ConsoleBlock::answer_to`]). A newer question replaces one not yet taken.
//!
//! Beside the questions, for anyone watching (the console panel): the producer's status (its
//! probes, its breakpoints: [`ConsoleBlock::set_status`]) and a log of the questions asked and
//! the start of each answer, whoever asked ([`ConsoleBlock::post`]).

use crate::protocol::{Stamp, Versioned};

#[cfg(test)]
mod tests;

pub const CONSOLE_MAGIC: u32 = u32::from_le_bytes(*b"MKCO");
pub const CONSOLE_MAJOR: u16 = 3;
/// The longest command and answer (bytes); a longer answer is cut, and says so.
pub const COMMAND_MAX: usize = 1024;
pub const ANSWER_MAX: usize = 60 * 1024;
/// The status and the log kept (bytes), and the lines of an answer the log keeps.
pub const STATUS_MAX: usize = 4 * 1024;
pub const LOG_MAX: usize = 16 * 1024;
const LOGGED_LINES: usize = 24;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ConsoleBlock {
    pub stamp: Stamp,
    /// The latest question's number, and the number of the one answered.
    pub asked: u64,
    pub answered: u64,
    pub command_len: u32,
    pub answer_len: u32,
    pub command: [u8; COMMAND_MAX],
    pub answer: [u8; ANSWER_MAX],
    /// Counts the status's and the log's writes.
    pub generation: u64,
    pub status_len: u32,
    pub log_len: u32,
    pub status: [u8; STATUS_MAX],
    pub log: [u8; LOG_MAX],
}

/// What a cut answer ends with.
const CUT: &str = "\n(cut: the answer is longer than the console holds)";

impl ConsoleBlock {
    /// Tool: asks `command` (cut to fit); its number, for [`ConsoleBlock::answer_to`].
    pub fn ask(&mut self, command: &str) -> u64 {
        let bytes = cut(command, COMMAND_MAX);
        self.command[..bytes.len()].copy_from_slice(bytes);
        self.command_len = bytes.len() as u32;
        self.asked += 1;
        self.asked
    }

    /// Producer: the question not yet answered, with its number.
    pub fn pending(&self) -> Option<(u64, String)> {
        (self.asked > self.answered).then(|| (self.asked, String::from_utf8_lossy(&self.command[..(self.command_len as usize).min(COMMAND_MAX)]).into_owned()))
    }

    /// Producer: answers question `number` with `text` (cut to fit, saying so).
    pub fn answer(&mut self, number: u64, text: &str) {
        let bytes = if text.len() <= ANSWER_MAX { text.as_bytes() } else { cut(text, ANSWER_MAX - CUT.len()) };
        self.answer[..bytes.len()].copy_from_slice(bytes);
        let mut length = bytes.len();
        if bytes.len() < text.len() {
            self.answer[length..length + CUT.len()].copy_from_slice(CUT.as_bytes());
            length += CUT.len();
        }
        self.answer_len = length as u32;
        self.answered = number;
    }

    /// Tool: the answer to question `number`, once given.
    pub fn answer_to(&self, number: u64) -> Option<String> {
        (self.answered == number).then(|| String::from_utf8_lossy(&self.answer[..(self.answer_len as usize).min(ANSWER_MAX)]).into_owned())
    }
}

impl Versioned for ConsoleBlock {
    const MAGIC: u32 = CONSOLE_MAGIC;
    const MAJOR: u16 = CONSOLE_MAJOR;
    const MINOR: u16 = 0;
    const WHAT: &'static str = "console block";
    const CAPACITY: usize = 192 * 1024;

    fn stamp(&self) -> Stamp {
        self.stamp
    }

    fn start(&mut self) {
        self.stamp = Stamp::of::<Self>();
    }
}

impl ConsoleBlock {
    /// Producer: the status as it is now.
    pub fn set_status(&mut self, text: &str) {
        let bytes = cut(text, STATUS_MAX);
        self.status[..bytes.len()].copy_from_slice(bytes);
        self.status_len = bytes.len() as u32;
        self.generation += 1;
    }

    /// Producer: a question and its answer (its first lines) onto the log, the oldest lines
    /// dropped to make room.
    pub fn post(&mut self, command: &str, answer: &str) {
        let mut entry = format!("> {command}\n");
        let lines: Vec<&str> = answer.lines().collect();
        for line in lines.iter().take(LOGGED_LINES) {
            entry.push_str(line);
            entry.push('\n');
        }
        if lines.len() > LOGGED_LINES {
            entry.push_str(&format!("  ({} more lines)\n", lines.len() - LOGGED_LINES));
        }
        let mut log = self.log_text();
        log.push_str(&entry);
        // Whole lines dropped from the front until it fits.
        while log.len() > LOG_MAX {
            match log.find('\n') {
                Some(end) if end + 1 < log.len() => drop(log.drain(..=end)),
                _ => log = String::from_utf8_lossy(cut(&log, LOG_MAX)).into_owned(),
            }
        }
        self.log[..log.len()].copy_from_slice(log.as_bytes());
        self.log_len = log.len() as u32;
        self.generation += 1;
    }

    pub fn status_text(&self) -> String {
        String::from_utf8_lossy(&self.status[..(self.status_len as usize).min(STATUS_MAX)]).into_owned()
    }

    pub fn log_text(&self) -> String {
        String::from_utf8_lossy(&self.log[..(self.log_len as usize).min(LOG_MAX)]).into_owned()
    }
}

/// The longest start of `text` that fits in `most` bytes, cut at a character.
fn cut(text: &str, most: usize) -> &[u8] {
    let mut end = text.len().min(most);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text.as_bytes()[..end]
}
