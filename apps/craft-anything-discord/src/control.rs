//! The control commands' rules (`/status`, `/stop`, `/resume`, `/retry`, `/note`): who may use
//! them, what `/status` says and when a port can be resumed or retried. No Discord types.

use crate::board::{Board, FeatureState, format_elapsed};
use crate::jobs::JobRecord;
use crate::state::PortState;
use std::time::Duration;

/// Only members with the porter role run the control commands.
pub fn may_control(member_roles: &[u64], porter_role: u64) -> bool {
    member_roles.contains(&porter_role)
}

/// Where a port's job stands in the rebuild queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Running,
    /// 1 is next in line.
    Waiting(usize),
    NotQueued,
    /// A project mirrored from the craft desktop app: it builds there, not in the bot's queue.
    App,
}

impl Place {
    fn describe(self) -> String {
        match self {
            Place::Running => "running now".to_owned(),
            Place::Waiting(n) => format!("waiting, position {n}"),
            Place::NotQueued => "not in the queue".to_owned(),
            Place::App => "built in the craft desktop app (from app)".to_owned(),
        }
    }
}

const STATES: [(FeatureState, &str); 5] = [
    (FeatureState::Planned, "Planned"),
    (FeatureState::InProgress, "In progress"),
    (FeatureState::ReadyToTest, "Ready to test"),
    (FeatureState::Passed, "Passed"),
    (FeatureState::Broken, "Broken"),
];

fn features_text(board: &Board) -> String {
    let lines: Vec<String> = STATES
        .iter()
        .filter_map(|(state, label)| {
            let names: Vec<&str> = board.features().iter().filter(|(_, s)| s == state).map(|(n, _)| n.as_str()).collect();
            (!names.is_empty()).then(|| format!("{} {label} ({}): {}", state.icon(), names.len(), names.join(", ")))
        })
        .collect();
    if lines.is_empty() { "No features reported yet.".to_owned() } else { lines.join("\n") }
}

/// The `/status` answer for a port: its stage, its features by state, its place in the queue, how
/// long its last job ran and what the port has cost.
pub fn status_text(state: &PortState, place: Place, elapsed: Duration) -> String {
    let board = state.board.clone().unwrap_or_default();
    let stage = board.stage().map_or("not started", |s| s.label());
    let job = state.last_job().map_or_else(|| "none yet".to_owned(), |j| format!("#{} ({})", j.id, j.status.label()));
    format!(
        "**{}**\nStage: {stage}\nQueue: {}\nLast job: {job}\nRunning time: {}\nUsage: {}\n\n**Features**\n{}",
        state.label,
        place.describe(),
        format_elapsed(elapsed),
        state.usage_line(None).replace('\n', " · "),
        features_text(&board),
    )
}

/// The job `/resume` carries on, or why there is none. `project_exists`: the port's project folder
/// holds a project (`rebuild --continue-in` needs one).
pub fn resumable(state: &PortState, project_exists: bool) -> Result<&JobRecord, String> {
    let job = state.last_job().ok_or("This port has no run to resume yet.")?;
    if job.status.is_active() {
        return Err(format!("This port's job is already {}.", job.status.label()));
    }
    if !job.status.can_resume() {
        return Err(format!("The last run {}; there is nothing to resume. Use /retry to run it again.", job.status.label()));
    }
    if !project_exists {
        return Err("The project folder holds no project yet, so there is nothing to continue. Use /retry to start again.".to_owned());
    }
    Ok(job)
}

/// The job `/retry` runs again from its request, or why it can't.
pub fn retryable(state: &PortState) -> Result<&JobRecord, String> {
    let job = state.last_job().ok_or("This port has no run to retry yet.")?;
    if job.status.is_active() {
        return Err(format!("This port's job is already {}; /stop it first.", job.status.label()));
    }
    Ok(job)
}

/// Whether `/stop` has a job to stop.
pub fn stoppable(state: &PortState) -> Result<&JobRecord, String> {
    state.last_job().filter(|j| j.status.is_active()).ok_or_else(|| "Nothing is running or waiting for this port.".to_owned())
}

