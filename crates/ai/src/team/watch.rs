//! Following a session's transcripts as they grow (the session's own and every subagent's), and
//! putting the agents together as a tree.

use super::log::{AgentLog, Spawn, SpawnEnd};
use super::{AgentNode, AgentState, Team, ToolCount, millis_between};
use crate::cli_agents::herdr::paths::{Host, transcript_dir_name};
use crate::jsonl_tail::JsonlTail;
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// No entry for this long, and no recorded end: the agent is shown as quiet, not as working.
const QUIET_MS: u64 = 5 * 60 * 1000;
/// Subagents started by subagents are followed this deep (a guard against a loop in the files).
const MAX_DEPTH: usize = 8;

/// The folders Claude Code may keep `project`'s sessions in: the one herdr's Claude Code uses (on
/// Windows when this runs in WSL), and this system's own (headless runs).
pub fn transcript_dirs(project: &Path) -> Vec<PathBuf> {
    let host = Host::current();
    let mut dirs = Vec::new();
    if let Some(home) = crate::cli_agents::herdr::claude_home(&host) {
        dirs.push(home.join("projects").join(transcript_dir_name(&host.to_agent(project))));
    }
    if let Some(home) = crate::cli_agents::adapters::claude_config_dir() {
        let dir = home.join("projects").join(transcript_dir_name(&project.display().to_string()));
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs
}

/// One saved conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub id: String,
    pub path: PathBuf,
    pub modified: SystemTime,
}

/// The sessions in `dirs`, newest first. A folder that isn't there has none; one that can't be
/// listed is an error.
pub fn sessions(dirs: &[PathBuf]) -> Result<Vec<Session>, String> {
    let mut out = Vec::new();
    for dir in dirs {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("couldn't list {}: {e}", dir.display())),
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let id = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let modified = entry.metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
            out.push(Session { id, path, modified });
        }
    }
    out.sort_by_key(|s| std::cmp::Reverse(s.modified));
    Ok(out)
}

/// What a subagent's `.meta.json` says.
#[derive(Debug, Default, Clone)]
struct Meta {
    agent_type: String,
    description: String,
    tool_use_id: String,
    model: String,
    finished: bool,
}

impl Meta {
    fn read(path: &Path) -> Result<Meta, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("couldn't read {}: {e}", path.display()))?;
        let v: Value = serde_json::from_slice(&bytes).map_err(|e| format!("{} isn't valid: {e}", path.display()))?;
        let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
        Ok(Meta {
            agent_type: s("agentType"),
            description: s("description"),
            tool_use_id: s("toolUseId"),
            model: s("model"),
            finished: v.get("finished").and_then(Value::as_bool).unwrap_or(false),
        })
    }
}

struct Sub {
    tail: JsonlTail,
    log: AgentLog,
    meta_path: PathBuf,
    meta: Option<Meta>,
    problem: Option<String>,
}

/// A session followed as it grows: call [`TeamWatch::poll`] now and then, then [`TeamWatch::team`].
pub struct TeamWatch {
    session: String,
    main: JsonlTail,
    log: AgentLog,
    subagents_dir: PathBuf,
    subs: BTreeMap<String, Sub>,
}

impl TeamWatch {
    pub fn new(session: &Session) -> Self {
        let subagents_dir = session.path.with_extension("").join("subagents");
        TeamWatch {
            session: session.id.clone(),
            main: JsonlTail::new(session.path.clone(), 0),
            log: AgentLog::new(false),
            subagents_dir,
            subs: BTreeMap::new(),
        }
    }

