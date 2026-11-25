//! A tester's verdict on a feature: recorded through the engine (`build_feature_report` on the
//! port's project folder), and for a failure, left as a note for the builder through Craft's own
//! notes (`craft_anything_ai::rebuild::notes`), so a live run's next step or the next run on the
//! folder reads it. Recording goes through [`FeatureRecorder`], so tests use a fake.

use crate::text::clip;
use craft_anything_platform::process::Process;
use serde_json::{Value, json};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The engine keeps a feature note of at most this many characters.
pub const RECORD_NOTE_LIMIT: usize = 1_000;
/// The Fail form's "What went wrong?" and "Steps to reproduce" limits, in characters.
pub const WHAT_LIMIT: u16 = 1_000;
pub const STEPS_LIMIT: u16 = 1_000;
const RECORD_TIMEOUT: Duration = Duration::from_secs(120);

/// What the tester said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    Fail { what: String, steps: Option<String> },
}

impl Verdict {
    /// Builds a Fail from the form's fields; blank steps count as none. `None` when "What went
    /// wrong?" is blank (Discord requires it, but the bot does not rely on that).
    pub fn fail(what: &str, steps: Option<&str>) -> Option<Verdict> {
        let what = what.trim();
        if what.is_empty() {
            return None;
        }
        let steps = steps.map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned);
        Some(Verdict::Fail { what: what.to_owned(), steps })
    }

    /// The engine's feature state for this verdict.
    pub fn state(&self) -> &'static str {
        match self {
            Verdict::Pass => "passed",
            Verdict::Fail { .. } => "broken",
        }
    }

    /// The note kept with the feature's record (the engine's limit applies).
    pub fn record_note(&self, tester: &str) -> String {
        match self {
            Verdict::Pass => clip(&format!("Passed by {tester}"), RECORD_NOTE_LIMIT),
            Verdict::Fail { what, steps } => {
                let steps = steps.as_deref().map(|s| format!("\nSteps to reproduce: {s}")).unwrap_or_default();
                clip(&format!("Failed by {tester}: {what}{steps}"), RECORD_NOTE_LIMIT)
            }
        }
    }

    /// The Log thread line.
    pub fn log_line(&self, feature: &str, tester: &str) -> String {
        match self {
            Verdict::Pass => format!("✅ Tester {tester} passed **{feature}**."),
            Verdict::Fail { what, steps } => {
                let steps = steps.as_deref().map(|s| format!("\nSteps to reproduce: {s}")).unwrap_or_default();
                format!("❌ Tester {tester} failed **{feature}**.\nWhat went wrong: {what}{steps}")
            }
        }
    }
}

/// The note the builder reads at its next step, or the next run on the folder: the whole failure.
pub fn builder_note(feature: &str, tester: &str, what: &str, steps: Option<&str>) -> String {
    let steps = steps.map(|s| format!("\nSteps to reproduce: {s}")).unwrap_or_default();
    format!(
        "A tester ({tester}) tried the feature \"{feature}\" with its guide and it failed.\nWhat went wrong: {what}{steps}\n\
         It is recorded as broken. Fix it, then report it ready_to_test again with its guide."
    )
}

/// Records a feature's state through the engine.
pub trait FeatureRecorder: Send + Sync {
    fn record(&self, project: &Path, feature: &str, state: &str, note: &str) -> Result<(), String>;
}

/// Records with `CRAFT_CLI build_feature_report --building <project> --json {...}`.
pub struct CliRecorder {
    pub craft_cli: PathBuf,
}

/// The arguments after `CRAFT_CLI` for one record.
pub fn record_args(project: &Path, feature: &str, state: &str, note: &str) -> Vec<OsString> {
    let params = json!({"name": feature, "state": state, "note": note}).to_string();
    vec!["build_feature_report".into(), "--building".into(), project.into(), "--json".into(), params.into()]
}

impl FeatureRecorder for CliRecorder {
    fn record(&self, project: &Path, feature: &str, state: &str, note: &str) -> Result<(), String> {
        let out = Process::new(&self.craft_cli)
            .args(record_args(project, feature, state, note))
            .label("craft-anything-cli build_feature_report")
            .timeout(RECORD_TIMEOUT)
            .run()
            .map_err(|e| e.message)?;
        if out.success {
            return Ok(());
        }
        // The CLI prints a structured error on stdout; fall back to stderr when it has none.
        let said = serde_json::from_slice::<Value>(&out.stdout).ok().and_then(|v| v.pointer("/error/message").and_then(Value::as_str).map(str::to_owned));
        Err(said.unwrap_or_else(|| clip(out.stderr_text().trim(), 300)))
    }
}

/// What came of recording a verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorded {
    /// The builder note could not be left (the record itself went in).
    pub note_error: Option<String>,
}

