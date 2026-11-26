//! The porting team: the roles a project's builder can hand work to. Each role becomes a Claude
//! Code subagent when a build starts (that wiring lives in the AI crate). The roster is the
//! person's, kept in the project's `craft.json` as `team`. Until the person changes it the default
//! roster applies, and nothing is written to disk.

use crate::workspace::Manifest;
use craft_anything_evidence::{ErrorCode, ToolError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests;

/// Most roles one project's team can have.
pub const MAX_ROLES: usize = 12;
const MAX_ID_LEN: usize = 40;
const MAX_NAME_LEN: usize = 60;
const MAX_DESCRIPTION_LEN: usize = 400;
const MAX_PROMPT_LEN: usize = 8_000;
const MAX_MODEL_LEN: usize = 80;
const MAX_TOOL_LEN: usize = 80;
const MAX_SKILL_LEN: usize = 80;
const MAX_COMMAND_LEN: usize = 1_000;

/// One role on the porting team.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Role {
    /// File-safe id: lower-case letters, digits and single hyphens, starting with a letter. It is
    /// the subagent's name and what a transcript's `subagent_type` is matched against.
    pub id: String,
    /// The name people see. Renaming it keeps the id.
    pub name: String,
    /// When the builder should hand work to this role.
    pub description: String,
    /// The role's instructions (the subagent's system prompt).
    pub prompt: String,
    /// The model the role runs on (`sonnet`, `opus`, `haiku`, `inherit` or a full model id); none
    /// means the builder's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The tools the role may use; none means the default set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
    /// Skill folder names the role uses.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    /// Hooks that run while the role works.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hooks: Vec<Hook>,
}

/// A hook a role runs: an event, the tool calls it applies to, and the command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Hook {
    /// The event (`PreToolUse`, `PostToolUse`, `Stop`…).
    pub event: String,
    /// Which tool calls it applies to (`Bash`, `Edit|Write`); empty for every one.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub matcher: String,
    /// The command to run, on one line.
    pub command: String,
}

impl Role {
    /// Refuse a role the builder could not use: a bad id, an empty or oversized field, or a
    /// control character where a line is expected.
    pub fn validate(&self) -> Result<(), ToolError> {
        if !valid_id(&self.id) {
            return Err(invalid(format!(
                "The role id {:?} must be up to {MAX_ID_LEN} lower-case letters, digits and single hyphens, starting with a letter.",
                self.id
            )));
        }
        line("name", &self.name, MAX_NAME_LEN)?;
        line("description", &self.description, MAX_DESCRIPTION_LEN)?;
        text("prompt", &self.prompt, MAX_PROMPT_LEN)?;
        if let Some(model) = &self.model {
            if !model.bytes().all(|c| c.is_ascii_alphanumeric() || b"-._:/[]".contains(&c)) {
                return Err(invalid(format!("The model {model:?} has characters a model name doesn't use.")));
            }
            line("model", model, MAX_MODEL_LEN)?;
        }
        for tool in self.tools.iter().flatten() {
            line("tool", tool, MAX_TOOL_LEN)?;
        }
        for skill in &self.skills {
            if !skill.bytes().all(|c| c.is_ascii_alphanumeric() || b"-._".contains(&c)) {
                return Err(invalid(format!("The skill {skill:?} must be the name of a skill folder.")));
            }
            line("skill", skill, MAX_SKILL_LEN)?;
        }
        for hook in &self.hooks {
            if !hook.event.bytes().all(|c| c.is_ascii_alphabetic()) || hook.event.is_empty() {
                return Err(invalid(format!("The hook event {:?} must be a name such as PreToolUse.", hook.event)));
            }
            line("hook matcher", &hook.matcher, MAX_TOOL_LEN)?;
            line("hook command", &hook.command, MAX_COMMAND_LEN)?;
        }
        Ok(())
    }
}

impl Manifest {
    /// The team's roles: the person's roster, or the default one when none is saved.
    pub fn roster(&self) -> Vec<Role> {
        self.team.clone().unwrap_or_else(default_roster)
    }
}

/// Whether `id` is a valid role id.
pub fn valid_id(id: &str) -> bool {
    let b = id.as_bytes();
    !b.is_empty()
        && b.len() <= MAX_ID_LEN
        && b[0].is_ascii_lowercase()
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
        && !id.ends_with('-')
        && !id.contains("--")
}

