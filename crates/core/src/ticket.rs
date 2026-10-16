//! Which head pose and which eye an image was built from, carried from where the pose is sampled
//! to where the image is shown. A game builds a frame's views on one thread (the pose chosen, the
//! eye cameras written) and draws and presents it on another, a frame or two later; everything that
//! acts on the image (the publisher's pose record, a per-eye upscaler's history, the HUD laid over
//! it) must know what it was built from, not what is current when it gets there.
//!
//! A [`Ticket`] is issued where the pose is sampled and travels in order: the render thread looks
//! at the oldest one not shown yet ([`Tickets::drawing`]) and the present takes it
//! ([`Tickets::show`]). How many were waiting at each show says whether the pairing holds (one or
//! two, by the game's pipelining); more, and frames are being built that are never shown (or the
//! present is not the one hooked), so the oldest are dropped ([`Tickets::lost`]).
//!
//! A producer whose eyes alternate by present number (`monaka_stereo::driver`, [`crate::alternate`])
//! can key tickets by that number instead and look them up ([`Tickets::find`]).

use monaka_contract::protocol::HeadPose;
use std::collections::VecDeque;

/// What a frame draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    /// One eye (0 left, 1 right): alternate eyes.
    Eye(usize),
    /// Both eyes in the one frame (side by side, or a second scene pass in the same frame).
    Both,
    /// No stereo: a flat frame (a video, a menu), shown without a pose.
    Flat,
}

/// One frame's: its number (issue order, or the present number it is keyed by), what it draws, the
/// head pose it was built from, and whatever else the producer must carry with it (`X`: the baked
/// head yaw, a jitter, the game's frame counter).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ticket<X: Copy = ()> {
    pub number: u64,
    pub view: View,
    pub pose: HeadPose,
    pub extra: X,
}

/// A show: the ticket of the frame being shown and how many were waiting (itself included).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shown<X: Copy = ()> {
    pub ticket: Option<Ticket<X>>,
    pub waiting: usize,
}

/// The tickets issued and not shown yet, oldest first.
#[derive(Clone, Debug)]
pub struct Tickets<X: Copy = ()> {
    queue: VecDeque<Ticket<X>>,
    /// The most kept: more than this between issuing and showing means the pairing is lost.
    depth: usize,
    next: u64,
    shown: u64,
    /// Shows with no ticket waiting.
    missing: u64,
    /// Tickets dropped unshown.
    lost: u64,
    /// The most that were waiting at a show.
    deepest: usize,
    /// The ticket the last show took off the queue, and that show's count (for
    /// [`Tickets::show_behind_drawing`]; none after a show that took the last one again).
    last: Option<(Ticket<X>, u64)>,
    /// Shows that found the queue a frame ahead and took the last ticket again.
    realigned: u64,
}

impl<X: Copy> Tickets<X> {
    /// Keeps at most `depth` unshown tickets (a game pipelines one to three frames: 4 is ample).
    pub const fn new(depth: usize) -> Self {
        Self { queue: VecDeque::new(), depth, next: 0, shown: 0, missing: 0, lost: 0, deepest: 0, last: None, realigned: 0 }
    }

    /// A frame's views were built for `view` from `pose`: its ticket, numbered in issue order.
    pub fn issue(&mut self, view: View, pose: HeadPose, extra: X) -> Ticket<X> {
        let number = self.next;
        self.issue_numbered(number, view, pose, extra)
    }

    /// As [`Tickets::issue`], keyed by the producer's own number (a present number).
    pub fn issue_numbered(&mut self, number: u64, view: View, pose: HeadPose, extra: X) -> Ticket<X> {
        let ticket = Ticket { number, view, pose, extra };
        if self.queue.len() >= self.depth.max(1) {
            self.queue.pop_front();
            self.lost += 1;
        }
        self.queue.push_back(ticket);
        self.next = number + 1;
        ticket
    }

    /// The frame being drawn now: the oldest not shown.
    pub fn drawing(&self) -> Option<&Ticket<X>> {
        self.queue.front()
    }

    /// The newest issued (the frame being built).
    pub fn building(&self) -> Option<&Ticket<X>> {
        self.queue.back()
    }

    /// The unshown ticket numbered `number`.
    pub fn find(&self, number: u64) -> Option<&Ticket<X>> {
        self.queue.iter().find(|t| t.number == number)
    }

    /// The frame drawing now is shown: its ticket leaves.
    pub fn show(&mut self) -> Shown<X> {
        let waiting = self.queue.len();
        let ticket = self.queue.pop_front();
        self.shown += 1;
        self.deepest = self.deepest.max(waiting);
        if ticket.is_none() {
            self.missing += 1;
        }
        Shown { ticket, waiting }
    }

    /// A frame is shown by a present that comes once the next frame is being drawn (an engine that
    /// presents at the start of drawing the next frame): that frame's ticket is issued already, so
    /// at least two wait whenever the pairing holds, and the oldest leaves. With only one waiting,
    /// the frame shown is the one the last show gave (its ticket was taken a frame early: a present
    /// before any frame was drawn, say), and the waiting one stays: the frame shown gets the last
    /// ticket again, and from the next show on the pairing holds. Only once in a row: with nothing
    /// new built (a menu), the next show takes the waiting ticket.
    pub fn show_behind_drawing(&mut self) -> Shown<X> {
        let waiting = self.queue.len();
        if waiting == 1
            && let Some((ticket, at)) = self.last
            && at == self.shown
        {
            self.shown += 1;
            self.realigned += 1;
            self.deepest = self.deepest.max(waiting);
            self.last = None;
            return Shown { ticket: Some(ticket), waiting };
        }
        let shown = self.show();
        self.last = shown.ticket.map(|ticket| (ticket, self.shown));
        shown
    }

    /// The frames up to and including `number` are shown (a present that completes a numbered
    /// frame): their tickets leave, and the one numbered `number` comes back.
    pub fn show_through(&mut self, number: u64) -> Shown<X> {
        let waiting = self.queue.len();
        let mut found = None;
        while self.queue.front().is_some_and(|t| t.number <= number) {
            let ticket = self.queue.pop_front();
            if ticket.is_some_and(|t| t.number == number) {
                found = ticket;
            } else {
                self.lost += 1;
            }
        }
        self.shown += 1;
        self.deepest = self.deepest.max(waiting);
        if found.is_none() {
            self.missing += 1;
        }
        Shown { ticket: found, waiting }
    }

    /// Drops every unshown ticket (the game stopped presenting, the producer stops).
    pub fn clear(&mut self) {
        self.queue.clear();
    }

    pub fn lost(&self) -> u64 {
        self.lost
    }

    pub fn missing(&self) -> u64 {
        self.missing
    }

    /// `"1200 shown (at most 2 waiting), 0 missing, 0 lost, 0 realigned"`.
    pub fn summary(&self) -> String {
        format!("{} shown (at most {} waiting), {} missing, {} lost, {} realigned", self.shown, self.deepest, self.missing, self.lost, self.realigned)
    }
}

#[cfg(test)]
mod tests;
