//! The port's features as the builder reports them (`build_feature_report`): one record per
//! feature, kept in the project's inbox, so the app, the CLI and a rebuild run read the same
//! progress. Writes are atomic. A record that can't be read is an error, never an empty list.

use crate::project::write_atomic;
use craft_anything_evidence::{ErrorCode, ToolError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

/// Where the records live: the project's inbox, a folder Craft Anything reserves.
pub const FEATURES_FILE: &str = "inbox/features.json";
pub const FEATURES_VERSION: u32 = 1;
/// Most features one project keeps.
pub const MAX_FEATURES: usize = 500;
const MAX_NAME_CHARS: usize = 120;
const MAX_NOTE_CHARS: usize = 1000;
const MAX_RECORD_BYTES: u64 = 4 << 20;

/// Where a feature stands. The wire names are the snake_case ids below.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FeatureState {
    Planned,
    InProgress,
    ReadyToTest,
    Passed,
    Broken,
}

impl FeatureState {
    /// The state's wire name: the text serde writes for it.
    pub fn id(self) -> &'static str {
        match self {
            FeatureState::Planned => "planned",
            FeatureState::InProgress => "in_progress",
            FeatureState::ReadyToTest => "ready_to_test",
            FeatureState::Passed => "passed",
            FeatureState::Broken => "broken",
        }
    }
}

/// One feature as last reported.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Feature {
    pub name: String,
    pub state: FeatureState,
    /// The guide a tester follows, a project-relative path such as `tests/<feature>.md`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guide: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// How close the build looks to the original, 0 to 100.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub similarity: Option<u8>,
}

/// A report from the builder. The name picks the record; an omitted guide, note or similarity
/// keeps what is stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub name: String,
    pub state: FeatureState,
    /// Already checked by the caller: a file inside the project.
    pub guide: Option<String>,
    pub note: Option<String>,
    pub similarity: Option<u8>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Doc {
    version: u32,
    #[serde(default)]
    features: Vec<Feature>,
}

/// Every feature the project has reported, in the order first reported. No record yet is an
/// empty list; a record that can't be read, or was written by a newer Craft Anything, is an error.
pub fn load(root: &Path) -> Result<Vec<Feature>, ToolError> {
    let path = root.join(FEATURES_FILE);
    let file = match std::fs::File::open(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(ToolError::io(path.display(), e)),
    };
    let mut bytes = Vec::new();
    file.take(MAX_RECORD_BYTES + 1).read_to_end(&mut bytes).map_err(|e| ToolError::io(path.display(), e))?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err(ToolError::new(ErrorCode::OverLimit, format!("{FEATURES_FILE} is too large to read.")));
    }
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| unreadable(format!("Couldn't read {FEATURES_FILE}: {e}")))?;
    let version = value.get("version").and_then(serde_json::Value::as_u64).ok_or_else(|| unreadable(format!("{FEATURES_FILE} has no version.")))?;
    if version > u64::from(FEATURES_VERSION) {
        return Err(ToolError::unsupported(format!("{FEATURES_FILE} was written by a newer Craft Anything, so it is left alone."))
            .with_remediation("Update Craft Anything to keep reporting features in this project."));
    }
    let doc: Doc = serde_json::from_value(value).map_err(|e| unreadable(format!("Couldn't read {FEATURES_FILE}: {e}")))?;
    Ok(doc.features)
}

/// Records a report: the feature with this name is updated, or added. A refused report changes
/// nothing. Returns the record as stored.
pub fn report(root: &Path, r: Report) -> Result<Feature, ToolError> {
    let name = clean_name(&r.name)?;
    let note = clean_note(r.note.as_deref())?;
    if r.similarity.is_some_and(|s| s > 100) {
        return Err(ToolError::invalid("similarity is a number from 0 to 100.").with_remediation("Report the closeness as a whole number from 0 to 100."));
    }
    let mut features = load(root)?;
    let index = features.iter().position(|f| f.name == name);
    let previous = index.map(|i| features[i].clone());
    let feature = Feature {
        name,
        state: r.state,
        guide: r.guide.or_else(|| previous.as_ref().and_then(|p| p.guide.clone())),
        note: note.or_else(|| previous.as_ref().and_then(|p| p.note.clone())),
        similarity: r.similarity.or_else(|| previous.as_ref().and_then(|p| p.similarity)),
    };
    if feature.state == FeatureState::ReadyToTest && feature.guide.is_none() {
        return Err(ToolError::invalid("ready_to_test needs the guide the person will test from.")
            .with_remediation("Write the guide (tests/<feature>.md), then report the feature again with its guide."));
    }
    match index {
        Some(i) => features[i] = feature.clone(),
        None => {
            if features.len() >= MAX_FEATURES {
                return Err(ToolError::new(ErrorCode::OverLimit, format!("A project keeps at most {MAX_FEATURES} features.")));
            }
            features.push(feature.clone());
        }
    }
    save(root, &features)?;
    Ok(feature)
}

/// The features whose record differs from `seen`, which is then brought up to date. A run calls
/// it after each report: the changes are what it tells the person.
pub fn changed(features: &[Feature], seen: &mut HashMap<String, Feature>) -> Vec<Feature> {
    let mut out = Vec::new();
    for f in features {
        if seen.get(&f.name) != Some(f) {
            seen.insert(f.name.clone(), f.clone());
            out.push(f.clone());
        }
    }
    out
}

fn clean_name(name: &str) -> Result<String, ToolError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(ToolError::invalid("A feature needs a name, such as \"Title screen\"."));
    }
    if name.chars().count() > MAX_NAME_CHARS || name.chars().any(char::is_control) {
        return Err(ToolError::invalid(format!("A feature name is one line of up to {MAX_NAME_CHARS} characters.")));
    }
    Ok(name.to_string())
}

