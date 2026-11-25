//! Starting Claude Code in a herdr pane for a conversation, and letting it go again.

use super::args::{FileKind, mcp_for_agent, values_in_files};
use super::client::{self, AgentState, HerdrCall, HerdrError};
use super::paths::{self, Host};
use super::{ClaudeCodeHerdr, DISPLAY};
use crate::cli_agents::{AgentAdapter, CliSpec, access_for, agent_env, args_for, mcp_for};
use craft_anything_evidence::{ErrorCode, ToolError};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long Claude Code may take to be ready for a prompt.
const START_TIMEOUT_MS: u64 = 90_000;

/// What a conversation's Claude Code needs from this computer.
pub(super) struct Env<'a> {
    pub herdr: &'a dyn HerdrCall,
    pub host: &'a Host,
    /// Claude Code's own folder on the agent's side, as this process reads it.
    pub claude_home: &'a Path,
    /// This conversation's private folder (the preamble and the tool server's config).
    pub files: &'a Path,
    /// Pause between looks at herdr (short in tests).
    pub tick: Duration,
}

/// A running Claude Code.
#[derive(Debug, Clone)]
pub(super) struct Started {
    pub name: String,
    pub pane: String,
    pub label: String,
    /// The folder it runs in, as the agent's side names it.
    pub cwd: String,
    pub session: String,
    pub transcript: PathBuf,
    /// A question's copy of the builder's session: its transcript starts with the builder's.
    pub fork: bool,
}

pub(super) enum StartError {
    /// The session to continue isn't on this computer: nothing started.
    Lost,
    Failed(ToolError),
}

fn failed(what: &str, e: HerdrError) -> StartError {
    StartError::Failed(
        ToolError::new(ErrorCode::BadSettings, format!("herdr couldn't {what}: {} ({})", e.message, e.code))
            .with_remediation("Check that herdr is running (`herdr status`), then try again."),
    )
}

/// Start Claude Code for `spec` in a new pane of the project's workspace. `resume`: the session to
/// continue (a question forks it instead).
pub(super) fn start(env: &Env, spec: &CliSpec, resume: Option<&str>) -> Result<Started, StartError> {
    let project = spec.project.clone().or_else(|| std::env::current_dir().ok()).unwrap_or_else(std::env::temp_dir);
    let cwd = env.host.to_agent(&project);
    let kind = if spec.question {
        'q'
    } else if spec.builder.is_some() {
        'b'
    } else {
        'c'
    };
    let name = paths::agent_name(&project, kind);
    let label = paths::workspace_label(&project);
    let sessions = env.claude_home.join("projects").join(paths::transcript_dir_name(&cwd));
    let fork = spec.question && resume.is_some();
    let session = match resume.filter(|_| !fork) {
        Some(id) if !sessions.join(format!("{id}.jsonl")).is_file() => return Err(StartError::Lost),
        Some(id) => id.to_string(),
        None => super::new_session_id(),
    };
    free_name(env, &name)?;
    let pane = new_pane(env, spec, &cwd, &label)?;
    let args = arguments(env, spec, resume, fork, &session).map_err(StartError::Failed)?;
    unsplit(&args).map_err(StartError::Failed)?;
    if let Err(e) = client::start_claude(env.herdr, &name, &pane, &args, START_TIMEOUT_MS) {
        let screen = client::screen(env.herdr, &name).unwrap_or_default();
        // Not answered for the person: Escape is "No", which ends it and frees the name.
        let _ = client::keys(env.herdr, &name, &["esc"]);
        if client::asks_to_trust(&screen) {
            return Err(StartError::Failed(trust_error(&cwd, &label)));
        }
        let seen = screen.lines().map(str::trim).filter(|l| !l.is_empty()).take(12).collect::<Vec<_>>().join("\n");
        return Err(StartError::Failed(
            ToolError::new(ErrorCode::BadSettings, format!("Claude Code didn't start in herdr: {} ({})", e.message, e.code))
                .with_remediation(format!("Look at the \"{label}\" workspace in herdr. Its screen showed:\n{seen}")),
        ));
    }
    // herdr says it's ready before Claude Code draws its trust dialog (seen with 2.1.332), so the
    // first moments are watched for it before anything is typed.
    settle(env, &name, &cwd, &label).map_err(StartError::Failed)?;
    Ok(Started { name, pane, label, cwd, transcript: sessions.join(format!("{session}.jsonl")), session, fork })
}

