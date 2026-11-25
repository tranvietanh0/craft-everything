//! One rebuild at a time, first come first served. Pure: the caller starts the job that [`RebuildQueue::submit`]
//! or [`RebuildQueue::finish`] hands back, and calls `finish` however the child ends (ok, failed or crashed).
//! The queue keeps a copy of the running job, so the whole line can be saved ([`crate::saved_queue`])
//! and searched (`/status`, `/stop`).

use std::collections::VecDeque;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admission<T> {
    /// Nothing was running: start this job now.
    Start(T),
    /// Something is running: the job waits; `position` is 1 for the next in line.
    Queued { position: usize },
}

#[derive(Debug, Clone)]
pub struct RebuildQueue<T> {
    running: Option<T>,
    waiting: VecDeque<T>,
}

impl<T> Default for RebuildQueue<T> {
    fn default() -> Self {
        RebuildQueue { running: None, waiting: VecDeque::new() }
    }
}

impl<T: Clone> RebuildQueue<T> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_running(&self) -> bool {
        self.running.is_some()
    }

    pub fn running(&self) -> Option<&T> {
        self.running.as_ref()
    }

    pub fn waiting(&self) -> usize {
        self.waiting.len()
    }

    /// The waiting jobs, next in line first.
    pub fn waiting_jobs(&self) -> impl Iterator<Item = &T> {
        self.waiting.iter()
    }

    /// The place in line (1 for the next) of the first waiting job `is_it` picks.
    pub fn position(&self, is_it: impl Fn(&T) -> bool) -> Option<usize> {
        self.waiting.iter().position(is_it).map(|i| i + 1)
    }

    /// Takes the first waiting job `is_it` picks out of the line.
    pub fn remove_waiting(&mut self, is_it: impl Fn(&T) -> bool) -> Option<T> {
        let index = self.waiting.iter().position(is_it)?;
        self.waiting.remove(index)
    }

    /// A new request starts now when nothing runs; otherwise it joins the back of the line.
    pub fn submit(&mut self, job: T) -> Admission<T> {
        if self.running.is_some() {
            self.waiting.push_back(job);
            Admission::Queued { position: self.waiting.len() }
        } else {
            self.running = Some(job.clone());
            Admission::Start(job)
        }
    }

    /// The running job ended. Returns the next waiting job, which is now running; `None` leaves the queue idle.
    pub fn finish(&mut self) -> Option<T> {
        self.running = self.waiting.pop_front();
        self.running.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_request_starts_at_once() {
        let mut q = RebuildQueue::new();
        assert_eq!(q.submit("a"), Admission::Start("a"));
        assert!(q.is_running());
    }

    #[test]
    fn later_requests_wait_in_arrival_order() {
        let mut q = RebuildQueue::new();
        q.submit("a");
        assert_eq!(q.submit("b"), Admission::Queued { position: 1 });
        assert_eq!(q.submit("c"), Admission::Queued { position: 2 });
        assert_eq!(q.waiting(), 2);
    }

    #[test]
    fn finishing_starts_the_next_job_in_order() {
        let mut q = RebuildQueue::new();
        q.submit("a");
        q.submit("b");
        q.submit("c");
        assert_eq!(q.finish(), Some("b"));
        assert!(q.is_running());
        assert_eq!(q.finish(), Some("c"));
        assert_eq!(q.finish(), None);
        assert!(!q.is_running());
    }

    #[test]
    fn a_waiting_job_can_be_found_and_taken_out_of_line() {
        let mut q = RebuildQueue::new();
        q.submit("a");
        q.submit("b");
        q.submit("c");
        assert_eq!(q.running(), Some(&"a"));
        assert_eq!(q.position(|j| *j == "c"), Some(2));
        assert_eq!(q.remove_waiting(|j| *j == "b"), Some("b"));
        assert_eq!(q.position(|j| *j == "c"), Some(1), "the line closes up");
        assert_eq!(q.remove_waiting(|j| *j == "a"), None, "the running job is not waiting");
    }

    #[test]
    fn an_idle_queue_admits_the_next_request_immediately() {
        let mut q: RebuildQueue<&str> = RebuildQueue::new();
        assert_eq!(q.finish(), None);
        assert_eq!(q.submit("x"), Admission::Start("x"));
    }
}
