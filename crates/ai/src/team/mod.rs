//! The team behind a build: the builder (Claude Code), the subagents it started with its `Agent`
//! tool (and theirs, nested), and the tools each one called, read from Claude Code's transcripts.
//!
//! Where they are (Claude Code 2.1, as found on disk):
//! - the session: `<claude home>/projects/<folder>/<session>.jsonl`;
//! - each subagent: `<…>/<session>/subagents/agent-<agentId>.jsonl` (every entry `isSidechain`)
//!   beside `agent-<agentId>.meta.json` (`agentType`, `description`, `toolUseId`, `model`,
//!   `finished`). `toolUseId` is the `Agent` call that started it, in whichever transcript made it.
//!
//! A start is an `Agent` (or older `Task`) `tool_use` with `subagent_type` and `description`. Its
//! `tool_result` entry carries `toolUseResult`: `status: "completed"` with the totals, or
//! `status: "async_launched"` with the `agentId` for one started in the background, whose end is
//! a `<task-notification>` naming its `<tool-use-id>`.

pub mod agent_files;
pub mod discord;
mod log;
mod roles;
mod watch;

#[cfg(test)]
mod tests;

pub use log::AgentLog;
pub use roles::{RoleMatch, by_role, skill_names};
pub use watch::{Session, TeamWatch, sessions, transcript_dirs};

use chrono::{DateTime, Utc};

/// What an agent is doing, as far as its transcript tells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentState {
    /// Writing, thinking or about to call a tool.
    Running,
    /// A tool call it made has no result yet: the tool is running, or (in herdr) waiting for the
    /// person to approve it. The transcript doesn't tell which.
    InTool(String),
    /// Its turn ended (the builder waits for the next message).
    Idle,
    /// Nothing written for a while and no recorded end: it may have been stopped.
    Quiet,
    Done,
    Failed,
    /// Stopped before it finished.
    Stopped,
}

impl AgentState {
    pub fn is_live(&self) -> bool {
        matches!(self, AgentState::Running | AgentState::InTool(_))
    }
}

/// What kind of tool a call was, for grouping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ToolGroup {
    /// One of Craft Anything's own tools (`mcp__craft-anything__…`).
    Craft,
    /// A skill (`Skill`), named by the skill.
    Skill,
    /// Starting a subagent (`Agent`, `Task`).
    Agent,
    /// Another MCP server's tool.
    Mcp,
    /// Claude Code's own (`Bash`, `Edit`, `Read`…).
    Builtin,
}

/// How many times one tool was called.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCount {
    pub name: String,
    pub group: ToolGroup,
    pub count: u32,
}

/// One agent in the tree.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentNode {
    /// The subagent type (`Explore`, `gk-apk-port-runner`…), or `builder` for the session's own.
    pub kind: String,
    pub description: String,
    pub model: String,
    pub state: AgentState,
    pub started: Option<DateTime<Utc>>,
    /// How long it ran (still running: so far).
    pub duration_ms: Option<u64>,
    /// Tokens it used (prompt, cached and output), when known.
    pub tokens: Option<u64>,
    pub tool_uses: Option<u64>,
    /// The first line of what it answered.
    pub summary: Option<String>,
    /// Calls by tool, most first.
    pub tools: Vec<ToolCount>,
    /// The last few calls, newest last.
    pub recent: Vec<String>,
    pub children: Vec<AgentNode>,
}

impl AgentNode {
    /// This agent and every one below it.
    pub fn count(&self) -> usize {
        1 + self.children.iter().map(AgentNode::count).sum::<usize>()
    }

    /// How many of this agent and those below it are working now.
    pub fn live(&self) -> usize {
        usize::from(self.state.is_live()) + self.children.iter().map(AgentNode::live).sum::<usize>()
    }
}

/// A session's team at one moment.
#[derive(Debug, Clone, PartialEq)]
pub struct Team {
    pub session: String,
    pub builder: AgentNode,
    /// Subagent transcripts that couldn't be read (path and why), shown, not skipped silently.
    pub problems: Vec<String>,
}

/// The time now, to time running agents against.
pub fn now() -> DateTime<Utc> {
    std::time::SystemTime::now().into()
}

/// Parses a transcript timestamp (`2026-11-24T20:08:23.354Z`).
pub(crate) fn timestamp(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text).ok().map(|t| t.with_timezone(&Utc))
}

/// Milliseconds from `a` to `b` (0 when `b` is earlier).
pub(crate) fn millis_between(a: DateTime<Utc>, b: DateTime<Utc>) -> u64 {
    u64::try_from((b - a).num_milliseconds()).unwrap_or(0)
}
