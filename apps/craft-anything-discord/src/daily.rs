//! The daily summary posted to `DAILY_CHANNEL_ID`: when it is due (a local time of day, in a
//! fixed UTC offset or the system's time zone), what it says (the last 24 hours across all ports)
//! and the date it was last posted, kept in `PORTS_ROOT/.bot/daily.json` so a restart never posts
//! one day twice.

use crate::activity::Activity;
use crate::cost::Usage;
use crate::saved_queue::BOT_DIR;
use crate::state::PortState;
use chrono::{DateTime, FixedOffset, Local, NaiveDate, NaiveTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub const DAILY_FILE: &str = "daily.json";
/// The summary's time when `DAILY_SUMMARY_TIME` is not set.
pub const DEFAULT_TIME: &str = "09:00";
/// How far back the summary looks.
pub const WINDOW: TimeDelta = TimeDelta::hours(24);

/// When the summary is due each day.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Schedule {
    pub at: NaiveTime,
    /// `None`: the system's local time zone.
    pub offset: Option<FixedOffset>,
}

impl Schedule {
    /// `time` is `HH:MM` (default [`DEFAULT_TIME`]); `offset` is like `+07:00`. An error names the problem.
    pub fn parse(time: Option<&str>, offset: Option<&str>) -> Result<Schedule, (&'static str, String)> {
        let time = time.unwrap_or(DEFAULT_TIME);
        let at = NaiveTime::parse_from_str(time, "%H:%M").map_err(|_| ("DAILY_SUMMARY_TIME", "expected a 24-hour time such as 09:00".to_owned()))?;
        let offset = offset
            .map(|o| o.parse::<FixedOffset>().map_err(|_| ("DAILY_SUMMARY_UTC_OFFSET", "expected an offset such as +07:00 or -05:30".to_owned())))
            .transpose()?;
        Ok(Schedule { at, offset })
    }

    /// The local date and time at `now`.
    fn local(&self, now: DateTime<Utc>) -> chrono::NaiveDateTime {
        match self.offset {
            Some(offset) => now.with_timezone(&offset).naive_local(),
            None => now.with_timezone(&Local).naive_local(),
        }
    }

    /// The local date to post for, once today's time has come and today is not posted yet.
    pub fn due(&self, now: DateTime<Utc>, last_posted: Option<NaiveDate>) -> Option<NaiveDate> {
        let local = self.local(now);
        let today = local.date();
        (local.time() >= self.at && last_posted.is_none_or(|last| last < today)).then_some(today)
    }
}

/// The date of the last summary posted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DailyState {
    pub last_posted: Option<NaiveDate>,
}

impl DailyState {
    pub fn path(ports_root: &Path) -> PathBuf {
        ports_root.join(BOT_DIR).join(DAILY_FILE)
    }

    /// Nothing posted yet when there is no file; a file that can't be read is an error (so the bot
    /// doesn't post again a day it may already have posted).
    pub fn load(ports_root: &Path) -> Result<DailyState, String> {
        let path = Self::path(ports_root);
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(DailyState::default()),
            Err(e) => return Err(format!("couldn't read {}: {e}", path.display())),
        };
        serde_json::from_slice(&bytes).map_err(|e| format!("{} is not valid: {e}", path.display()))
    }

    pub fn save(&self, ports_root: &Path) -> Result<(), String> {
        let path = Self::path(ports_root);
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| format!("couldn't encode the daily state: {e}"))?;
        craft_anything_platform::fs::write_atomic(&path, &bytes).map_err(|e| format!("couldn't save {}: {}", path.display(), e.message))
    }
}

/// One port's last 24 hours.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PortDay {
    pub label: String,
    pub channel: u64,
    /// Features that reached each summarised state, by state name, each feature once.
    pub ready: BTreeSet<String>,
    pub passed: BTreeSet<String>,
    pub broken: BTreeSet<String>,
    pub passes: u64,
    pub fails: u64,
    pub usage: Usage,
}

/// Every port touched in the 24 hours before `now`, in the order given.
pub fn summarize(ports: &[PortState], now: DateTime<Utc>) -> Vec<PortDay> {
    let since = now - WINDOW;
    ports.iter().filter(|p| p.touched_since(since)).map(|p| port_day(p, since)).collect()
}

fn port_day(port: &PortState, since: DateTime<Utc>) -> PortDay {
    let mut day = PortDay { label: port.label.clone(), channel: port.channel, ..PortDay::default() };
    for entry in port.activity_since(since) {
        match entry {
            Activity::Feature { feature, state, .. } => {
                let set = match state.as_str() {
                    "ready_to_test" => &mut day.ready,
                    "passed" => &mut day.passed,
                    _ => &mut day.broken,
                };
                set.insert(feature.clone());
            }
            Activity::Review { passed: true, .. } => day.passes += 1,
            Activity::Review { passed: false, .. } => day.fails += 1,
            Activity::Usage { usage, .. } => day.usage.add(usage),
        }
    }
    day
}

