//! The probe console beside a channel ([`monaka_contract::console`]). A tool asks and reads answers;
//! the producer takes questions and answers them. Either side may create it. Every access waits
//! at most a couple of milliseconds; a miss is not an error.

use crate::shared::SharedBlock;
use crate::{ChannelName, Result};
use monaka_contract::console::ConsoleBlock;
use monaka_contract::protocol::Block;

const WAIT_MS: u32 = 2;

pub struct ConsoleChannel {
    block: SharedBlock<ConsoleBlock>,
}

impl ConsoleChannel {
    /// The producer's console.
    pub fn open(channel: &ChannelName) -> Result<Self> {
        let (name, lock) = channel.block(Block::Console);
        Self::open_named(&name, &lock)
    }

    /// The viewer's own (the 2D window's command line): the same questions and answers, the viewer
    /// answering.
    pub fn open_view(channel: &ChannelName) -> Result<Self> {
        let (name, lock) = channel.block(Block::ViewConsole);
        Self::open_named(&name, &lock)
    }

    fn open_named(name: &str, lock: &str) -> Result<Self> {
        Ok(Self { block: SharedBlock::<ConsoleBlock>::open_versioned(name, lock, WAIT_MS * 50)? })
    }

    /// Producer: the question waiting, with its number.
    pub fn pending(&mut self) -> Option<(u64, String)> {
        self.block.lock(WAIT_MS).tolerant()?.pending()
    }

    /// Producer: answers question `number`; whether it was written.
    pub fn answer(&mut self, number: u64, text: &str) -> bool {
        let Some(mut record) = self.block.lock(WAIT_MS * 5).tolerant() else { return false };
        record.answer(number, text);
        true
    }

    /// Producer: the status as it is now.
    pub fn set_status(&mut self, text: &str) -> bool {
        let Some(mut record) = self.block.lock(WAIT_MS).tolerant() else { return false };
        record.set_status(text);
        true
    }

    /// Producer: a question and its answer onto the log.
    pub fn post(&mut self, command: &str, answer: &str) -> bool {
        let Some(mut record) = self.block.lock(WAIT_MS * 5).tolerant() else { return false };
        record.post(command, answer);
        true
    }

    /// Watcher: the status and the log, with the count of their writes (unchanged: nothing new).
    pub fn view(&mut self) -> Option<(u64, String, String)> {
        let record = self.block.lock(WAIT_MS).tolerant()?;
        Some((record.generation, record.status_text(), record.log_text()))
    }

    /// Tool: asks `command`; its number, once written.
    pub fn ask(&mut self, command: &str) -> Option<u64> {
        Some(self.block.lock(WAIT_MS * 5).tolerant()?.ask(command))
    }

    /// Tool: the answer to question `number`, once given.
    pub fn answer_to(&mut self, number: u64) -> Option<String> {
        self.block.lock(WAIT_MS).tolerant()?.answer_to(number)
    }
}
