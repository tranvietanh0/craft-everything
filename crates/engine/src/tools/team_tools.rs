//! A project's porting team (the roles its builder can hand work to) as tools, so the app, the CLI
//! and an MCP agent all read and change it the same way. Ids follow docs/tool-ids.md.

use super::project_tools::open;
use crate::team::{Role, remove_role, set_role};
use crate::tool::{Ctx, Effects, Requires, ToolOutput, ToolSpec, Toolset, parse_params, schema_of};
use crate::workspace::{Manifest, Workspace};
use craft_anything_evidence::ToolError;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::PathBuf;

pub fn specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            id: "project_team_list",
            title: "List the porting team",
            summary: "A project's porting team: each role's id, name, when the builder should use it, its prompt, model, tools, skills and hooks. `default` is true when the person hasn't changed the team and the default roster applies.",
            beginner: "Shows the roles on a project's team.",
            toolset: Toolset::Projects,
            effects: Effects::READ_ONLY,
            requires: Requires::Nothing,
            input_schema: schema_of::<ProjectParams>,
            run: project_team_list,
        },
        ToolSpec {
            id: "project_team_set_role",
            title: "Add or change a team role",
            summary: "Add a role to a project's team, or replace the role with the same id. With `replaces`, the role with that id is replaced by this one, which may carry a new id (a rename). Ids are file-safe slugs; a bad field or a clash is refused and nothing is saved. Skills and hooks are kept in the roster as the person wrote them.",
            beginner: "Adds or edits a role on a project's team.",
            toolset: Toolset::Projects,
            effects: Effects::WRITES_FILESYSTEM,
            requires: Requires::Nothing,
            input_schema: schema_of::<SetRoleParams>,
            run: project_team_set_role,
        },
        ToolSpec {
            id: "project_team_remove_role",
            title: "Remove a team role",
            summary: "Take a role off a project's team by id. The default roster is kept until the team is changed, so removing from it saves the rest.",
            beginner: "Removes a role from a project's team.",
            toolset: Toolset::Projects,
            effects: Effects::WRITES_FILESYSTEM,
            requires: Requires::Nothing,
            input_schema: schema_of::<RemoveRoleParams>,
            run: project_team_remove_role,
        },
        ToolSpec {
            id: "project_team_reset",
            title: "Reset the porting team",
            summary: "Go back to the default team of five roles (analyst, asset porter, UI rebuilder, logic porter, tester) and forget the person's changes in craft.json.",
            beginner: "Puts a project's team back to the standard one.",
            toolset: Toolset::Projects,
            effects: Effects::WRITES_FILESYSTEM,
            requires: Requires::Nothing,
            input_schema: schema_of::<ProjectParams>,
            run: project_team_reset,
        },
    ]
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectParams {
    /// The project's folder.
    pub project: PathBuf,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetRoleParams {
    /// The project's folder.
    pub project: PathBuf,
    /// The role to add, or to replace by its id.
    pub role: Role,
    /// The id of the role this one replaces, when its id differs (a rename); none replaces by id.
    #[serde(default)]
    pub replaces: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoveRoleParams {
    /// The project's folder.
    pub project: PathBuf,
    /// The id of the role to remove (from project_team_list).
    pub id: String,
}

fn project_team_list(_ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: ProjectParams = parse_params(params)?;
    let ws = open(&p.project)?;
    Ok(ToolOutput::Value(roster_json(&ws)))
}

fn project_team_set_role(_ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: SetRoleParams = parse_params(params)?;
    let mut ws = open(&p.project)?;
    let (role, replaces) = (p.role, p.replaces);
    let mut outcome = Ok(());
    ws.set_person(|m: &mut Manifest| {
        let mut roster = m.roster();
        outcome = set_role(&mut roster, role, replaces.as_deref());
        if outcome.is_ok() {
            m.team = Some(roster);
        }
    })?;
    outcome?;
    Ok(ToolOutput::Value(roster_json(&ws)))
}

fn project_team_remove_role(_ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: RemoveRoleParams = parse_params(params)?;
    let mut ws = open(&p.project)?;
    let mut outcome = Ok(());
    ws.set_person(|m: &mut Manifest| {
        let mut roster = m.roster();
        outcome = remove_role(&mut roster, &p.id);
        if outcome.is_ok() {
            m.team = Some(roster);
        }
    })?;
    outcome?;
    Ok(ToolOutput::Value(roster_json(&ws)))
}

fn project_team_reset(_ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: ProjectParams = parse_params(params)?;
    let mut ws = open(&p.project)?;
    ws.set_person(|m: &mut Manifest| m.team = None)?;
    Ok(ToolOutput::Value(roster_json(&ws)))
}

fn roster_json(ws: &Workspace) -> Value {
    json!({
        "project": ws.root,
        "default": ws.manifest.team.is_none(),
        "roles": ws.manifest.roster(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::{MANIFEST, TargetRef};
    use crate::{CallOptions, Caller, Engine};

    fn call(e: &Engine, tool: &str, p: Value) -> Result<Value, ToolError> {
        e.call(tool, p, CallOptions::new(Caller::Cli)).map(|o| o.to_json()["result"].clone())
    }

    fn role_json(id: &str) -> Value {
        json!({"id": id, "name": "Reviewer", "description": "Reviews the port.", "prompt": "Review the port."})
    }

    #[test]
    fn the_team_is_listed_edited_and_reset_through_tools() {
        let d = tempfile::tempdir().unwrap();
        let ws = Workspace::create(d.path(), "Game", "A game", TargetRef::None).unwrap();
        let e = Engine::new(crate::testing::session());
        let root = json!(ws.root);

        let before = call(&e, "project_team_list", json!({"project": root})).unwrap();
        assert_eq!((before["default"].clone(), before["roles"].as_array().map(Vec::len)), (json!(true), Some(5)));

        let added = call(&e, "project_team_set_role", json!({"project": root, "role": role_json("reviewer")})).unwrap();
        assert_eq!((added["default"].clone(), added["roles"].as_array().map(Vec::len)), (json!(false), Some(6)));
        let saved = std::fs::read_to_string(ws.root.join(MANIFEST)).unwrap();
        assert!(saved.contains("\"reviewer\""), "the change is saved in craft.json");

        let renamed = json!({"project": root, "replaces": "reviewer", "role": role_json("auditor")});
        call(&e, "project_team_set_role", renamed).unwrap();
        let ids = call(&e, "project_team_list", json!({"project": root})).unwrap();
        assert!(ids["roles"].as_array().unwrap().iter().any(|r| r["id"] == "auditor"));

        call(&e, "project_team_remove_role", json!({"project": root, "id": "auditor"})).unwrap();
        call(&e, "project_team_reset", json!({"project": root})).unwrap();
        let back = call(&e, "project_team_list", json!({"project": root})).unwrap();
        assert_eq!(back["default"], json!(true));
        assert!(!std::fs::read_to_string(ws.root.join(MANIFEST)).unwrap().contains("\"team\""));
    }

    #[test]
    fn a_refused_role_changes_nothing_on_disk() {
        let d = tempfile::tempdir().unwrap();
        let ws = Workspace::create(d.path(), "Game", "A game", TargetRef::None).unwrap();
        let e = Engine::new(crate::testing::session());
        let root = json!(ws.root);
        let bad = json!({"project": root, "role": role_json("Not A Slug")});
        assert!(call(&e, "project_team_set_role", bad).is_err());
        assert!(call(&e, "project_team_remove_role", json!({"project": root, "id": "nobody"})).is_err());
        assert!(!std::fs::read_to_string(ws.root.join(MANIFEST)).unwrap().contains("\"team\""));
    }
}
