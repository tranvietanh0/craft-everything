//! The project's porting team as Claude Code subagent files: one `.claude/agents/<role id>.md` per
//! role in the build folder, written when a Claude Code builder starts in Deep mode. Fields are the
//! ones Claude Code documents for subagent frontmatter (code.claude.com/docs/en/sub-agents).
//!
//! A file Craft Anything wrote carries [`MARKER`] as the first line of its frontmatter, so a later
//! write replaces it and removes it once its role leaves the team. A file without the marker is the
//! person's: it is never changed or removed, and a role with the same id is left to it.

use craft_anything_engine::team::Role;
use craft_anything_evidence::ToolError;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Where Claude Code looks for a project's subagents, from the project folder.
pub const AGENTS_DIR: &str = ".claude/agents";
/// The line that marks a file Craft Anything wrote (a YAML comment, which Claude Code ignores).
pub const MARKER: &str = "# Written by Craft Anything from this project's team. Edit the team in the app: changes made here are replaced.";

/// Who a subagent file belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileOwner {
    /// Craft Anything wrote it from the team.
    Craft,
    /// The person's own: Craft Anything leaves it alone.
    Person,
}

/// What one write did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Written {
    /// Roles the builder can hand work to now, by id: written by Craft Anything, or the person's
    /// own file with that name.
    pub available: Vec<String>,
    /// Roles whose file is the person's own, left as it is.
    pub kept: Vec<String>,
    /// Craft Anything's files for roles no longer on the team, removed.
    pub removed: Vec<String>,
}

/// The folder the subagent files go in.
pub fn agents_dir(project: &Path) -> PathBuf {
    project.join(AGENTS_DIR)
}

/// Write `roster` into the project's `.claude/agents`, and remove Craft Anything's files for roles
/// that left it. The person's own files are never touched.
pub fn write(project: &Path, roster: &[Role]) -> Result<Written, ToolError> {
    let dir = agents_dir(project);
    std::fs::create_dir_all(&dir).map_err(|e| ToolError::io(dir.display(), e))?;
    let owners = owners(project)?;
    let mut out = Written::default();
    for role in roster {
        let path = dir.join(format!("{}.md", role.id));
        if owners.get(&role.id) == Some(&FileOwner::Person) {
            out.kept.push(role.id.clone());
        } else {
            let text = render(role);
            if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
                std::fs::write(&path, text).map_err(|e| ToolError::io(path.display(), e))?;
            }
        }
        out.available.push(role.id.clone());
    }
    let ids: BTreeSet<&str> = roster.iter().map(|r| r.id.as_str()).collect();
    for (id, owner) in &owners {
        if *owner == FileOwner::Craft && !ids.contains(id.as_str()) {
            let path = dir.join(format!("{id}.md"));
            std::fs::remove_file(&path).map_err(|e| ToolError::io(path.display(), e))?;
            out.removed.push(id.clone());
        }
    }
    Ok(out)
}

/// The `.md` files directly in the project's `.claude/agents`, by file name without `.md`, and
/// whose each is. No folder: none.
pub fn owners(project: &Path) -> Result<BTreeMap<String, FileOwner>, ToolError> {
    let dir = agents_dir(project);
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(ToolError::io(dir.display(), e)),
    };
    let mut out = BTreeMap::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(id) = path.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".md")) else {
            continue;
        };
        if !path.is_file() {
            continue;
        }
        let text = std::fs::read_to_string(&path).map_err(|e| ToolError::io(path.display(), e))?;
        out.insert(id.to_string(), if is_craft_file(&text) { FileOwner::Craft } else { FileOwner::Person });
    }
    Ok(out)
}

/// Whether `text` is a file Craft Anything wrote: frontmatter whose first line is [`MARKER`].
fn is_craft_file(text: &str) -> bool {
    let mut lines = text.lines();
    lines.next().map(str::trim_end) == Some("---") && lines.next().map(str::trim_end) == Some(MARKER)
}

/// One role as a subagent file: frontmatter, then the prompt as the subagent's system prompt.
/// Values are written as JSON strings, which YAML reads as double-quoted scalars.
pub fn render(role: &Role) -> String {
    let q = |s: &str| serde_json::Value::from(s).to_string();
    let list = |items: &[String]| format!("[{}]", items.iter().map(|s| q(s)).collect::<Vec<_>>().join(", "));
    let mut out = format!("---\n{MARKER}\nname: {}\ndescription: {}\n", q(&role.id), q(&role.description));
    if let Some(model) = role.model.as_deref().filter(|m| !m.trim().is_empty()) {
        out.push_str(&format!("model: {}\n", q(model)));
    }
    // An empty list would leave the subagent no tools at all; none means the default set.
    if let Some(tools) = role.tools.as_deref().filter(|t| !t.is_empty()) {
        out.push_str(&format!("tools: {}\n", list(tools)));
    }
    if !role.skills.is_empty() {
        out.push_str(&format!("skills: {}\n", list(&role.skills)));
    }
    if !role.hooks.is_empty() {
        out.push_str("hooks:\n");
        let mut events: BTreeMap<&str, Vec<&craft_anything_engine::team::Hook>> = BTreeMap::new();
        for hook in &role.hooks {
            events.entry(hook.event.as_str()).or_default().push(hook);
        }
        for (event, hooks) in events {
            out.push_str(&format!("  {event}:\n"));
            for hook in hooks {
                if hook.matcher.is_empty() {
                    out.push_str("    - hooks:\n");
                } else {
                    out.push_str(&format!("    - matcher: {}\n      hooks:\n", q(&hook.matcher)));
                }
                out.push_str(&format!("        - type: command\n          command: {}\n", q(&hook.command)));
            }
        }
    }
    out.push_str("---\n\n");
    out.push_str(role.prompt.trim_end());
    out.push('\n');
    out
}

/// What the builder is told about its team: which subagents it has and to hand work to them with
/// the `Agent` tool. Only the roles in `available` are listed.
pub fn preamble(roster: &[Role], available: &[String]) -> String {
    let roles: Vec<String> = roster.iter().filter(|r| available.contains(&r.id)).map(|r| format!("- {} ({}): {}", r.id, r.name, r.description)).collect();
    if roles.is_empty() {
        return String::new();
    }
    format!(
        "You lead a porting team. Each role below is a subagent set up for this project. Hand each piece of work to the role it fits with the Agent tool, `subagent_type` set to the role's id, and give it what it needs to know in the prompt. Keep the plan, the checks and the reports to the person yourself.\n{}\n\n",
        roles.join("\n")
    )
}
