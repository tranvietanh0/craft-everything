//! One rebuild at a time, first come first served. Pure: the caller starts the job that [`RebuildQueue::submit`]
//! or [`RebuildQueue::finish`] hands back, and calls `finish` however the child ends (ok, failed or crashed).

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
    running: bool,
    waiting: VecDeque<T>,
}

impl<T> Default for RebuildQueue<T> {
    fn default() -> Self {
        RebuildQueue { running: false, waiting: VecDeque::new() }
    }
}

impl<T> RebuildQueue<T> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_running(&self) -> bool {
        self.running
    }

    pub fn waiting(&self) -> usize {
        self.waiting.len()
    }

    /// A new request starts now when nothing runs; otherwise it joins the back of the line.
    pub fn submit(&mut self, job: T) -> Admission<T> {
        if self.running {
            self.waiting.push_back(job);
            Admission::Queued { position: self.waiting.len() }
        } else {
            self.running = true;
            Admission::Start(job)
        }
    }

    /// The running job ended. Returns the next waiting job, which is now running; `None` leaves the queue idle.
    pub fn finish(&mut self) -> Option<T> {
        match self.waiting.pop_front() {
            Some(next) => Some(next),
            None => {
                self.running = false;
                None
            }
        }
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
    fn an_idle_queue_admits_the_next_request_immediately() {
        let mut q: RebuildQueue<&str> = RebuildQueue::new();
        assert_eq!(q.finish(), None);
        assert_eq!(q.submit("x"), Admission::Start("x"));
    }
}
