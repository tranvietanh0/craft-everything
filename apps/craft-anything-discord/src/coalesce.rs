//! Rate limits for Discord writes. [`Coalescer`] decides when a pending update may go out (the board
//! and the Log thread are edited at most every [`EDIT_INTERVAL`]); [`pack_lines`] joins log lines into
//! messages of at most [`MESSAGE_LIMIT`] characters. Both are pure: the caller supplies the clock.

use std::time::{Duration, Instant};

/// Shortest gap between two edits of the same board or log message.
pub const EDIT_INTERVAL: Duration = Duration::from_secs(3);
/// Discord's message length limit, in characters.
pub const MESSAGE_LIMIT: usize = 2_000;

/// Many changes in one gap become one send: `mark` as often as you like, `sent` once you have sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Coalescer {
    min_gap: Duration,
    last_sent: Option<Instant>,
    pending: bool,
}

impl Coalescer {
    pub fn new(min_gap: Duration) -> Self {
        Coalescer { min_gap, last_sent: None, pending: false }
    }

    pub fn mark(&mut self) {
        self.pending = true;
    }

    pub fn is_pending(&self) -> bool {
        self.pending
    }

    /// How long until a send is allowed, or `None` when nothing is pending.
    pub fn wait(&self, now: Instant) -> Option<Duration> {
        if !self.pending {
            return None;
        }
        Some(match self.last_sent {
            None => Duration::ZERO,
            Some(last) => self.min_gap.saturating_sub(now.saturating_duration_since(last)),
        })
    }

    /// True when a pending update may go out now.
    pub fn is_due(&self, now: Instant) -> bool {
        self.wait(now).is_some_and(|d| d.is_zero())
    }

    /// Records that the pending update was sent at `now`.
    pub fn sent(&mut self, now: Instant) {
        self.last_sent = Some(now);
        self.pending = false;
    }
}

/// Joins `lines` with newlines into messages of at most `limit` characters, keeping order. A line
/// longer than `limit` is split at character boundaries. An empty input gives no messages.
pub fn pack_lines(lines: &[String], limit: usize) -> Vec<String> {
    let limit = limit.max(1);
    let mut messages = Vec::new();
    let mut current = String::new();
    // Characters in `current`, newlines included; `started` says whether it holds any line yet.
    let mut current_len = 0_usize;
    let mut started = false;
    for line in lines {
        let chars: Vec<char> = line.chars().collect();
        let pieces: Vec<String> = if chars.is_empty() { vec![String::new()] } else { chars.chunks(limit).map(|c| c.iter().collect()).collect() };
        for piece in pieces {
            let piece_len = piece.chars().count();
            if started && current_len + 1 + piece_len > limit {
                messages.push(std::mem::take(&mut current));
                current_len = 0;
                started = false;
            }
            if started {
                current.push('\n');
                current_len += 1;
            }
            current.push_str(&piece);
            current_len += piece_len;
            started = true;
        }
    }
    if started {
        messages.push(current);
    }
    messages
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pending_update_waits_for_the_gap() {
        let start = Instant::now();
        let mut c = Coalescer::new(EDIT_INTERVAL);
        c.mark();
        assert!(c.is_due(start), "the first update is never held back");
        c.sent(start);
        c.mark();
        assert!(!c.is_due(start + Duration::from_secs(1)));
        assert!(c.is_due(start + EDIT_INTERVAL));
    }

    #[test]
    fn many_marks_in_one_gap_are_one_send() {
        let start = Instant::now();
        let mut c = Coalescer::new(EDIT_INTERVAL);
        c.mark();
        c.mark();
        c.mark();
        c.sent(start);
        assert!(!c.is_pending());
        assert_eq!(c.wait(start), None);
    }

    #[test]
    fn wait_shrinks_as_time_passes() {
        let start = Instant::now();
        let mut c = Coalescer::new(EDIT_INTERVAL);
        c.mark();
        c.sent(start);
        c.mark();
        let early = c.wait(start + Duration::from_secs(1)).unwrap_or_default();
        let late = c.wait(start + Duration::from_secs(2)).unwrap_or_default();
        assert!(late < early);
    }

    #[test]
    fn lines_pack_in_order_under_the_limit() {
        let lines: Vec<String> = ["aaaa", "bbbb", "cccc"].iter().map(|s| (*s).to_owned()).collect();
        let packed = pack_lines(&lines, 9);
        assert_eq!(packed, ["aaaa\nbbbb", "cccc"]);
        assert!(packed.iter().all(|m| m.chars().count() <= 9));
    }

    #[test]
    fn a_line_longer_than_the_limit_is_split() {
        let lines = vec!["x".repeat(25)];
        let packed = pack_lines(&lines, 10);
        assert_eq!(packed.iter().map(|m| m.chars().count()).collect::<Vec<_>>(), [10, 10, 5]);
    }

    #[test]
    fn a_blank_line_keeps_its_place() {
        let lines: Vec<String> = ["a", "", "b"].iter().map(|s| (*s).to_owned()).collect();
        assert_eq!(pack_lines(&lines, 10), ["a\n\nb"]);
    }

    #[test]
    fn no_lines_gives_no_messages() {
        assert!(pack_lines(&[], MESSAGE_LIMIT).is_empty());
    }
}
