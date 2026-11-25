//! The rebuild queue on disk, `PORTS_ROOT/.bot/queue.json`, so the line survives a restart. It is
//! rewritten (atomically: a temp file, then a rename) whenever the line changes. After a restart
//! the jobs that were waiting are queued again in order; the job that was running is not started
//! again: it is marked interrupted and its port is offered `/resume`.

use crate::jobs::{JobKind, JobStatus};
use crate::run::Target;
use crate::state::PortState;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The bot's own folder under `PORTS_ROOT` (no port has a slug starting with a dot).
pub const BOT_DIR: &str = ".bot";
pub const QUEUE_FILE: &str = "queue.json";
/// Where a queue file that can't be read is moved, so the next save doesn't overwrite it.
pub const UNREADABLE_FILE: &str = "queue.json.unreadable";
/// What the board says about a run the restart cut off.
pub const INTERRUPTED_MESSAGE: &str = "Interrupted: the bot restarted while this ran. Use /resume to carry on.";

/// One job in the line: enough to run it again with no memory of the bot that queued it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub slug: String,
    /// The port's channel, how the port is found again.
    pub channel: u64,
    /// The job's id in the port's `bot.json`.
    pub job: u64,
    pub kind: JobKind,
    pub input: PathBuf,
    pub target: Target,
    pub label: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedQueue {
    pub running: Option<Entry>,
    /// Next in line first.
    pub waiting: Vec<Entry>,
}

/// What to do with a saved queue after a restart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recovery {
    /// Queue these again, in this order.
    pub requeue: Vec<Entry>,
    /// This was running: mark it interrupted and offer `/resume`; don't start it.
    pub interrupted: Option<Entry>,
}

pub fn path(ports_root: &Path) -> PathBuf {
    ports_root.join(BOT_DIR).join(QUEUE_FILE)
}

impl SavedQueue {
    /// An empty queue when there is no file yet; a file that can't be read is an error.
    pub fn load(ports_root: &Path) -> Result<SavedQueue, String> {
        let path = path(ports_root);
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(SavedQueue::default()),
            Err(e) => return Err(format!("couldn't read {}: {e}", path.display())),
        };
        serde_json::from_slice(&bytes).map_err(|e| format!("{} is not valid: {e}", path.display()))
    }

    pub fn save(&self, ports_root: &Path) -> Result<(), String> {
        let path = path(ports_root);
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| format!("couldn't encode the queue: {e}"))?;
        craft_anything_platform::fs::write_atomic(&path, &bytes).map_err(|e| format!("couldn't save {}: {}", path.display(), e.message))
    }

    /// Loads the queue for a restart. A file that can't be read is moved aside to
    /// [`UNREADABLE_FILE`] (kept for a person to look at) and the queue starts empty; the error
    /// says so, for the caller to report.
    pub fn load_for_restart(ports_root: &Path) -> (SavedQueue, Option<String>) {
        match SavedQueue::load(ports_root) {
            Ok(queue) => (queue, None),
            Err(e) => {
                let aside = ports_root.join(BOT_DIR).join(UNREADABLE_FILE);
                let kept = match std::fs::rename(path(ports_root), &aside) {
                    Ok(()) => format!("it was moved to {}", aside.display()),
                    Err(move_error) => format!("moving it aside failed too ({move_error}); the next save replaces it"),
                };
                (SavedQueue::default(), Some(format!("{e}; the queue starts empty and {kept}")))
            }
        }
    }

    pub fn recover(self) -> Recovery {
        Recovery { requeue: self.waiting, interrupted: self.running }
    }
}

impl PortState {
    /// Job `id` was running when the bot stopped: it is interrupted, and the board says so. A job
    /// that already ended keeps its status. Returns whether anything changed.
    pub fn mark_interrupted(&mut self, id: u64, now: DateTime<Utc>) -> bool {
        if !self.job(id).is_some_and(|j| j.status.is_active()) {
            return false;
        }
        self.end_job(id, JobStatus::Interrupted, now);
        let finished = crate::events::Event::Finished { ok: false, paused: true, message: INTERRUPTED_MESSAGE.to_owned() };
        self.board.get_or_insert_with(crate::board::Board::new).apply(&finished);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(slug: &str, job: u64) -> Entry {
        Entry { slug: slug.into(), channel: 7, job, kind: JobKind::Port, input: "/in.apk".into(), target: Target::Cocos, label: slug.into() }
    }

    #[test]
    fn the_queue_round_trips_through_its_file_in_order() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(SavedQueue::load(root.path()), Ok(SavedQueue::default()), "no file: an empty queue");
        let queue = SavedQueue { running: Some(entry("snake", 1)), waiting: vec![entry("tetris", 1), entry("pong", 2)] };
        queue.save(root.path()).unwrap();
        let loaded = SavedQueue::load(root.path()).unwrap();
        assert_eq!(loaded, queue);
        let names: Vec<String> =
            std::fs::read_dir(path(root.path()).parent().unwrap()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, [QUEUE_FILE], "no temp file is left behind");
    }

    #[test]
    fn a_broken_queue_file_is_reported_and_kept_aside_not_overwritten() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(BOT_DIR)).unwrap();
        std::fs::write(path(root.path()), b"{ not json").unwrap();
        assert!(SavedQueue::load(root.path()).is_err());
        let (queue, error) = SavedQueue::load_for_restart(root.path());
        assert_eq!(queue, SavedQueue::default());
        assert!(error.is_some());
        let kept = std::fs::read(root.path().join(BOT_DIR).join(UNREADABLE_FILE)).unwrap();
        assert_eq!(kept, b"{ not json", "the broken file is kept for a person");
    }

    #[test]
    fn after_a_restart_the_waiting_jobs_requeue_and_the_running_one_is_interrupted() {
        let queue = SavedQueue { running: Some(entry("snake", 3)), waiting: vec![entry("tetris", 1), entry("pong", 1)] };
        let recovery = queue.recover();
        assert_eq!(recovery.interrupted.map(|e| e.slug), Some("snake".to_owned()), "not started again");
        assert_eq!(recovery.requeue.iter().map(|e| e.slug.as_str()).collect::<Vec<_>>(), ["tetris", "pong"]);

        let now = Utc::now();
        let mut state = PortState::default();
        let id = state.new_job(JobKind::Port, "/in.apk".into(), Target::Unity, now);
        state.start_job(id, now);
        assert!(state.mark_interrupted(id, now));
        let job = state.job(id).cloned();
        assert!(job.is_some_and(|j| j.status == JobStatus::Interrupted && j.status.can_resume()));
        let view = state.board.as_ref().map(|b| b.render(std::time::Duration::ZERO, String::new()));
        assert!(view.and_then(|v| v.outcome).is_some_and(|o| o.contains("/resume")), "the board offers /resume");
        assert!(!state.mark_interrupted(id, now), "an ended job is left alone");
    }
}