/// herdr 0.9.3 starts `claude.exe` on Windows with PowerShell's `Start-Process -ArgumentList`
/// and every argument joined by spaces (seen live), so a value with white space in it would reach
/// Claude Code as several. Such a run is refused rather than started with other arguments.
fn unsplit(args: &[String]) -> Result<(), ToolError> {
    match args.iter().find(|a| a.chars().any(char::is_whitespace)) {
        None => Ok(()),
        Some(a) => Err(ToolError::new(ErrorCode::BadSettings, format!("{DISPLAY} can't pass an argument with spaces in it to Claude Code through herdr: {a}"))
            .with_remediation("Remove the spaces (a folder without spaces for the temporary files or the extra arguments in the backend's settings), or use Claude Code without herdr.")),
    }
}

/// Looks at a just-started Claude Code until it has stayed idle for a moment.
const SETTLE_LOOKS: u32 = 5;

/// Wait until Claude Code is idle a few looks in a row. A trust dialog meanwhile is closed with
/// "No" (Escape) and reported; any other dialog is left for the person and the turn reports it.
fn settle(env: &Env, name: &str, cwd: &str, label: &str) -> Result<(), ToolError> {
    let mut idle = 0;
    for _ in 0..SETTLE_LOOKS * 4 {
        match client::agent_state(env.herdr, name) {
            Ok((AgentState::Blocked, _)) => {
                return match refuse_trust(env.herdr, name, cwd, label) {
                    Some(e) => Err(e),
                    None => Ok(()),
                };
            }
            Ok((s, _)) if s.settled() => idle += 1,
            _ => idle = 0,
        }
        if idle >= SETTLE_LOOKS {
            return Ok(());
        }
        std::thread::sleep(env.tick);
    }
    Ok(())
}

/// When the screen is Claude Code's trust dialog: closed with "No" (which ends Claude Code and
/// frees its name) and the error to report. `None` for any other dialog.
pub(super) fn refuse_trust(h: &dyn HerdrCall, name: &str, cwd: &str, label: &str) -> Option<ToolError> {
    let screen = client::screen(h, name).unwrap_or_default();
    if !client::asks_to_trust(&screen) {
        return None;
    }
    let _ = client::keys(h, name, &["esc"]);
    Some(trust_error(cwd, label))
}

/// What the person is told when Claude Code asks to trust the project folder.
fn trust_error(cwd: &str, label: &str) -> ToolError {
    ToolError::new(ErrorCode::BadSettings, format!("Claude Code is asking to trust {cwd} in herdr (workspace \"{label}\")."))
        .with_remediation(format!(
            "Craft Anything doesn't answer that for you. In that herdr workspace (or any terminal) run `claude` in {cwd}, choose \"Yes, I trust this folder\", type /exit, then try again. Trusting a folder above it covers it too, unless it is a git repository of its own."
        ))
}

/// A Claude Code left running under this name (a run that was killed) is asked to exit, unless it
/// is busy or waiting for the person.
fn free_name(env: &Env, name: &str) -> Result<(), StartError> {
    let Ok((state, _)) = client::agent_state(env.herdr, name) else {
        return Ok(());
    };
    if matches!(state, AgentState::Working | AgentState::Blocked) {
        return Err(StartError::Failed(
            ToolError::new(
                ErrorCode::BadSettings,
                format!("Claude Code is still {} in herdr as {name}.", if state == AgentState::Working { "working" } else { "waiting for you" }),
            )
            .with_remediation("Finish or close it in herdr (type /exit there), then try again."),
        ));
    }
    let _ = client::prompt(env.herdr, name, "/exit");
    for _ in 0..40 {
        if client::agent_state(env.herdr, name).is_err() {
            return Ok(());
        }
        std::thread::sleep(env.tick);
    }
    Err(StartError::Failed(
        ToolError::new(ErrorCode::BadSettings, format!("An earlier Claude Code is still running in herdr as {name}."))
            .with_remediation("Type /exit in its pane, then try again."),
    ))
}