    /// Reads what was written since the last poll. The session's own transcript not being
    /// readable is an error; a subagent's is noted in [`Team::problems`].
    pub fn poll(&mut self) -> Result<(), String> {
        let lines = self.main.read_lines().map_err(|e| format!("couldn't read {}: {e}", self.main.path().display()))?;
        for line in &lines {
            self.log.line(line);
        }
        if let Ok(entries) = std::fs::read_dir(&self.subagents_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let Some(id) = name.strip_prefix("agent-").and_then(|n| n.strip_suffix(".jsonl")) else {
                    continue;
                };
                self.subs.entry(id.to_string()).or_insert_with(|| Sub {
                    tail: JsonlTail::new(entry.path(), 0),
                    log: AgentLog::new(true),
                    meta_path: self.subagents_dir.join(format!("agent-{id}.meta.json")),
                    meta: None,
                    problem: None,
                });
            }
        }
        for sub in self.subs.values_mut() {
            match sub.tail.read_lines() {
                Ok(lines) => lines.iter().for_each(|l| sub.log.line(l)),
                Err(e) => sub.problem = Some(format!("couldn't read {}: {e}", sub.tail.path().display())),
            }
            // Small, and rewritten when the subagent ends: read until it says it finished.
            if !sub.meta.as_ref().is_some_and(|m| m.finished) && sub.meta_path.exists() {
                match Meta::read(&sub.meta_path) {
                    Ok(m) => sub.meta = Some(m),
                    Err(e) => sub.problem = Some(e),
                }
            }
        }
        Ok(())
    }

    /// The team as read so far, timed against `now`.
    pub fn team(&self, now: DateTime<Utc>) -> Team {
        let mut used = Vec::new();
        let mut builder = self.node_for_log("builder".into(), String::new(), &self.log, None, now);
        builder.children = self.children(&self.log, now, 0, &mut used);
        // Subagents whose start wasn't found (it may be in a part of a transcript not written yet).
        for (id, sub) in &self.subs {
            if !used.contains(id) {
                let meta = sub.meta.clone().unwrap_or_default();
                let mut node = self.node_for_log(meta.agent_type.clone(), meta.description.clone(), &sub.log, Some(&meta), now);
                node.children = self.children(&sub.log, now, 1, &mut used);
                builder.children.push(node);
            }
        }
        let problems = self.subs.values().filter_map(|s| s.problem.clone()).collect();
        Team { session: self.session.clone(), builder, problems }
    }

    fn children(&self, log: &AgentLog, now: DateTime<Utc>, depth: usize, used: &mut Vec<String>) -> Vec<AgentNode> {
        if depth >= MAX_DEPTH {
            return Vec::new();
        }
        log.spawns
            .iter()
            .map(|spawn| {
                let found = self.subs.iter().find(|(id, sub)| {
                    !used.contains(*id)
                        && (spawn.agent_id.as_deref() == Some(id.as_str()) || sub.meta.as_ref().is_some_and(|m| m.tool_use_id == spawn.tool_use_id))
                });
                match found {
                    Some((id, sub)) => {
                        used.push(id.clone());
                        let mut node = spawn_node(spawn, Some(sub), now);
                        node.children = self.children(&sub.log, now, depth + 1, used);
                        node
                    }
                    None => spawn_node(spawn, None, now),
                }
            })
            .collect()
    }

    fn node_for_log(&self, kind: String, description: String, log: &AgentLog, meta: Option<&Meta>, now: DateTime<Utc>) -> AgentNode {
        let finished = meta.is_some_and(|m| m.finished);
        let state = log_state(log, finished, now);
        let duration_ms = log.first.map(|f| millis_between(f, if state.is_live() { now } else { log.last.unwrap_or(f) }));
        AgentNode {
            kind,
            description,
            model: Some(log.model.clone()).filter(|m| !m.is_empty()).or_else(|| meta.map(|m| m.model.clone())).unwrap_or_default(),
            state,
            started: log.first,
            duration_ms,
            tokens: log.tokens(),
            tool_uses: Some(log.tools.values().map(|(_, n)| u64::from(*n)).sum()),
            summary: None,
            tools: tool_counts(log),
            recent: log.recent.iter().cloned().collect(),
            children: Vec::new(),
        }
    }
}

/// How an agent stands from its own transcript alone.
fn log_state(log: &AgentLog, finished: bool, now: DateTime<Utc>) -> AgentState {
    if finished {
        return AgentState::Done;
    }
    if log.turn_ended {
        return AgentState::Idle;
    }
    if log.last.is_some_and(|l| millis_between(l, now) > QUIET_MS) {
        return AgentState::Quiet;
    }
    match log.pending.values().last() {
        Some(tool) => AgentState::InTool(tool.clone()),
        None => AgentState::Running,
    }
}

/// A started subagent, with what its own transcript says when it was found.
fn spawn_node(spawn: &Spawn, sub: Option<&Sub>, now: DateTime<Utc>) -> AgentNode {
    let meta = sub.and_then(|s| s.meta.as_ref());
    let log = sub.map(|s| &s.log);
    let state = match spawn.end {
        SpawnEnd::Done => AgentState::Done,
        SpawnEnd::Failed => AgentState::Failed,
        SpawnEnd::Stopped => AgentState::Stopped,
        SpawnEnd::Running => match log {
            Some(log) => log_state(log, meta.is_some_and(|m| m.finished), now),
            None => AgentState::Running,
        },
    };
    let started = spawn.started.or_else(|| log.and_then(|l| l.first));
    let duration_ms = spawn.duration_ms.or_else(|| {
        let end = if state.is_live() { Some(now) } else { log.and_then(|l| l.last) };
        started.zip(end).map(|(s, e)| millis_between(s, e))
    });
    AgentNode {
        kind: spawn.agent_type.clone(),
        description: spawn.description.clone(),
        model: log
            .map(|l| l.model.clone())
            .filter(|m| !m.is_empty())
            .or_else(|| spawn.model.clone())
            .or_else(|| meta.map(|m| m.model.clone()))
            .unwrap_or_default(),
        state,
        started,
        duration_ms,
        tokens: spawn.tokens.or_else(|| log.and_then(AgentLog::tokens)),
        tool_uses: spawn.tool_uses.or_else(|| log.map(|l| l.tools.values().map(|(_, n)| u64::from(*n)).sum())),
        summary: spawn.summary.clone(),
        tools: log.map(tool_counts).unwrap_or_default(),
        recent: log.map(|l| l.recent.iter().cloned().collect()).unwrap_or_default(),
        children: Vec::new(),
    }
}

fn tool_counts(log: &AgentLog) -> Vec<ToolCount> {
    let mut out: Vec<ToolCount> = log.tools.iter().map(|(name, (group, count))| ToolCount { name: name.clone(), group: *group, count: *count }).collect();
    out.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
    out
}
