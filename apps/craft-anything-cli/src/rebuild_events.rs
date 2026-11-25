//! `rebuild --events ndjson`: every run event as one JSON object per line on stdout, for a program
//! that runs the CLI (a bot). Event types and fields are listed in `docs/rebuild-events.md`.
//! Agent events keep the `type` names the chat's `--format jsonl` already uses (snake_case).

use clap::ValueEnum;
use craft_anything_ai::AgentEvent;
use craft_anything_ai::rebuild::RunEvent;
use craft_anything_engine::inbox::RequestKind;
use craft_anything_engine::workspace::{Step, TargetRef};
use serde_json::{Value, json};
use std::io::{self, Write};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum EventFormat {
    /// One JSON object per line on stdout; human text goes to stderr.
    Ndjson,
}

/// Writes a run's events as NDJSON, one flushed line each. Agent text arrives in small pieces, so
/// it is joined and written one line at a time; any text still waiting is written before the next
/// event, so the order of the stream is the order of the run.
pub struct NdjsonWriter<W: Write> {
    out: W,
    partial: String,
}

impl<W: Write> NdjsonWriter<W> {
    pub fn new(out: W) -> Self {
        Self { out, partial: String::new() }
    }

    /// The first line of a run: the project folder and what it is building.
    pub fn project(&mut self, dir: &Path, target: &TargetRef) -> io::Result<()> {
        self.line(&json!({"type": "project", "dir": dir.display().to_string(), "target": target}))
    }

    pub fn event(&mut self, e: &RunEvent) -> io::Result<()> {
        if let RunEvent::Agent(AgentEvent::TextDelta { text }) = e {
            return self.text(text);
        }
        self.flush_text()?;
        self.line(&to_value(e)?)
    }

    /// Writes agent text that has no line end yet. Called before every other event and at the end.
    pub fn flush_text(&mut self) -> io::Result<()> {
        if self.partial.is_empty() {
            return Ok(());
        }
        let rest = std::mem::take(&mut self.partial);
        self.line(&json!({"type": "agent_text", "text": rest}))
    }

    fn text(&mut self, delta: &str) -> io::Result<()> {
        self.partial.push_str(delta);
        while let Some(end) = self.partial.find('\n') {
            let tail = self.partial.split_off(end + 1);
            let mut line = std::mem::replace(&mut self.partial, tail);
            line.pop();
            if line.ends_with('\r') {
                line.pop();
            }
            self.line(&json!({"type": "agent_text", "text": line}))?;
        }
        Ok(())
    }

    fn line(&mut self, v: &Value) -> io::Result<()> {
        // Value's Display is compact JSON with every newline escaped, so one value is one line.
        writeln!(self.out, "{v}")?;
        self.out.flush()
    }
}

fn to_value(e: &RunEvent) -> io::Result<Value> {
    Ok(match e {
        RunEvent::Step { step, text } => json!({"type": "step", "step": step_id(*step), "label": step.label(), "text": text}),
        RunEvent::Agent(agent) => serde_json::to_value(agent).map_err(io::Error::other)?,
        RunEvent::Note { shown, pictures } => {
            let pictures: Vec<String> = pictures.iter().map(|p| p.display().to_string()).collect();
            json!({"type": "note", "shown": shown, "pictures": pictures})
        }
        RunEvent::Ask(r) => {
            let kind = match r.kind {
                RequestKind::Question => "question",
                RequestKind::Approval => "approval",
            };
            json!({"type": "ask", "id": r.id, "kind": kind, "title": r.title, "detail": r.detail, "choices": r.choices, "allow_text": r.allow_text, "action": r.action})
        }
        RunEvent::Checked(c) => {
            let artifacts: Vec<String> = c.artifacts.iter().map(|p| p.display().to_string()).collect();
            json!({"type": "checked", "round": c.round, "check": c.check, "passed": c.passed, "similarity": c.similarity, "note": c.note, "artifacts": artifacts})
        }
        RunEvent::Preview(url) => json!({"type": "preview", "url": url}),
        RunEvent::Finished { ok, message, paused } => json!({"type": "finished", "ok": ok, "paused": paused, "message": message}),
    })
}

fn step_id(step: Step) -> &'static str {
    match step {
        Step::Study => "study",
        Step::Build => "build",
        Step::Check => "check",
        Step::Done => "done",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft_anything_engine::inbox::Request;
    use std::path::PathBuf;

    fn lines(w: NdjsonWriter<Vec<u8>>) -> Vec<Value> {
        String::from_utf8(w.out).unwrap().lines().map(|l| serde_json::from_str(l).expect("every line is one JSON object")).collect()
    }

    fn delta(text: &str) -> RunEvent {
        RunEvent::Agent(AgentEvent::TextDelta { text: text.into() })
    }

    #[test]
    fn agent_text_is_one_line_per_line_and_keeps_a_partial_line_until_flushed() {
        let mut w = NdjsonWriter::new(Vec::new());
        for piece in ["Hel", "lo\nwor", "ld\r\n", "tail"] {
            w.event(&delta(piece)).unwrap();
        }
        w.flush_text().unwrap();
        let got: Vec<String> = lines(w).iter().map(|v| v["text"].as_str().unwrap_or_default().to_string()).collect();
        assert_eq!(got, vec!["Hello".to_string(), "world".to_string(), "tail".to_string()]);
    }

    #[test]
    fn pending_agent_text_is_written_before_the_next_event() {
        let mut w = NdjsonWriter::new(Vec::new());
        w.event(&delta("half a sentence")).unwrap();
        w.event(&RunEvent::Preview("http://x".into())).unwrap();
        let v = lines(w);
        assert_eq!(v[0]["type"], "agent_text");
        assert_eq!(v[0]["text"], "half a sentence");
        assert_eq!(v[1]["type"], "preview");
    }

    #[test]
    fn every_run_event_is_one_object_with_a_type() {
        let mut w = NdjsonWriter::new(Vec::new());
        w.project(Path::new("/projects/x"), &TargetRef::None).unwrap();
        let events = [
            RunEvent::Step { step: Step::Build, text: "Building".into() },
            RunEvent::Agent(AgentEvent::Status { text: "reading".into() }),
            RunEvent::Agent(AgentEvent::ToolStarted { call_id: "c1".into(), tool: "Read".into(), input: json!({"path": "a"}) }),
            RunEvent::Note { shown: "more colour".into(), pictures: vec![PathBuf::from("pics/a.png")] },
            RunEvent::Ask(Request {
                id: "q1".into(),
                kind: RequestKind::Question,
                title: "Which?".into(),
                detail: String::new(),
                choices: vec!["A".into()],
                allow_text: false,
                action: None,
                created_ms: 0,
            }),
            RunEvent::Preview("http://127.0.0.1:1/".into()),
            RunEvent::Finished { ok: true, message: "Ready.".into(), paused: false },
        ];
        for e in &events {
            w.event(e).unwrap();
        }
        let v = lines(w);
        assert_eq!(v.len(), events.len() + 1);
        assert!(v.iter().all(|x| x["type"].is_string()), "every line has a type: {v:?}");
        assert_eq!(v[0]["type"], "project");
        assert_eq!(v[0]["dir"], "/projects/x");
        assert_eq!(v[1]["step"], "build");
        assert_eq!(v[1]["label"], "Build");
        assert_eq!(v[3]["type"], "tool_started", "agent events keep the chat's type names");
        assert_eq!(v[5]["kind"], "question");
        assert_eq!(v[v.len() - 1]["type"], "finished");
        assert_eq!(v[v.len() - 1]["ok"], true);
    }
}