/// A new shell pane in the project's workspace, with Claude Code's limits and effort set. An
/// earlier run's pane in this folder that nothing runs in any more is closed first; the
/// workspace's first pane is the person's and is left alone.
fn new_pane(env: &Env, spec: &CliSpec, cwd: &str, label: &str) -> Result<String, StartError> {
    let first = match client::find_workspace(env.herdr, label).map_err(|e| failed("list its workspaces", e))? {
        Some(ws) => {
            let panes = client::panes(env.herdr, &ws).map_err(|e| failed("list the workspace's panes", e))?;
            let first = panes.first().map(|p| p.id.clone()).ok_or_else(|| failed("find a pane", HerdrError::new("no_pane", "the workspace has none")))?;
            for old in panes.iter().skip(1).filter(|p| p.state == AgentState::Unknown && p.cwd.eq_ignore_ascii_case(cwd)) {
                let _ = client::close_pane(env.herdr, &old.id);
            }
            first
        }
        None => client::create_workspace(env.herdr, cwd, label).map_err(|e| failed("make a workspace", e))?.1,
    };
    let vars = agent_env(spec.builder.is_some() && !spec.question, spec.options.effort);
    let vars: Vec<(&str, String)> = vars.iter().map(|(k, v)| (*k, v.clone())).collect();
    client::split(env.herdr, &first, cwd, &vars).map_err(|e| failed("open a pane", e))
}

/// Claude Code's arguments: the adapter's, with the preamble and the tool server in files, and the
/// session named so its transcript can be found.
fn arguments(env: &Env, spec: &CliSpec, resume: Option<&str>, fork: bool, session: &str) -> Result<Vec<String>, ToolError> {
    let access = access_for(spec.builder.is_some(), spec.deep, spec.question);
    let preamble = spec.preamble.clone().unwrap_or_else(|| crate::prompt::cli_preamble(spec.beginner));
    let mcp = mcp_for_agent(env.host, &mcp_for(spec, false));
    let adapter: &dyn AgentAdapter = &ClaudeCodeHerdr;
    // The message isn't in these: it is typed into the pane once Claude Code is ready.
    let args = args_for(adapter, &spec.options, "", &preamble, &mcp, resume, access, None);
    let mut args = values_in_files(args, |kind, value| {
        let file = env.files.join(match kind {
            FileKind::Preamble => "preamble.md",
            FileKind::Mcp => "mcp.json",
        });
        std::fs::write(&file, value).map_err(|e| ToolError::io(file.display(), e))?;
        Ok::<_, ToolError>(env.host.to_agent(&file))
    })?;
    // A new session, or a question's copy, gets the id we choose; a resumed one keeps its own.
    if resume.is_none() || fork {
        args.extend(["--session-id".into(), session.to_string()]);
    }
    Ok(args)
}

/// End Claude Code (Escape first, in case it is mid-turn or at a dialog) and free its name. The
/// pane and workspace stay for the person to look at.
pub(super) fn release(h: &dyn HerdrCall, started: &Started) {
    let _ = client::keys(h, &started.name, &["esc"]);
    let _ = client::run_line(h, &started.pane, "/exit");
}

/// Shown when Claude Code needs the person in herdr.
pub(super) fn waiting_text(label: &str) -> String {
    format!("{}: {DISPLAY} is asking you in the \"{label}\" workspace.", super::WAITING_STATUS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_argument_herdr_would_split_is_refused() {
        let args = |v: &[&str]| v.iter().map(|a| a.to_string()).collect::<Vec<_>>();
        assert!(unsplit(&args(&["--model", "opus", "--tools=", "--mcp-config", "C:/tmp/mcp.json"])).is_ok());
        assert!(unsplit(&args(&["--settings", "{\"a\": 1}"])).is_err());
        assert!(unsplit(&args(&["--mcp-config", "C:/Users/A B/mcp.json"])).is_err());
    }
}