/// Add `role`, or replace the role it replaces. Without `replaces` the role with the same id is
/// replaced, or the role is added. With `replaces` the role with that id is replaced by this one,
/// which may carry a new id (a rename), as long as no other role has it.
pub fn set_role(roster: &mut Vec<Role>, role: Role, replaces: Option<&str>) -> Result<(), ToolError> {
    role.validate()?;
    let target = replaces.unwrap_or(&role.id);
    let position = roster.iter().position(|r| r.id == target);
    if replaces.is_some() && position.is_none() {
        return Err(not_found(target));
    }
    if roster.iter().any(|r| r.id == role.id && r.id != target) {
        return Err(invalid(format!("Another role already has the id {}.", role.id)));
    }
    match position {
        Some(i) => roster[i] = role,
        None if roster.len() >= MAX_ROLES => {
            return Err(invalid(format!("A team has at most {MAX_ROLES} roles.")).with_remediation("Remove a role first."));
        }
        None => roster.push(role),
    }
    Ok(())
}

/// Take the role with `id` off the team.
pub fn remove_role(roster: &mut Vec<Role>, id: &str) -> Result<(), ToolError> {
    let position = roster.iter().position(|r| r.id == id).ok_or_else(|| not_found(id))?;
    roster.remove(position);
    Ok(())
}

/// The five roles every new project starts with.
pub fn default_roster() -> Vec<Role> {
    vec![
        role(
            "analyst",
            "Analyst",
            "Reads the original app: its files, its evidence and the person's pictures of it. Writes down what it does and how, so the others build from facts.",
            "You are the analyst on this porting team. Study the original app in the capture and reference folders and record what you find as evidence: screens, flows, data formats, logic and anything the person pictured. Write each finding where the other roles can read it, mark what you could not confirm, and never change the app you are studying. When another role asks about the original, answer from the evidence and say where it came from.",
        ),
        role(
            "asset-porter",
            "Asset porter",
            "Extracts and converts the images, sounds, fonts and models the original ships, into formats the rebuild can load, and records where each one came from.",
            "You are the asset porter. Extract the pictures, audio, fonts and 3D models the original uses, convert them to formats the rebuild loads, and save them in the app's asset folders under names the other roles can use. Keep each file's origin in a short note beside it. Do not rebuild screens or logic. If an asset is missing or unreadable, say so rather than substituting a look-alike without telling the person.",
        ),
        role(
            "ui-rebuilder",
            "UI rebuilder",
            "Rebuilds the screens, layout, navigation and styling so they match the original's look and behaviour, using the assets the asset porter delivers.",
            "You are the UI rebuilder. Build each screen of the original in the app: its layout, controls, navigation and styling, matched against the person's pictures and the analyst's notes. Use the theme and asset names the project already has. Where a screen depends on logic that is not ported yet, leave a clearly marked placeholder and report it instead of faking the behaviour.",
        ),
        role(
            "logic-porter",
            "Logic porter",
            "Ports the game or app logic and its data: rules, state, calculations and saved content, so the rebuilt app behaves like the original.",
            "You are the logic porter. Port the original's rules, state changes, calculations and data tables into the app's code, following the analyst's findings. Keep the data in the shape it has in the original, and write a test for each rule where a test can show it. Report any behaviour you could not confirm from the evidence.",
        ),
        role(
            "tester",
            "Tester",
            "Builds and runs the app, compares it with the original and reports the gaps it finds, with evidence. Hand it the app after each change to check the result.",
            "You are the tester. Build the app, run it, and compare what it does and shows with the original, using the evidence the analyst recorded. Report each gap with the steps that show it and what the original did instead. Do not edit the app's code to make a check pass: report the failure and let the role that owns that code fix it.",
        ),
    ]
}

fn role(id: &str, name: &str, description: &str, prompt: &str) -> Role {
    Role {
        id: id.into(),
        name: name.into(),
        description: description.into(),
        prompt: prompt.into(),
        model: None,
        tools: None,
        skills: Vec::new(),
        hooks: Vec::new(),
    }
}

/// A field that must be one non-empty line of printable text.
fn line(field: &str, value: &str, max: usize) -> Result<(), ToolError> {
    if value.trim().is_empty() || value.chars().count() > max || value.chars().any(char::is_control) {
        return Err(invalid(format!("The {field} must be one line of text, up to {max} characters.")));
    }
    Ok(())
}

/// A field that may run over several lines (a prompt), with a size limit.
fn text(field: &str, value: &str, max: usize) -> Result<(), ToolError> {
    let bad_control = value.chars().any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'));
    if value.trim().is_empty() || value.chars().count() > max || bad_control {
        return Err(invalid(format!("The {field} must be text, up to {max} characters.")));
    }
    Ok(())
}

fn invalid(message: String) -> ToolError {
    ToolError::invalid(message)
}

fn not_found(id: &str) -> ToolError {
    ToolError::new(ErrorCode::NotFound, format!("No role called {id}.")).with_remediation("Use project_team_list for the ids on this team.")
}