/// An empty note is not given (it keeps the stored one).
fn clean_note(note: Option<&str>) -> Result<Option<String>, ToolError> {
    let Some(note) = note.map(str::trim).filter(|n| !n.is_empty()) else {
        return Ok(None);
    };
    if note.chars().count() > MAX_NOTE_CHARS {
        return Err(ToolError::invalid(format!("A note is up to {MAX_NOTE_CHARS} characters.")));
    }
    Ok(Some(note.to_string()))
}

fn save(root: &Path, features: &[Feature]) -> Result<(), ToolError> {
    let path = root.join(FEATURES_FILE);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| ToolError::io(dir.display(), e))?;
    }
    let doc = Doc { version: FEATURES_VERSION, features: features.to_vec() };
    let bytes = serde_json::to_vec_pretty(&doc).map_err(|e| ToolError::internal(e.to_string()))?;
    write_atomic(&path, &bytes)
}

fn unreadable(message: String) -> ToolError {
    ToolError::new(ErrorCode::FileError, message).with_remediation(format!("Fix or remove {FEATURES_FILE} in the project's inbox folder."))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report_of(name: &str, state: FeatureState) -> Report {
        Report { name: name.into(), state, guide: None, note: None, similarity: None }
    }

    #[test]
    fn state_ids_are_the_wire_names() {
        for s in [FeatureState::Planned, FeatureState::InProgress, FeatureState::ReadyToTest, FeatureState::Passed, FeatureState::Broken] {
            assert_eq!(serde_json::to_value(s).unwrap(), serde_json::json!(s.id()));
        }
    }

    #[test]
    fn a_missing_record_is_no_features_and_a_report_adds_one() {
        let d = tempfile::tempdir().unwrap();
        assert!(load(d.path()).unwrap().is_empty());
        report(d.path(), report_of("Title screen", FeatureState::Planned)).unwrap();
        let all = load(d.path()).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!((all[0].name.as_str(), all[0].state), ("Title screen", FeatureState::Planned));
    }

    #[test]
    fn the_same_name_updates_and_keeps_what_was_not_given() {
        let d = tempfile::tempdir().unwrap();
        report(d.path(), Report { note: Some("waiting on art".into()), similarity: Some(60), ..report_of("Map", FeatureState::Planned) }).unwrap();
        report(d.path(), report_of("Map", FeatureState::InProgress)).unwrap();
        let all = load(d.path()).unwrap();
        assert_eq!(all.len(), 1, "one record per name");
        assert_eq!(all[0].state, FeatureState::InProgress);
        assert_eq!(all[0].note.as_deref(), Some("waiting on art"), "an omitted note is kept");
        assert_eq!(all[0].similarity, Some(60), "an omitted similarity is kept");
    }

    #[test]
    fn refused_reports_change_nothing() {
        let d = tempfile::tempdir().unwrap();
        report(d.path(), report_of("Map", FeatureState::Planned)).unwrap();
        let before = load(d.path()).unwrap();
        assert!(report(d.path(), Report { similarity: Some(101), ..report_of("Map", FeatureState::Passed) }).is_err());
        assert!(report(d.path(), report_of("   ", FeatureState::Passed)).is_err());
        assert!(report(d.path(), report_of(&"x".repeat(MAX_NAME_CHARS + 1), FeatureState::Passed)).is_err());
        assert!(report(d.path(), report_of("Map\nNew", FeatureState::Passed)).is_err());
        assert!(report(d.path(), report_of("Map", FeatureState::ReadyToTest)).is_err(), "ready_to_test needs a guide");
        assert_eq!(load(d.path()).unwrap(), before);
    }

    #[test]
    fn a_full_project_refuses_a_new_feature_but_still_updates_one() {
        let d = tempfile::tempdir().unwrap();
        for i in 0..MAX_FEATURES {
            report(d.path(), report_of(&format!("F{i}"), FeatureState::Planned)).unwrap();
        }
        let err = report(d.path(), report_of("one more", FeatureState::Planned)).unwrap_err();
        assert_eq!(err.code, ErrorCode::OverLimit);
        assert!(report(d.path(), report_of("F0", FeatureState::Passed)).is_ok());
    }

    #[test]
    fn a_corrupt_or_newer_record_is_an_error_not_an_empty_list() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("inbox")).unwrap();
        std::fs::write(d.path().join(FEATURES_FILE), "{ not json").unwrap();
        assert!(load(d.path()).is_err());
        std::fs::write(d.path().join(FEATURES_FILE), r#"{"version": 99, "features": []}"#).unwrap();
        let err = load(d.path()).unwrap_err();
        assert!(err.message.contains("newer"), "{err:?}");
        assert!(report(d.path(), report_of("Map", FeatureState::Planned)).is_err(), "a report doesn't overwrite what it can't read");
    }

    #[test]
    fn a_run_hears_only_what_changed() {
        let mut seen = HashMap::new();
        let first = vec![Feature { name: "A".into(), state: FeatureState::Planned, guide: None, note: None, similarity: None }];
        assert_eq!(changed(&first, &mut seen).len(), 1, "a new feature is reported");
        assert!(changed(&first, &mut seen).is_empty(), "an unchanged one is not");
        let mut second = first.clone();
        second[0].state = FeatureState::Passed;
        second.push(Feature { name: "B".into(), state: FeatureState::Planned, guide: None, note: None, similarity: None });
        let got = changed(&second, &mut seen);
        assert_eq!(got.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["A", "B"]);
    }
}
