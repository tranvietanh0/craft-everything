//! Claude Code's session transcript (`<claude home>/projects/<folder>/<session>.jsonl`) read as it
//! grows, turned into the same [`AgentEvent`]s its headless stream gives. Its `assistant` and
//! `user` entries have the stream's shape, so they go through the headless adapter's own parser;
//! what the transcript adds is usage per message (no final `result` line) and a `turn_duration`
//! entry when a turn ends.

use super::super::adapters::ClaudeCode;
use super::super::{StreamParser, content_text};
use crate::AgentEvent;
use crate::model::Usage;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// One turn's reading of a transcript.
pub struct Transcript {
    path: PathBuf,
    offset: u64,
    partial: String,
    parser: StreamParser,
    /// Usage of each model message, by message id: a message written as several entries (one per
    /// content block) repeats its usage, so it is counted once.
    usage: HashMap<String, (String, Usage)>,
    /// Entries before the user message that starts with this are someone else's (a forked
    /// session's copied history), not this turn's.
    gate: Option<String>,
    /// Claude Code wrote that the turn ended.
    pub ended: bool,
    /// Its last model message finished the turn (`end_turn`) rather than calling a tool.
    pub answered: bool,
    /// Any entry of this turn was read.
    pub seen: bool,
}

/// What a message is recognised by: its first 60 characters that aren't white space (herdr may
/// type line breaks differently).
fn key(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).take(60).collect()
}

impl Transcript {
    /// Read `path` from `offset` (its length before this turn, 0 for a new session). With
    /// `after_prompt`, entries count only from the user message that is that prompt.
    pub fn new(path: PathBuf, offset: u64, after_prompt: Option<&str>) -> Self {
        Transcript {
            path,
            offset,
            partial: String::new(),
            parser: StreamParser::new(Arc::new(ClaudeCode::default())),
            usage: HashMap::new(),
            gate: after_prompt.map(key).filter(|k| !k.is_empty()),
            ended: false,
            answered: false,
            seen: false,
        }
    }

