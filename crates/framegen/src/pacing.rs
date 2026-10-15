//! Pacing for doubled pair rates: a present publishes the generated pair (halfway between the last
//! real pair and this one) at once and leaves this real pair waiting half a frame, so the headset
//! gets twice the rendered rate, evenly spaced, for about one frame of latency. The real pair goes
//! out from a thread of its own at its time ([`Pacer`]), or before anything the next present
//! publishes, so pairs never go out of order. When it is due: [`Interval`].

use monaka_core::sync::Locked;
use monaka_producer::log_first;
use std::sync::atomic::{AtomicBool, Ordering::*};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Pairs published from a thread of their own at their time, in order with those published at the
/// presents: a pair still waiting when the next present publishes goes out first.
pub struct Pacer<T: Send + 'static> {
    shared: Arc<Paced<T>>,
    thread: Option<JoinHandle<()>>,
}

struct Paced<T> {
    due: Mutex<Option<(Instant, T)>>,
    wake: Condvar,
    stop: AtomicBool,
    publish: fn(T),
}

impl<T: Send + 'static> Pacer<T> {
    /// The thread `name`, publishing with `publish`.
    pub fn start(name: &str, publish: fn(T)) -> std::io::Result<Self> {
        let shared = Arc::new(Paced { due: Mutex::new(None), wake: Condvar::new(), stop: AtomicBool::new(false), publish });
        let thread = {
            let shared = shared.clone();
            std::thread::Builder::new().name(name.into()).spawn(move || shared.run())?
        };
        Ok(Self { shared, thread: Some(thread) })
    }

    /// A pair still waiting published at once, then `now` run, then `next` left waiting until its
    /// time: all under one lock, so nothing the thread publishes comes between.
    pub fn now_then<R>(&self, now: impl FnOnce() -> R, next: Option<(Instant, T)>) -> R {
        let mut due = self.shared.due.locked();
        if let Some((_, waiting)) = due.take() {
            (self.shared.publish)(waiting);
        }
        let result = now();
        if next.is_some() {
            *due = next;
            self.shared.wake.notify_one();
        }
        result
    }
}

impl<T> Paced<T> {
    fn run(&self) {
        let mut due = self.due.locked();
        while !self.stop.load(Acquire) {
            let now = Instant::now();
            due = match due.as_ref().map(|(at, _)| *at) {
                None => self.wake.wait(due).unwrap_or_else(|e| e.into_inner()),
                Some(at) if at > now => self.wake.wait_timeout(due, at - now).unwrap_or_else(|e| e.into_inner()).0,
                Some(_) => {
                    if let Some((_, waiting)) = due.take() {
                        (self.publish)(waiting);
                    }
                    due
                }
            };
        }
    }
}

/// Stops the thread; a pair still waiting is dropped.
impl<T: Send + 'static> Drop for Pacer<T> {
    fn drop(&mut self) {
        self.shared.stop.store(true, Release);
        {
            let _due = self.shared.due.locked();
            self.shared.wake.notify_all();
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// A producer's pacer, its thread started with the first pair that waits.
pub struct Pacing<T: Send + 'static> {
    name: &'static str,
    publish: fn(T),
    pacer: Mutex<Option<Pacer<T>>>,
}

impl<T: Send + 'static> Pacing<T> {
    /// Pairs paced by a thread called `name`, published with `publish`.
    pub const fn new(name: &'static str, publish: fn(T)) -> Self {
        Self { name, publish, pacer: Mutex::new(None) }
    }

    /// Runs `now` at once, after any pair still waiting went out (so pairs stay in order), and
    /// leaves `next` waiting for its time. Without a thread (it could not start: logged) `next` goes
    /// out at once, after `now`.
    pub fn now_then<R>(&self, now: impl FnOnce() -> R, next: Option<(Instant, T)>) -> R {
        let mut pacer = self.pacer.locked();
        if pacer.is_none() && next.is_some() {
            match Pacer::start(self.name, self.publish) {
                Ok(started) => *pacer = Some(started),
                Err(e) => log_first(1, format_args!("frame pacing thread: {e}")),
            }
        }
        match pacer.as_ref() {
            Some(pacer) => pacer.now_then(now, next),
            None => {
                let result = now();
                if let Some((_, waiting)) = next {
                    (self.publish)(waiting);
                }
                result
            }
        }
    }

    /// Stops the thread (a pair still waiting is dropped); the next waiting pair starts it again.
    pub fn stop(&self) {
        drop(self.pacer.locked().take());
    }
}

/// The real pairs' interval, smoothed, and when a real pair is due after its generated one.
#[derive(Clone, Copy, Debug)]
pub struct Interval {
    interval: Duration,
    last: Option<Instant>,
}

/// A gap longer than this (a hitch, a pause) counts as this.
const LONGEST_GAP: Duration = Duration::from_millis(100);
/// Half the interval is kept within these.
const SOONEST: Duration = Duration::from_millis(2);
const LATEST: Duration = Duration::from_millis(40);

impl Interval {
    /// Starting from `interval` until pairs come.
    pub const fn new(interval: Duration) -> Self {
        Self { interval, last: None }
    }

    /// A real pair at `now`: the interval moves a fifth of the way to its gap from the last one, and
    /// the pair is due half an interval from `now` (2 to 40 ms).
    pub fn due(&mut self, now: Instant) -> Instant {
        if let Some(last) = self.last {
            let gap = (now - last).min(LONGEST_GAP);
            self.interval = self.interval.mul_f32(0.8) + gap.mul_f32(0.2);
        }
        self.last = Some(now);
        now + (self.interval / 2).clamp(SOONEST, LATEST)
    }

    /// The smoothed interval.
    pub fn interval(&self) -> Duration {
        self.interval
    }
}

#[cfg(test)]
mod tests;
