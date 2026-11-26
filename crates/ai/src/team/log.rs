//! One transcript's entries (the session's, or one subagent's) folded into what the team view
//! needs: the tools called, the subagents started and how each ended, and how the agent stands.

use super::{ToolGroup, timestamp};
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, VecDeque};

/// How many recent calls are kept per agent.
const RECENT: usize = 5;
/// Longest summary kept, in characters.
const SUMMARY_CHARS: usize = 160;
const CRAFT_PREFIX: &str = "mcp__craft-anything__";

/// How a started subagent ended, as its starter's transcript tells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnEnd {
    Running,
    Done,
    Failed,
    Stopped,
}

/// A subagent started by an `Agent` call.
#[derive(Debug, Clone, PartialEq)]
pub struct Spawn {
    pub tool_use_id: String,
    pub agent_type: String,
    pub description: String,
    pub started: Option<DateTime<Utc>>,
    pub agent_id: Option<String>,
    pub model: Option<String>,
    pub end: SpawnEnd,
    pub duration_ms: Option<u64>,
    pub tokens: Option<u64>,
    pub tool_uses: Option<u64>,
    pub summary: Option<String>,
}

/// What one transcript said so far. Feed it lines in order with [`AgentLog::line`].
#[derive(Debug, Default)]
pub struct AgentLog {
    /// Entries marked `isSidechain` belong here (a subagent's own file) rather than to someone else.
    sidechain: bool,
    pub model: String,
    pub first: Option<DateTime<Utc>>,
    pub last: Option<DateTime<Utc>>,
    pub tools: BTreeMap<String, (ToolGroup, u32)>,
    pub recent: VecDeque<String>,
    /// Calls with no result yet, by call id.
    pub pending: BTreeMap<String, String>,
    pub spawns: Vec<Spawn>,
    by_call: HashMap<String, usize>,
    /// Its turn ended (`turn_duration`) after its last model message.
    pub turn_ended: bool,
    /// Tokens per model message id (a message written as several entries repeats its usage).
    usage: HashMap<String, u64>,
}

impl AgentLog {
    /// The log of a session (`sidechain` false) or of a subagent's own transcript (true).
    pub fn new(sidechain: bool) -> Self {
        AgentLog { sidechain, ..AgentLog::default() }
    }

    pub fn tokens(&self) -> Option<u64> {
        (!self.usage.is_empty()).then(|| self.usage.values().sum())
    }