    /// A failure Claude Code wrote down (an API error, a plan limit), as the headless parser reads it.
    pub fn failure(&self) -> Option<crate::TurnFailure> {
        self.parser.failure
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Events for what was written since the last read. A file not there yet is nothing new.
    pub fn read_new(&mut self) -> Vec<AgentEvent> {
        let Ok(mut f) = std::fs::File::open(&self.path) else {
            return Vec::new();
        };
        let mut bytes = Vec::new();
        if f.seek(SeekFrom::Start(self.offset)).is_err() || f.read_to_end(&mut bytes).is_err() {
            return Vec::new();
        }
        self.offset += bytes.len() as u64;
        self.partial.push_str(&String::from_utf8_lossy(&bytes));
        let mut events = Vec::new();
        while let Some(end) = self.partial.find('\n') {
            let line: String = self.partial.drain(..=end).collect();
            events.extend(self.line(&line));
        }
        events
    }

    /// Events for one whole transcript line.
    pub fn line(&mut self, line: &str) -> Vec<AgentEvent> {
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
            return Vec::new();
        };
        // A subagent's own steps, and Claude Code's notes to itself, aren't the conversation.
        if v.get("isSidechain").and_then(Value::as_bool) == Some(true) || v.get("isMeta").and_then(Value::as_bool) == Some(true) {
            return Vec::new();
        }
        let kind = v.get("type").and_then(Value::as_str).unwrap_or_default();
        if let Some(gate) = &self.gate {
            let text = v.pointer("/message/content").map(content_text).unwrap_or_default();
            if kind == "user" && key(&text).starts_with(gate.as_str()) {
                self.gate = None;
            }
            return Vec::new();
        }
        match kind {
            "assistant" => {
                self.seen = true;
                let message = v.get("message").unwrap_or(&Value::Null);
                if let (Some(id), Some(u)) = (message.get("id").and_then(Value::as_str), message.get("usage")) {
                    let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
                    let usage = Usage {
                        input_tokens: n("input_tokens"),
                        output_tokens: n("output_tokens"),
                        cache_read_tokens: n("cache_read_input_tokens"),
                        cache_write_tokens: n("cache_creation_input_tokens"),
                    };
                    let model = message.get("model").and_then(Value::as_str).unwrap_or_default().to_string();
                    self.usage.insert(id.to_string(), (model, usage));
                }
                match message.get("stop_reason").and_then(Value::as_str) {
                    Some("end_turn") => self.answered = true,
                    Some(_) => self.answered = false,
                    None => {}
                }
                self.parser.line(line)
            }
            "user" => {
                self.seen = true;
                self.parser.line(line)
            }
            "system" if v.get("subtype").and_then(Value::as_str) == Some("turn_duration") => {
                self.ended = true;
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// Every prompt token (cached ones included, as agent apps report them), the output tokens,
    /// and the cost at list prices (`None` when a model's price isn't known).
    pub fn totals(&self) -> (u64, u64, Option<f64>) {
        let mut cost = Some(0.0);
        let (mut input, mut output) = (0u64, 0u64);
        for (model, u) in self.usage.values() {
            input = input.saturating_add(u.input_tokens).saturating_add(u.cache_read_tokens).saturating_add(u.cache_write_tokens);
            output = output.saturating_add(u.output_tokens);
            cost = cost.zip(crate::price::cost(model, u, &BTreeMap::new(), false)).map(|(a, b)| a + b);
        }
        (input, output, cost.filter(|_| !self.usage.is_empty()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assistant(id: &str, blocks: Value, stop: &str, output: u64) -> String {
        serde_json::json!({"type": "assistant", "isSidechain": false, "message": {"id": id, "model": "claude-opus-5-5", "role": "assistant",
            "content": blocks, "stop_reason": stop, "usage": {"input_tokens": 2, "cache_read_input_tokens": 100, "cache_creation_input_tokens": 10, "output_tokens": output}}})
        .to_string()
    }

    /// Lines in the shape Claude Code 2.1 writes for an interactive session.
    fn turn() -> Vec<String> {
        vec![
            r#"{"type":"permission-mode","permissionMode":"default","sessionId":"s"}"#.to_string(),
            r#"{"type":"user","isSidechain":false,"message":{"role":"user","content":"Reply with the word ok"}}"#.to_string(),
            assistant("m1", serde_json::json!([{"type": "tool_use", "id": "t1", "name": "mcp__craft-anything__core_feature_set", "input": {"name": "Menu"}}]), "tool_use", 5),
            r#"{"type":"user","isSidechain":true,"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"sub","content":"x"}]}}"#.to_string(),
            r#"{"type":"user","isSidechain":false,"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"done","is_error":false}]}}"#.to_string(),
            assistant("m2", serde_json::json!([{"type": "thinking", "thinking": "."}]), "end_turn", 7),
            assistant("m2", serde_json::json!([{"type": "text", "text": "ok"}]), "end_turn", 7),
            r#"{"type":"system","subtype":"turn_duration","durationMs":1200}"#.to_string(),
        ]
    }

    #[test]
    fn a_turn_reads_as_the_headless_stream_would() {
        let mut t = Transcript::new(PathBuf::from("unused"), 0, None);
        let events: Vec<AgentEvent> = turn().iter().flat_map(|l| t.line(l)).collect();
        let tools: Vec<&AgentEvent> = events.iter().filter(|e| matches!(e, AgentEvent::ToolStarted { .. } | AgentEvent::ToolFinished { .. })).collect();
        assert!(
            matches!(tools.as_slice(), [AgentEvent::ToolStarted { tool, .. }, AgentEvent::ToolFinished { ok: true, .. }] if tool == "core_feature_set"),
            "{events:?}"
        );
        assert!(events.contains(&AgentEvent::TextDelta { text: "ok".into() }));
        assert!(t.ended && t.answered);
        let (input, output, cost) = t.totals();
        assert_eq!(output, 12, "a message written as two entries is counted once");
        assert!(input > output && cost.is_some_and(|c| c > 0.0));
    }

    #[test]
    fn a_forks_copied_history_is_not_this_turn() {
        let mut t = Transcript::new(PathBuf::from("unused"), 0, Some("What does the menu do?"));
        let mut lines = turn();
        lines.push(r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"What does the\nmenu do?"}]}}"#.to_string());
        lines.push(assistant("m3", serde_json::json!([{"type": "text", "text": "It opens."}]), "end_turn", 3));
        let events: Vec<AgentEvent> = lines.iter().flat_map(|l| t.line(l)).collect();
        assert_eq!(events, vec![AgentEvent::TextDelta { text: "It opens.".into() }], "only what follows the question");
        assert!(!t.ended, "the copied turn's end isn't this one's");
        assert_eq!(t.totals().1, 3);
    }

    #[test]
    fn lines_written_in_pieces_are_read_whole() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        let lines = turn();
        let all = lines.join("\n") + "\n";
        let (first, rest) = all.split_at(all.len() / 2);
        std::fs::write(&path, first).unwrap();
        let mut t = Transcript::new(path.clone(), 0, None);
        let mut events = t.read_new();
        std::fs::write(&path, all.as_bytes()).unwrap();
        events.extend(t.read_new());
        assert!(rest.len() > 1 && t.ended && events.contains(&AgentEvent::TextDelta { text: "ok".into() }));
    }
}
