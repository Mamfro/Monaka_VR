//! Helpers for the session log's lines (the line itself is [`crate::log()`] and `log!`).

/// Whether the `n`th time something happens (counted from 1) is worth a log line: the first
/// `first` times, then at each power of two, so a site that keeps firing stays a handful of lines.
pub fn seldom(n: u64, first: u64) -> bool {
    n <= first || n.is_power_of_two()
}

#[cfg(test)]
mod tests;
