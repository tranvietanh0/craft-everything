//! Talking to herdr: one call runs its CLI and returns what it printed. Everything else here reads
//! herdr's JSON, so it is tested with a fake [`HerdrCall`] and no herdr at all.

use craft_anything_platform::process::Process;
use serde_json::Value;
use std::time::Duration;

/// A herdr refusal (`{"error":{"code","message"}}`) or a call that couldn't run (`code` = `spawn`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HerdrError {
    pub code: String,
    pub message: String,
}

impl HerdrError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        HerdrError { code: code.to_string(), message: message.into() }
    }
}

/// Runs herdr with `args` and returns its stdout, or its error.
pub trait HerdrCall: Send + Sync {
    fn call(&self, args: &[String]) -> Result<String, HerdrError>;
}

/// The real herdr program.
pub struct HerdrCli {
    pub program: String,
}

impl HerdrCall for HerdrCli {
    fn call(&self, args: &[String]) -> Result<String, HerdrError> {
        // Starting Claude Code waits up to its own --timeout; everything else answers at once.
        let out = Process::new(&self.program)
            .args(args)
            .label("herdr")
            .timeout(Duration::from_secs(330))
            .stdout_cap(4 * 1024 * 1024)
            .run()
            .map_err(|e| HerdrError::new("spawn", e.message))?;
        outcome(out.success, &out.stdout_text(), &out.stderr_text())
    }
}

/// What a finished herdr call means: a JSON `error` on stdout is the refusal, whatever the exit code.
pub fn outcome(success: bool, stdout: &str, stderr: &str) -> Result<String, HerdrError> {
    if let Ok(v) = serde_json::from_str::<Value>(stdout.trim())
        && let Some(e) = v.get("error")
    {
        let s = |k: &str| e.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
        return Err(HerdrError { code: s("code"), message: s("message") });
    }
    if success {
        return Ok(stdout.to_string());
    }
    let why = if stderr.trim().is_empty() { stdout.trim() } else { stderr.trim() };
    Err(HerdrError::new("failed", why.chars().take(400).collect::<String>()))
}

fn json(out: &str) -> Result<Value, HerdrError> {
    serde_json::from_str(out.trim()).map_err(|_| HerdrError::new("bad_reply", format!("herdr printed something that isn't JSON: {}", out.trim())))
}

fn text_at(v: &Value, pointer: &str) -> Option<String> {
    v.pointer(pointer).and_then(Value::as_str).map(str::to_string)
}

/// What herdr's screen rules say an agent is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentState {
    Idle,
    Working,
    /// Waiting for the person (an approval, a question, a trust dialog).
    Blocked,
    Done,
    Unknown,
}

impl AgentState {
    pub fn parse(s: &str) -> AgentState {
        match s {
            "idle" => AgentState::Idle,
            "working" => AgentState::Working,
            "blocked" => AgentState::Blocked,
            "done" => AgentState::Done,
            _ => AgentState::Unknown,
        }
    }

    /// Nothing is running: a turn that was working has ended.
    pub fn settled(self) -> bool {
        matches!(self, AgentState::Idle | AgentState::Done)
    }
}

/// Whether the herdr server answers.
pub fn status(h: &dyn HerdrCall) -> Result<(), HerdrError> {
    let out = h.call(&["status".into()])?;
    if out.contains("running") { Ok(()) } else { Err(HerdrError::new("not_running", format!("herdr's server isn't running: {}", out.trim()))) }
}

/// The first workspace labelled `label`.
pub fn find_workspace(h: &dyn HerdrCall, label: &str) -> Result<Option<String>, HerdrError> {
    let v = json(&h.call(&["workspace".into(), "list".into()])?)?;
    let list = v.pointer("/result/workspaces").and_then(Value::as_array).cloned().unwrap_or_default();
    Ok(list.iter().find(|w| w.get("label").and_then(Value::as_str) == Some(label)).and_then(|w| text_at(w, "/workspace_id")))
}

/// A new workspace in `cwd`, without taking focus: its id and its first pane.
pub fn create_workspace(h: &dyn HerdrCall, cwd: &str, label: &str) -> Result<(String, String), HerdrError> {
    let args = ["workspace", "create", "--cwd", cwd, "--label", label, "--no-focus"].map(String::from);
    let v = json(&h.call(&args)?)?;
    let ws = text_at(&v, "/result/workspace/workspace_id");
    let pane = text_at(&v, "/result/root_pane/pane_id");
    ws.zip(pane).ok_or_else(|| HerdrError::new("bad_reply", "herdr made a workspace but didn't say its id and pane"))
}

/// One pane of a workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pane {
    pub id: String,
    pub state: AgentState,
    pub cwd: String,
}