fn names(set: &BTreeSet<String>) -> String {
    set.iter().cloned().collect::<Vec<_>>().join(", ")
}

/// The summary message for `date`.
pub fn render(date: NaiveDate, days: &[PortDay]) -> String {
    let mut text = format!("📋 **Daily summary for {date}** (last 24 hours)\n");
    if days.is_empty() {
        text.push_str("No port was worked on.");
        return text;
    }
    for day in days {
        text.push_str(&format!("\n**{}** <#{}>\n", day.label, day.channel));
        for (icon, what, set) in [("🧪", "Ready to test", &day.ready), ("✅", "Passed", &day.passed), ("❌", "Broken", &day.broken)] {
            if !set.is_empty() {
                text.push_str(&format!("{icon} {what}: {}\n", names(set)));
            }
        }
        text.push_str(&format!("Tests: {} passed, {} failed · Cost: {}\n", day.passes, day.fails, day.usage.cost_text()));
    }
    let total = Usage::total(days.iter().map(|d| &d.usage));
    let (passes, fails) = days.iter().fold((0, 0), |(p, f), d| (p + d.passes, f + d.fails));
    text.push_str(&format!("\n{} ports · tests {passes} passed, {fails} failed · total cost {}", days.len(), total.cost_text()));
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::JobKind;
    use crate::run::Target;
    use chrono::TimeZone;

    fn schedule(at: &str) -> Schedule {
        Schedule::parse(Some(at), Some("+07:00")).unwrap()
    }

    #[test]
    fn the_summary_is_due_once_a_day_after_its_time() {
        let s = schedule("09:00");
        // 01:59 UTC is 08:59 at +07:00, 02:00 UTC is 09:00.
        let before = Utc.with_ymd_and_hms(2026, 11, 26, 1, 59, 0).unwrap();
        let at = Utc.with_ymd_and_hms(2026, 11, 26, 2, 0, 0).unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 11, 26);
        assert_eq!(s.due(before, None), None, "not yet");
        assert_eq!(s.due(at, None), today);
        assert_eq!(s.due(at + TimeDelta::hours(5), today), None, "already posted today");
        assert_eq!(s.due(at, today.and_then(|d| d.pred_opt())), today, "posted yesterday");
        assert_eq!(s.due(at + TimeDelta::days(1), today), today.and_then(|d| d.succ_opt()), "the next day is due again");
    }

    #[test]
    fn the_posted_date_survives_a_restart_so_a_day_is_posted_once() {
        let root = tempfile::tempdir().unwrap();
        let s = schedule("09:00");
        let now = Utc.with_ymd_and_hms(2026, 11, 26, 3, 0, 0).unwrap();
        let due = s.due(now, DailyState::load(root.path()).unwrap().last_posted);
        assert!(due.is_some());
        DailyState { last_posted: due }.save(root.path()).unwrap();
        let after_restart = DailyState::load(root.path()).unwrap();
        assert_eq!(s.due(now + TimeDelta::minutes(30), after_restart.last_posted), None);
    }

    #[test]
    fn bad_times_and_offsets_name_their_key() {
        assert_eq!(Schedule::parse(Some("9am"), None).map_err(|e| e.0), Err("DAILY_SUMMARY_TIME"));
        assert_eq!(Schedule::parse(None, Some("UTC+7")).map_err(|e| e.0), Err("DAILY_SUMMARY_UTC_OFFSET"));
        assert!(Schedule::parse(None, None).is_ok_and(|s| s.offset.is_none()), "default time, system zone");
    }

    #[test]
    fn the_summary_counts_the_last_day_only() {
        let now = Utc::now();
        let old = now - WINDOW - TimeDelta::hours(1);
        let mut busy = PortState { label: "Snake".into(), channel: 1, ..PortState::default() };
        busy.record_feature("Map", "ready_to_test", now);
        busy.record_feature("Map", "passed", now);
        busy.record_feature("Shop", "broken", now);
        busy.record_review("Map", true, now);
        busy.record_review("Shop", false, now);
        let mut turn = Usage::default();
        turn.add_turn(1, 1, false, Some(0.5), true);
        let job = busy.new_job(JobKind::Port, "/in.apk".into(), Target::Unity, old);
        busy.add_usage(job, &turn, now);
        let mut quiet = PortState { label: "Pong".into(), channel: 2, ..PortState::default() };
        quiet.new_job(JobKind::Port, "/in.apk".into(), Target::Unity, old);
        let days = summarize(&[busy, quiet], now);
        assert_eq!(days.len(), 1, "a port with nothing in the last day is left out");
        let day = &days[0];
        assert!(day.ready.contains("Map") && day.passed.contains("Map") && day.broken.contains("Shop"));
        assert_eq!((day.passes, day.fails), (1, 1));
        let text = render(now.date_naive(), &days);
        assert!(text.contains("Snake") && text.contains("~$"), "{text}");
    }
}
