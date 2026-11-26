//! The live team against the project's roster: which subagents are working as which role, and
//! which skills a role could preload.

use super::{AgentNode, Team};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// A session's subagents sorted by the role each worked as.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RoleMatch {
    /// The subagents started as each role, by role id, in the order they started.
    pub by_role: BTreeMap<String, Vec<AgentNode>>,
    /// Subagents that aren't a role on the team (built-in helpers such as `Explore`, or a role
    /// that was renamed or removed since).
    pub others: Vec<AgentNode>,
}

/// Sort every subagent below the builder, at any depth, by role: a subagent works as a role when
/// its `subagent_type` is the role's id (the name its subagent file gives it).
pub fn by_role(team: &Team, role_ids: &[&str]) -> RoleMatch {
    let mut out = RoleMatch::default();
    let mut stack: Vec<&AgentNode> = team.builder.children.iter().rev().collect();
    while let Some(node) = stack.pop() {
        match role_ids.iter().find(|id| **id == node.kind) {
            Some(id) => out.by_role.entry((*id).to_string()).or_default().push(node.clone()),
            None => out.others.push(node.clone()),
        }
        stack.extend(node.children.iter().rev());
    }
    out
}

/// The skill folders a role could preload: those in Claude Code's own skills folder and in the
/// project's `.claude/skills`, by folder name, sorted. A folder that can't be read has none.
pub fn skill_names(project: &Path) -> Vec<String> {
    let mut dirs = vec![project.join(".claude").join("skills")];
    if let Some(home) = crate::cli_agents::adapters::claude_config_dir() {
        dirs.push(home.join("skills"));
    }
    let mut names = BTreeSet::new();
    for dir in dirs {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let usable = name.bytes().all(|c| c.is_ascii_alphanumeric() || b"-._".contains(&c)) && !name.starts_with('.');
            if usable && entry.path().is_dir() {
                names.insert(name);
            }
        }
    }
    names.into_iter().collect()
}