/// Records the verdict on `feature` and, for a failure, leaves the builder a note. An error means
/// nothing was recorded.
pub fn record(recorder: &dyn FeatureRecorder, project: &Path, feature: &str, tester: &str, verdict: &Verdict) -> Result<Recorded, String> {
    recorder.record(project, feature, verdict.state(), &verdict.record_note(tester))?;
    let note_error = match verdict {
        Verdict::Pass => None,
        Verdict::Fail { what, steps } => leave_builder_note(project, &builder_note(feature, tester, what, steps.as_deref())).err(),
    };
    Ok(Recorded { note_error })
}

/// Leaves `text` in the project's notes for the builder: a live run's builder takes it at its next
/// step; otherwise the next run on the folder that brings no request of its own (`--continue-in
/// --resume`) starts with it.
pub fn leave_builder_note(project: &Path, text: &str) -> Result<(), String> {
    use craft_anything_ai::rebuild::notes;
    let ws = craft_anything_engine::workspace::Workspace::open(project).map_err(|e| e.message)?;
    let now = SystemTime::now();
    // Ids order the notes; a run's own ids count from 1, so a time-based one can't clash with them.
    let id = u64::try_from(now.duration_since(UNIX_EPOCH).unwrap_or_default().as_millis()).unwrap_or(u64::MAX);
    let left = notes::Left { text: text.to_owned(), shown: text.to_owned(), pictures: Vec::new(), added: Some(now) };
    notes::hand_over(&notes::notes_dir(&ws.inbox_dir()), id, text, Some(&left)).map_err(|e| e.message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft_anything_platform::sync::Mutex;

    #[derive(Default)]
    struct FakeRecorder {
        calls: Mutex<Vec<(String, String, String)>>,
        refuse: bool,
    }

    impl FeatureRecorder for FakeRecorder {
        fn record(&self, _project: &Path, feature: &str, state: &str, note: &str) -> Result<(), String> {
            if self.refuse {
                return Err("refused".into());
            }
            if let Ok(mut calls) = self.calls.lock() {
                calls.push((feature.to_owned(), state.to_owned(), note.to_owned()));
            }
            Ok(())
        }
    }

    fn project() -> (tempfile::TempDir, PathBuf) {
        use craft_anything_engine::workspace::{TargetRef, Workspace};
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::create(dir.path(), "Snake", "A snake game", TargetRef::None).unwrap();
        (dir, ws.root)
    }

    fn left_notes(project: &Path) -> Vec<String> {
        use craft_anything_ai::rebuild::notes;
        let ws = craft_anything_engine::workspace::Workspace::open(project).unwrap();
        notes::take_left(&notes::notes_dir(&ws.inbox_dir()), true).into_iter().map(|n| n.text).collect()
    }

    #[test]
    fn a_fail_records_the_feature_broken_with_the_note_and_leaves_the_builder_a_note() {
        let (_d, project) = project();
        let recorder = FakeRecorder::default();
        let verdict = Verdict::fail("The map is blank", Some("Open the map")).unwrap();
        let recorded = record(&recorder, &project, "Map", "Lan", &verdict).unwrap();
        assert_eq!(recorded.note_error, None);
        let calls = recorder.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1);
        let (feature, state, note) = &calls[0];
        assert_eq!((feature.as_str(), state.as_str()), ("Map", "broken"));
        assert!(note.contains("The map is blank") && note.contains("Lan"), "{note}");
        let notes = left_notes(&project);
        assert_eq!(notes.len(), 1, "the next run starts with the failure");
        assert!(notes[0].contains("The map is blank") && notes[0].contains("Open the map") && notes[0].contains("Map"), "{notes:?}");
    }

    #[test]
    fn a_pass_records_passed_and_leaves_no_note() {
        let (_d, project) = project();
        let recorder = FakeRecorder::default();
        record(&recorder, &project, "Map", "Lan", &Verdict::Pass).unwrap();
        assert_eq!(recorder.calls.lock().unwrap()[0].1, "passed");
        assert!(left_notes(&project).is_empty());
    }

    #[test]
    fn a_refused_record_leaves_nothing_behind() {
        let (_d, project) = project();
        let recorder = FakeRecorder { refuse: true, ..FakeRecorder::default() };
        assert!(record(&recorder, &project, "Map", "Lan", &Verdict::fail("blank", None).unwrap()).is_err());
        assert!(left_notes(&project).is_empty(), "no note for a failure that was not recorded");
    }

    #[test]
    fn the_record_note_fits_the_engine_limit() {
        let long = "x".repeat(usize::from(WHAT_LIMIT));
        let verdict = Verdict::fail(&long, Some(&long)).unwrap();
        assert!(verdict.record_note("a tester").chars().count() <= RECORD_NOTE_LIMIT);
        assert_eq!(Verdict::fail("  ", Some("steps")), None, "a blank reason is not a failure");
    }

    #[test]
    fn the_cli_gets_the_project_and_one_json_argument() {
        let args: Vec<String> =
            record_args(Path::new("/ports/snake/project"), "Map \"main\"", "broken", "it's blank").iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(args[..4], ["build_feature_report", "--building", "/ports/snake/project", "--json"]);
        let params: Value = serde_json::from_str(&args[4]).unwrap();
        assert_eq!(params, json!({"name": "Map \"main\"", "state": "broken", "note": "it's blank"}));
    }
}