    /// One transcript line. Lines that aren't JSON, or aren't about this agent, change nothing.
    pub fn line(&mut self, line: &str) {
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
            return;
        };
        if !self.sidechain && v.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            return;
        }
        if let Some(t) = v.get("timestamp").and_then(Value::as_str).and_then(timestamp) {
            self.first.get_or_insert(t);
            self.last = Some(t);
        }
        let at = self.last;
        match v.get("type").and_then(Value::as_str).unwrap_or_default() {
            "assistant" => self.assistant(v.get("message").unwrap_or(&Value::Null), at),
            "user" if v.get("isMeta").and_then(Value::as_bool) != Some(true) => self.user(&v),
            // A background subagent's end is queued for the agent before it's shown to it.
            "queue-operation" if v.get("operation").and_then(Value::as_str) == Some("enqueue") => {
                if let Some(text) = v.get("content").and_then(Value::as_str) {
                    self.notification(text);
                }
            }
            "system" if v.get("subtype").and_then(Value::as_str) == Some("turn_duration") => self.turn_ended = true,
            _ => {}
        }
    }

    fn assistant(&mut self, message: &Value, at: Option<DateTime<Utc>>) {
        self.turn_ended = false;
        if let Some(model) = message.get("model").and_then(Value::as_str).filter(|m| !m.is_empty() && !m.starts_with('<')) {
            self.model = model.to_string();
        }
        if let (Some(id), Some(u)) = (message.get("id").and_then(Value::as_str), message.get("usage")) {
            let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
            let total = n("input_tokens") + n("cache_read_input_tokens") + n("cache_creation_input_tokens") + n("output_tokens");
            self.usage.insert(id.to_string(), total);
        }
        for block in message.get("content").and_then(Value::as_array).into_iter().flatten() {
            if block.get("type").and_then(Value::as_str) == Some("tool_use") {
                self.tool_use(block, at);
            }
        }
    }

    fn tool_use(&mut self, block: &Value, at: Option<DateTime<Utc>>) {
        let id = block.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
        let name = block.get("name").and_then(Value::as_str).unwrap_or_default();
        let input = block.get("input").unwrap_or(&Value::Null);
        let field = |k: &str| input.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
        let (shown, group) = match name {
            "Agent" | "Task" => (name.to_string(), ToolGroup::Agent),
            "Skill" => (format!("Skill: {}", field("skill")), ToolGroup::Skill),
            n => match n.strip_prefix(CRAFT_PREFIX) {
                Some(tool) => (tool.to_string(), ToolGroup::Craft),
                None if n.starts_with("mcp__") => (n.to_string(), ToolGroup::Mcp),
                None => (n.to_string(), ToolGroup::Builtin),
            },
        };
        if group == ToolGroup::Agent && !id.is_empty() {
            self.by_call.insert(id.clone(), self.spawns.len());
            self.spawns.push(Spawn {
                tool_use_id: id.clone(),
                agent_type: Some(field("subagent_type")).filter(|t| !t.is_empty()).unwrap_or_else(|| "general-purpose".into()),
                description: field("description"),
                started: at,
                agent_id: None,
                model: Some(field("model")).filter(|m| !m.is_empty()),
                end: SpawnEnd::Running,
                duration_ms: None,
                tokens: None,
                tool_uses: None,
                summary: None,
            });
        }
        self.tools.entry(shown.clone()).or_insert((group, 0)).1 += 1;
        if self.recent.len() == RECENT {
            self.recent.pop_front();
        }
        self.recent.push_back(shown.clone());
        if !id.is_empty() {
            self.pending.insert(id, shown);
        }
    }

    fn user(&mut self, v: &Value) {
        let content = v.pointer("/message/content").unwrap_or(&Value::Null);
        if let Some(text) = content.as_str() {
            self.notification(text);
            return;
        }
        for block in content.as_array().into_iter().flatten() {
            match block.get("type").and_then(Value::as_str) {
                Some("tool_result") => {
                    let id = block.get("tool_use_id").and_then(Value::as_str).unwrap_or_default();
                    self.pending.remove(id);
                    let failed = block.get("is_error").and_then(Value::as_bool) == Some(true);
                    let text = crate::cli_agents::content_text(block.get("content").unwrap_or(&Value::Null));
                    self.spawn_result(id, v.get("toolUseResult"), failed, &text);
                }
                Some("text") => self.notification(block.get("text").and_then(Value::as_str).unwrap_or_default()),
                _ => {}
            }
        }
    }

    /// The `tool_result` of an `Agent` call: how it ended, or that it went to the background.
    fn spawn_result(&mut self, id: &str, result: Option<&Value>, failed: bool, text: &str) {
        let Some(&i) = self.by_call.get(id) else {
            return;
        };
        let s = &mut self.spawns[i];
        let r = result.filter(|r| r.is_object()).unwrap_or(&Value::Null);
        let num = |k: &str| r.get(k).and_then(Value::as_u64);
        if let Some(agent) = r.get("agentId").and_then(Value::as_str) {
            s.agent_id = Some(agent.to_string());
        }
        if let Some(model) = r.get("resolvedModel").and_then(Value::as_str) {
            s.model = Some(model.to_string());
        }
        let status = r.get("status").and_then(Value::as_str).unwrap_or_default();
        if status == "async_launched" {
            return;
        }
        s.end = if failed || status == "failed" { SpawnEnd::Failed } else { SpawnEnd::Done };
        s.duration_ms = num("totalDurationMs").or(s.duration_ms);
        s.tokens = num("totalTokens").or(s.tokens);
        s.tool_uses = num("totalToolUseCount").or(s.tool_uses);
        let answer = r.get("content").map(crate::cli_agents::content_text).filter(|t| !t.trim().is_empty());
        s.summary = summary(answer.as_deref().unwrap_or(text)).or(s.summary.take());
    }

    /// A `<task-notification>` that a background subagent ended.
    fn notification(&mut self, text: &str) {
        if !text.trim_start().starts_with("<task-notification>") {
            return;
        }
        let Some(&i) = tag(text, "tool-use-id").and_then(|id| self.by_call.get(id)) else {
            return;
        };
        let s = &mut self.spawns[i];
        let num = |t: &str| tag(text, t).and_then(|n| n.trim().parse::<u64>().ok());
        s.end = match tag(text, "status").unwrap_or_default().trim() {
            "completed" => SpawnEnd::Done,
            "failed" => SpawnEnd::Failed,
            "killed" | "stopped" | "cancelled" => SpawnEnd::Stopped,
            _ => s.end,
        };
        s.duration_ms = num("duration_ms").or(s.duration_ms);
        s.tokens = num("subagent_tokens").or_else(|| num("total_tokens")).or(s.tokens);
        s.tool_uses = num("tool_uses").or(s.tool_uses);
        if let Some(text) = tag(text, "summary").and_then(summary) {
            s.summary = Some(text);
        }
    }
}

/// The text inside the first `<name>…</name>`.
fn tag<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let start = text.find(&open)? + open.len();
    let end = text[start..].find(&format!("</{name}>")).map_or(text.len(), |e| start + e);
    Some(&text[start..end])
}

/// The first line that isn't empty, shortened.
fn summary(text: &str) -> Option<String> {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    let mut out: String = line.chars().take(SUMMARY_CHARS).collect();
    if line.chars().count() > SUMMARY_CHARS {
        out.push('…');
    }
    Some(out)
}