/// The commands that control a run; a project mirrored from the desktop app is run from there.
pub const RUN_COMMANDS: [&str; 3] = ["stop", "resume", "retry"];

/// The answer to `command` in a mirrored project's channel when the bot can't do it: the build is
/// the desktop app's. `None` for the commands that work there (`/status`, `/note`).
pub fn app_owned_answer(command: &str) -> Option<String> {
    RUN_COMMANDS.contains(&command).then(|| {
        format!(
            "This build belongs to the craft desktop app, so /{command} can't control it: stop, resume or retry it in the app. /status and /note work here."
        )
    })
}

/// The note a porter leaves the builder with `/note`.
pub fn porter_note(porter: &str, text: &str) -> String {
    format!("A porter ({porter}) left you a note:\n{}", text.trim())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::{JobKind, JobStatus};
    use crate::run::Target;
    use chrono::Utc;

    const PORTER: u64 = 10;
    const TESTER: u64 = 20;

    #[test]
    fn only_porters_may_run_the_control_commands() {
        assert!(may_control(&[TESTER, PORTER], PORTER));
        assert!(!may_control(&[TESTER], PORTER), "testers review, they don't control runs");
        assert!(!may_control(&[], PORTER));
    }

    fn port_with(status: Option<JobStatus>) -> PortState {
        let mut state = PortState { label: "Snake".into(), ..PortState::default() };
        if let Some(status) = status {
            let id = state.new_job(JobKind::Port, "/in.apk".into(), Target::Unity, Utc::now());
            state.start_job(id, Utc::now());
            state.end_job(id, status, Utc::now());
        }
        state
    }

    #[test]
    fn resume_needs_an_ended_unfinished_run_and_a_project() {
        assert!(resumable(&port_with(Some(JobStatus::Stopped)), true).is_ok());
        assert!(resumable(&port_with(Some(JobStatus::Interrupted)), true).is_ok());
        assert!(resumable(&port_with(Some(JobStatus::Stopped)), false).is_err(), "nothing to continue");
        assert!(resumable(&port_with(Some(JobStatus::Finished)), true).is_err());
        assert!(resumable(&port_with(Some(JobStatus::Running)), true).is_err());
        assert!(resumable(&port_with(None), true).is_err());
    }

    #[test]
    fn retry_and_stop_depend_on_whether_a_job_is_active() {
        assert!(retryable(&port_with(Some(JobStatus::Finished))).is_ok());
        assert!(retryable(&port_with(Some(JobStatus::Queued))).is_err());
        assert!(stoppable(&port_with(Some(JobStatus::Running))).is_ok());
        assert!(stoppable(&port_with(Some(JobStatus::Failed))).is_err());
    }

    #[test]
    fn a_mirrored_project_refuses_run_control_but_not_status_or_notes() {
        for command in ["stop", "resume", "retry"] {
            assert!(app_owned_answer(command).is_some_and(|a| a.contains("desktop app") && a.contains(command)), "{command}");
        }
        assert_eq!(app_owned_answer("status"), None);
        assert_eq!(app_owned_answer("note"), None);
        let text = status_text(&port_with(Some(JobStatus::Running)), Place::App, Duration::ZERO);
        assert!(text.contains("desktop app"), "{text}");
    }

    #[test]
    fn status_shows_the_queue_place_and_features_by_state() {
        let mut state = port_with(Some(JobStatus::Running));
        let mut board = Board::new();
        for (name, s) in [("Map", "passed"), ("Shop", "broken"), ("Inn", "passed")] {
            board.apply(&crate::events::Event::Feature { name: name.into(), state: s.into(), guide: None, note: None, similarity: None });
        }
        state.board = Some(board);
        let text = status_text(&state, Place::Waiting(2), Duration::from_secs(65));
        assert!(text.contains("position 2"), "{text}");
        assert!(text.contains("Passed (2): Map, Inn") && text.contains("Broken (1): Shop"), "{text}");
    }
}
