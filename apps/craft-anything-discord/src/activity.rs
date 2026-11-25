//! What happened on a port and when, kept in its `bot.json` for the daily summary: features that
//! reached `ready_to_test`, `passed` or `broken`, testers' Pass and Fail, and usage summed per hour
//! (so a long run adds at most one entry an hour). Entries older than [`KEEP`] are dropped.

use crate::cost::Usage;
use crate::state::PortState;
use chrono::{DateTime, DurationRound, TimeDelta, Utc};
use serde::{Deserialize, Serialize};

/// How long activity is kept.
pub const KEEP: TimeDelta = TimeDelta::days(8);
/// The feature states the summary counts.
pub const SUMMARY_STATES: [&str; 3] = ["ready_to_test", "passed", "broken"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Activity {
    Feature {
        at: DateTime<Utc>,
        feature: String,
        state: String,
    },
    Review {
        at: DateTime<Utc>,
        feature: String,
        passed: bool,
    },
    /// The usage of every turn that ended in the hour starting at `hour`.
    Usage {
        hour: DateTime<Utc>,
        usage: Usage,
    },
}

impl Activity {
    pub fn at(&self) -> DateTime<Utc> {
        match self {
            Activity::Feature { at, .. } | Activity::Review { at, .. } => *at,
            Activity::Usage { hour, .. } => *hour,
        }
    }
}

/// The start of the hour `at` is in.
fn hour_of(at: DateTime<Utc>) -> DateTime<Utc> {
    at.duration_trunc(TimeDelta::hours(1)).unwrap_or(at)
}

impl PortState {
    fn push_activity(&mut self, entry: Activity, now: DateTime<Utc>) {
        let oldest = now - KEEP;
        self.activity.retain(|a| a.at() >= oldest);
        self.activity.push(entry);
    }

    /// A feature event from the builder; only the states the summary counts are kept.
    pub fn record_feature(&mut self, feature: &str, state: &str, now: DateTime<Utc>) {
        if SUMMARY_STATES.contains(&state) {
            self.push_activity(Activity::Feature { at: now, feature: feature.to_owned(), state: state.to_owned() }, now);
        }
    }

    /// A tester's Pass or Fail.
    pub fn record_review(&mut self, feature: &str, passed: bool, now: DateTime<Utc>) {
        self.push_activity(Activity::Review { at: now, feature: feature.to_owned(), passed }, now);
    }

    /// Adds a turn's usage to this hour's entry.
    pub(crate) fn record_usage(&mut self, turn: &Usage, now: DateTime<Utc>) {
        let hour = hour_of(now);
        let current = self.activity.iter_mut().rev().find_map(|a| match a {
            Activity::Usage { hour: h, usage } if *h == hour => Some(usage),
            _ => None,
        });
        match current {
            Some(usage) => usage.add(turn),
            None => self.push_activity(Activity::Usage { hour, usage: turn.clone() }, now),
        }
    }

    /// Activity at or after `since`.
    pub fn activity_since(&self, since: DateTime<Utc>) -> impl Iterator<Item = &Activity> {
        self.activity.iter().filter(move |a| a.at() >= since)
    }

    /// Whether anything happened on the port at or after `since`: activity, or a job queued,
    /// started or ended.
    pub fn touched_since(&self, since: DateTime<Utc>) -> bool {
        self.activity_since(since).next().is_some()
            || self.jobs.iter().any(|j| [Some(j.queued_at), j.started_at, j.ended_at].into_iter().flatten().any(|t| t >= since))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_in_one_hour_is_one_entry_and_old_entries_go() {
        let now = Utc::now();
        let mut state = PortState::default();
        let mut turn = Usage::default();
        turn.add_turn(1, 1, false, Some(0.1), true);
        state.record_usage(&turn, now);
        state.record_usage(&turn, now);
        assert_eq!(state.activity.len(), 1, "same hour: summed");
        state.record_review("Map", true, now + KEEP + TimeDelta::hours(2));
        assert_eq!(state.activity.len(), 1, "the old usage entry was dropped");
        state.record_feature("Map", "in_progress", now);
        assert_eq!(state.activity.len(), 1, "in_progress is not summarised");
    }
}
