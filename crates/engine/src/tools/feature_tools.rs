//! The builder's feature progress: one record per feature of the port, kept in the project's inbox
//! (`workspace::features`). A rebuild run reads the records after each report and tells the
//! person what changed (`RunEvent::Feature`). The project is the one being built, as for the
//! other build tools.

use crate::inbox::InboxDir;
use crate::tool::{Ctx, Effects, Requires, ToolOutput, ToolSpec, Toolset, parse_params, schema_of};
use crate::tools::build_tools::{in_project, workspace};
use crate::workspace::Workspace;
use crate::workspace::features::{self, FeatureState, Report};
use craft_anything_evidence::ToolError;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

/// The report tool's id. A run compares tool names with it (`Registry::canonical`), so the run
/// reads the records only after this tool succeeds.
pub const FEATURE_REPORT_TOOL: &str = "build_feature_report";
pub const FEATURE_LIST_TOOL: &str = "build_features_list";

pub fn specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            id: FEATURE_REPORT_TOOL,
            title: "Report a feature",
            summary: "Report one feature of the port and where it stands: planned, in_progress, ready_to_test (with its guide, a project file such as tests/<feature>.md), passed or broken. The same name updates the feature; an omitted guide, note or similarity keeps the stored one. A ready_to_test feature needs an existing guide inside the project.",
            beginner: "Shows how each part of the build is going.",
            toolset: Toolset::Build,
            effects: Effects::WRITES_FILESYSTEM,
            requires: Requires::Nothing,
            input_schema: schema_of::<ReportParams>,
            run: build_feature_report,
        },
        ToolSpec {
            id: FEATURE_LIST_TOOL,
            title: "List the features",
            summary: "List every feature reported for the project being built, in the order first reported: name, state, guide, note and similarity.",
            beginner: "Shows the parts of the build reported so far.",
            toolset: Toolset::Build,
            effects: Effects::READ_ONLY,
            requires: Requires::Nothing,
            input_schema: schema_of::<ListParams>,
            run: build_features_list,
        },
    ]
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReportParams {
    /// The feature's name, one line (for example "Title screen"). The same name updates it.
    pub name: String,
    /// `planned`, `in_progress`, `ready_to_test`, `passed` or `broken`.
    pub state: FeatureState,
    /// The guide a tester follows, a path inside the project such as `tests/title-screen.md`.
    /// Required for `ready_to_test`.
    #[serde(default)]
    pub guide: Option<String>,
    /// A short note for the person: what is left, or what broke.
    #[serde(default)]
    pub note: Option<String>,
    /// How close the build looks to the original, 0 to 100.
    #[serde(default)]
    pub similarity: Option<u8>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListParams {}

fn build_feature_report(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: ReportParams = parse_params(params)?;
    let ws = workspace(ctx)?;
    let guide = p.guide.map(|g| guide_in_project(&ws, &g)).transpose()?;
    let feature = features::report(&ws.root, Report { name: p.name, state: p.state, guide, note: p.note, similarity: p.similarity })?;
    Ok(ToolOutput::Value(json!({"project": ws.root, "feature": feature})))
}

fn build_features_list(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let _: ListParams = parse_params(params)?;
    let ws = workspace(ctx)?;
    let all = features::load(&ws.root)?;
    Ok(ToolOutput::Value(json!({"project": ws.root, "features": all})))
}

/// A guide must be a file inside the project (`in_project` refuses `..`, absolute paths and links
/// out of it); one that doesn't exist yet can't be told to a tester.
fn guide_in_project(ws: &Workspace, guide: &str) -> Result<String, ToolError> {
    let rel = guide.trim();
    let path = in_project(ws, rel)?;
    if !path.is_file() {
        return Err(ToolError::invalid(format!("The guide {rel} isn't a file in the project."))
            .with_remediation("Write the guide first, then report the feature with its path, such as tests/<feature>.md."));
    }
    Ok(rel.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::TargetRef;
    use crate::{CallOptions, Caller, Consent, Engine};
    use craft_anything_evidence::ErrorCode;
    use std::sync::Arc;

    /// A project being built: the session holds its inbox, as a rebuild run attaches it.
    fn building() -> (tempfile::TempDir, Workspace, Engine) {
        let d = tempfile::tempdir().unwrap();
        let mut s = crate::testing::session();
        let ws = Workspace::create(d.path(), "Snake", "A snake game", TargetRef::None).unwrap();
        s.ext.insert(Arc::new(InboxDir(ws.inbox_dir())));
        (d, ws, Engine::new(s))
    }

    fn call(e: &Engine, id: &str, p: Value) -> Result<Value, ToolError> {
        e.call(id, p, CallOptions::new(Caller::Mcp).with_consent(Consent::Granted)).map(|o| o.to_json()["result"].clone())
    }

    #[test]
    fn a_report_is_read_back_and_a_second_one_updates_it() {
        let (_d, _ws, e) = building();
        call(&e, FEATURE_REPORT_TOOL, json!({"name": "Title screen", "state": "planned", "note": "art first"})).unwrap();
        call(&e, FEATURE_REPORT_TOOL, json!({"name": "Title screen", "state": "in_progress"})).unwrap();
        let list = call(&e, FEATURE_LIST_TOOL, json!({})).unwrap();
        let all = list["features"].as_array().unwrap();
        assert_eq!(all.len(), 1, "same name updates, not duplicates: {list}");
        assert_eq!(all[0]["state"], "in_progress");
        assert_eq!(all[0]["note"], "art first", "an omitted note is kept");
    }

    #[test]
    fn an_invalid_state_is_refused_with_a_clear_error() {
        let (_d, ws, e) = building();
        let err = call(&e, FEATURE_REPORT_TOOL, json!({"name": "Map", "state": "maybe"})).unwrap_err();
        assert_eq!(err.code, ErrorCode::BadInput, "{err:?}");
        assert!(err.message.contains("state") || err.message.contains("maybe"), "{err:?}");
        assert!(!ws.root.join(features::FEATURES_FILE).exists(), "nothing was written");
    }

    #[test]
    fn a_guide_outside_the_project_is_refused() {
        let (d, _ws, e) = building();
        let outside = d.path().join("elsewhere.md");
        std::fs::write(&outside, "# guide").unwrap();
        for guide in ["../elsewhere.md", outside.to_str().unwrap()] {
            let err = call(&e, FEATURE_REPORT_TOOL, json!({"name": "Map", "state": "planned", "guide": guide})).unwrap_err();
            assert_eq!(err.code, ErrorCode::BadInput, "{guide}: {err:?}");
        }
    }

    #[test]
    fn a_guide_must_exist_and_ready_to_test_needs_one() {
        let (_d, ws, e) = building();
        let missing = call(&e, FEATURE_REPORT_TOOL, json!({"name": "Map", "state": "planned", "guide": "tests/map.md"}));
        assert!(missing.is_err(), "a guide that isn't written yet is refused");
        assert!(call(&e, FEATURE_REPORT_TOOL, json!({"name": "Map", "state": "ready_to_test"})).is_err(), "ready_to_test without a guide");
        std::fs::create_dir_all(ws.root.join("tests")).unwrap();
        std::fs::write(ws.root.join("tests/map.md"), "# Map\n").unwrap();
        let ok = call(&e, FEATURE_REPORT_TOOL, json!({"name": "Map", "state": "ready_to_test", "guide": "tests/map.md"})).unwrap();
        assert_eq!(ok["feature"]["guide"], "tests/map.md");
    }

    #[test]
    fn the_feature_tools_work_in_clean_room() {
        assert!(crate::clean_room::allows(FEATURE_REPORT_TOOL));
        assert!(crate::clean_room::allows(FEATURE_LIST_TOOL));
    }
}
