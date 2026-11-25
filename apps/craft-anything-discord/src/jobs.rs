//! The runs a port has had, kept in its `bot.json`: what each was (a `/port`, `/resume` or
//! `/retry`), how it stands, when it started and ended, and its tokens and cost. A port's total
//! is the sum of its jobs, worked out where it is shown.

use crate::cost::Usage;
use crate::run::Target;
use crate::state::PortState;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    /// The first run, from `/port`.
    Port,
    /// `/resume`: carry on where the last run stopped.
    Resume,
    /// `/retry`: the request again, on the same folder.
    Retry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    Finished,
    Paused,
    Failed,
    /// `/stop` ended it.
    Stopped,
    /// The bot restarted while it ran.
    Interrupted,
}

impl JobStatus {
    /// Waiting or running: the port has a job in the queue.
    pub fn is_active(self) -> bool {
        matches!(self, JobStatus::Queued | JobStatus::Running)
    }

    /// A run that ended before the port was done, which `/resume` can carry on.
    pub fn can_resume(self) -> bool {
        matches!(self, JobStatus::Paused | JobStatus::Failed | JobStatus::Stopped | JobStatus::Interrupted)
    }

    pub fn label(self) -> &'static str {
        match self {
            JobStatus::Queued => "waiting",
            JobStatus::Running => "running",
            JobStatus::Finished => "finished",
            JobStatus::Paused => "paused",
            JobStatus::Failed => "failed",
            JobStatus::Stopped => "stopped",
            JobStatus::Interrupted => "interrupted by a bot restart",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobRecord {
    /// Unique within the port, counting from 1.
    pub id: u64,
    pub kind: JobKind,
    pub status: JobStatus,
    /// The downloaded app, `PORTS_ROOT/<slug>/input/<file>`.
    pub input: PathBuf,
    pub target: Target,
    pub queued_at: DateTime<Utc>,
    #[serde(default)]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub ended_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub usage: Usage,
}

impl JobRecord {
    /// How long it ran: to its end, or to `now` while it runs.
    pub fn elapsed(&self, now: DateTime<Utc>) -> Duration {
        let Some(start) = self.started_at else {
            return Duration::ZERO;
        };
        (self.ended_at.unwrap_or(now) - start).to_std().unwrap_or_default()
    }
}

/// The Usage line of the board and `/status`: this run's, then the port's total when it differs.
pub fn usage_line(job: Option<&Usage>, port: &Usage) -> String {
    match job {
        Some(job) if job.turns < port.turns => format!("This run: {}\nPort total: {}", job.describe(), port.describe()),
        Some(job) => job.describe(),
        None => port.describe(),
    }
}

impl PortState {
    /// Records a new job, waiting to run, and returns its id.
    pub fn new_job(&mut self, kind: JobKind, input: PathBuf, target: Target, now: DateTime<Utc>) -> u64 {
        let id = self.jobs.last().map_or(1, |j| j.id.saturating_add(1));
        self.jobs.push(JobRecord {
            id,
            kind,
            status: JobStatus::Queued,
            input,
            target,
            queued_at: now,
            started_at: None,
            ended_at: None,
            usage: Usage::default(),
        });
        id
    }

    pub fn job(&self, id: u64) -> Option<&JobRecord> {
        self.jobs.iter().find(|j| j.id == id)
    }

    pub fn last_job(&self) -> Option<&JobRecord> {
        self.jobs.last()
    }

    pub fn start_job(&mut self, id: u64, now: DateTime<Utc>) {
        if let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) {
            job.status = JobStatus::Running;
            job.started_at = Some(now);
            job.ended_at = None;
        }
    }

    pub fn end_job(&mut self, id: u64, status: JobStatus, now: DateTime<Utc>) {
        if let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) {
            job.status = status;
            job.ended_at = Some(now);
        }
    }

    /// Adds a `turn_done` event's usage to job `id`, and to the hour's activity for the daily summary.
    pub fn add_usage(&mut self, id: u64, turn: &Usage, now: DateTime<Utc>) {
        if let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) {
            job.usage.add(turn);
        }
        self.record_usage(turn, now);
    }

    /// Every job's usage added up.
    pub fn total_usage(&self) -> Usage {
        Usage::total(self.jobs.iter().map(|j| &j.usage))
    }

    /// The Usage line for job `id` (`None`: the port's last job).
    pub fn usage_line(&self, id: Option<u64>) -> String {
        let job = id.and_then(|id| self.job(id)).or_else(|| self.last_job());
        usage_line(job.map(|j| &j.usage), &self.total_usage())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(cost: f64, estimate: bool) -> Usage {
        let mut usage = Usage::default();
        usage.add_turn(10, 5, false, Some(cost), estimate);
        usage
    }

    #[test]
    fn cost_accumulates_per_job_and_the_port_total_spans_jobs() {
        let now = Utc::now();
        let mut state = PortState::default();
        let first = state.new_job(JobKind::Port, "/in.apk".into(), Target::Unity, now);
        state.add_usage(first, &turn(0.5, false), now);
        state.add_usage(first, &turn(0.25, false), now);
        let second = state.new_job(JobKind::Resume, "/in.apk".into(), Target::Unity, now);
        state.add_usage(second, &turn(1.0, true), now);
        assert_ne!(first, second);
        let job = state.job(first).map(|j| j.usage.clone()).unwrap_or_default();
        assert_eq!(job.turns, 2);
        let total = state.total_usage();
        assert_eq!(total.turns, 3);
        assert!(total.cost_usd > job.cost_usd, "the port total includes the later job");
        let second_line = state.usage_line(Some(second));
        assert!(second_line.contains("Port total:") && second_line.ends_with(&total.cost_text()), "{second_line}");
        assert!(total.cost_text().starts_with("~$"), "one estimated job makes the port total an estimate");
        let first_line = state.usage_line(Some(first));
        assert!(first_line.lines().next().is_some_and(|run| !run.contains('~')), "the first job's own cost was exact: {first_line}");
    }

    #[test]
    fn a_job_runs_then_ends_and_its_time_is_its_own() {
        let start = Utc::now();
        let mut state = PortState::default();
        let id = state.new_job(JobKind::Port, "/in.apk".into(), Target::App, start);
        assert!(state.job(id).is_some_and(|j| j.status.is_active() && j.elapsed(start) == Duration::ZERO));
        state.start_job(id, start);
        state.end_job(id, JobStatus::Stopped, start + chrono::Duration::seconds(90));
        let job = state.job(id).cloned();
        assert!(job.as_ref().is_some_and(|j| j.status.can_resume() && !j.status.is_active()));
        let later = start + chrono::Duration::hours(5);
        assert_eq!(job.map(|j| j.elapsed(later)), Some(Duration::from_secs(90)), "an ended job's time stops at its end");
    }
}