/// A workspace's panes, in herdr's order.
pub fn panes(h: &dyn HerdrCall, workspace: &str) -> Result<Vec<Pane>, HerdrError> {
    let v = json(&h.call(&["pane".into(), "list".into(), "--workspace".into(), workspace.into()])?)?;
    let list = v.pointer("/result/panes").and_then(Value::as_array).cloned().unwrap_or_default();
    Ok(list
        .iter()
        .filter_map(|p| {
            let state = AgentState::parse(p.get("agent_status").and_then(Value::as_str).unwrap_or_default());
            Some(Pane { id: text_at(p, "/pane_id")?, state, cwd: text_at(p, "/cwd").unwrap_or_default() })
        })
        .collect())
}

/// A new shell pane beside `pane`, in `cwd`, with `env` set for what runs in it.
pub fn split(h: &dyn HerdrCall, pane: &str, cwd: &str, env: &[(&str, String)]) -> Result<String, HerdrError> {
    let mut args = ["pane", "split", pane, "--direction", "right", "--cwd", cwd, "--no-focus"].map(String::from).to_vec();
    for (k, v) in env {
        args.extend(["--env".into(), format!("{k}={v}")]);
    }
    let v = json(&h.call(&args)?)?;
    text_at(&v, "/result/pane/pane_id").ok_or_else(|| HerdrError::new("bad_reply", "herdr split a pane but didn't say its id"))
}

pub fn close_pane(h: &dyn HerdrCall, pane: &str) -> Result<(), HerdrError> {
    h.call(&["pane".into(), "close".into(), pane.into()]).map(|_| ())
}

/// Start Claude Code as `name` in `pane` with `args`, waiting up to `timeout_ms` for it to be ready.
pub fn start_claude(h: &dyn HerdrCall, name: &str, pane: &str, args: &[String], timeout_ms: u64) -> Result<(), HerdrError> {
    let mut a = ["agent", "start", name, "--kind", "claude", "--pane", pane, "--timeout"].map(String::from).to_vec();
    a.push(timeout_ms.to_string());
    a.push("--".into());
    a.extend(args.iter().cloned());
    h.call(&a).map(|_| ())
}

/// An agent's state and how many turns it has finished (`completion_seq`).
pub fn agent_state(h: &dyn HerdrCall, name: &str) -> Result<(AgentState, u64), HerdrError> {
    let v = json(&h.call(&["agent".into(), "get".into(), name.into()])?)?;
    let a = v.pointer("/result/agent").unwrap_or(&Value::Null);
    let state = AgentState::parse(a.get("agent_status").and_then(Value::as_str).unwrap_or_default());
    Ok((state, a.get("completion_seq").and_then(Value::as_u64).unwrap_or(0)))
}

/// Type `text` into the agent and submit it, without waiting for the turn.
pub fn prompt(h: &dyn HerdrCall, name: &str, text: &str) -> Result<(), HerdrError> {
    h.call(&["agent".into(), "prompt".into(), name.into(), text.into()]).map(|_| ())
}

/// What the agent's screen shows now (plain text).
pub fn screen(h: &dyn HerdrCall, name: &str) -> Result<String, HerdrError> {
    h.call(&["agent".into(), "read".into(), name.into(), "--source".into(), "visible".into()])
}

pub fn keys(h: &dyn HerdrCall, name: &str, keys: &[&str]) -> Result<(), HerdrError> {
    let mut a = vec!["agent".to_string(), "send-keys".into(), name.into()];
    a.extend(keys.iter().map(|k| k.to_string()));
    h.call(&a).map(|_| ())
}

/// Type a line into a pane and press Enter (`/exit` ends Claude Code and frees its name).
pub fn run_line(h: &dyn HerdrCall, pane: &str, line: &str) -> Result<(), HerdrError> {
    h.call(&["pane".into(), "run".into(), pane.into(), line.into()]).map(|_| ())
}

/// Whether the screen is Claude Code's "trust this folder" dialog, which only the person answers.
pub fn asks_to_trust(screen: &str) -> bool {
    let s = screen.to_ascii_lowercase();
    s.contains("trust this folder") || s.contains("do you trust the files")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_json_error_is_a_refusal_even_when_herdr_exits_zero_or_nonzero() {
        let refused = r#"{"error":{"code":"agent_blocked","message":"agent x is blocked"},"id":"cli:agent:prompt"}"#;
        for success in [true, false] {
            assert_eq!(outcome(success, refused, "").unwrap_err().code, "agent_blocked");
        }
        assert!(outcome(true, "{\"result\":{}}", "").is_ok());
        assert_eq!(outcome(false, "", "boom").unwrap_err().message, "boom", "a failure without JSON says what herdr printed");
    }

    #[test]
    fn only_idle_and_done_end_a_turn() {
        let settled: Vec<&str> =
            ["idle", "working", "blocked", "done", "unknown", "new-state"].into_iter().filter(|s| AgentState::parse(s).settled()).collect();
        assert_eq!(settled, ["idle", "done"]);
    }

    #[test]
    fn the_trust_dialog_is_told_from_an_approval() {
        assert!(asks_to_trust("Accessing workspace:\n ❯ No, exit\n   Yes, I trust this folder"));
        assert!(!asks_to_trust("Do you want to make this edit to main.rs?\n ❯ 1. Yes"));
    }
}
