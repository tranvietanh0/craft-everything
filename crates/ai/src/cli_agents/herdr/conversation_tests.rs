use super::*;
use crate::cli_agents::Builder;
use crate::cli_agents::herdr::client::HerdrError;
use crate::cli_agents::herdr::paths::transcript_dir_name;
use crate::config::BackendOptions;
use crate::{BackendConfig, BackendKind};
use std::path::Path;

/// herdr as far as these turns need it: one workspace, one Claude Code that answers each prompt by
/// writing a transcript the way Claude Code does.
struct Fake {
    calls: Mutex<Vec<Vec<String>>>,
    /// `agent_status` answers still to give, oldest first; then "idle".
    states: Mutex<Vec<&'static str>>,
    started: AtomicBool,
    trust_dialog: bool,
    blocked_prompts: Mutex<u32>,
    sessions: PathBuf,
    turn: Vec<String>,
}

impl Fake {
    fn new(sessions: PathBuf) -> Self {
        Fake {
            calls: Mutex::default(),
            states: Mutex::default(),
            started: AtomicBool::new(false),
            trust_dialog: false,
            blocked_prompts: Mutex::new(0),
            sessions,
            turn: vec![
                r#"{"type":"assistant","message":{"id":"m1","model":"claude-opus-5-5","content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn","usage":{"input_tokens":3,"output_tokens":2}}}"#.into(),
                r#"{"type":"system","subtype":"turn_duration","durationMs":900}"#.into(),
            ],
        }
    }

    fn calls(&self) -> Vec<Vec<String>> {
        self.calls.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn called(&self, words: &[&str]) -> Option<Vec<String>> {
        self.calls().into_iter().find(|c| c.iter().zip(words).all(|(a, b)| a == b) && c.len() >= words.len())
    }

    fn session_arg(&self) -> String {
        let start = self.called(&["agent", "start"]).expect("Claude Code was started");
        let at = start.iter().position(|a| a == "--session-id").expect("a new session is named");
        start[at + 1].clone()
    }
}

impl HerdrCall for Fake {
    fn call(&self, args: &[String]) -> Result<String, HerdrError> {
        self.calls.lock().unwrap_or_else(PoisonError::into_inner).push(args.to_vec());
        let words: Vec<&str> = args.iter().map(String::as_str).collect();
        match words.as_slice() {
            ["workspace", "list"] => Ok(r#"{"result":{"workspaces":[]}}"#.into()),
            ["workspace", "create", ..] => Ok(r#"{"result":{"workspace":{"workspace_id":"w9"},"root_pane":{"pane_id":"w9:p1"}}}"#.into()),
            ["pane", "split", ..] => Ok(r#"{"result":{"type":"pane_created","pane":{"pane_id":"w9:p2"}}}"#.into()),
            ["agent", "start", ..] => {
                self.started.store(true, Ordering::SeqCst);
                Ok("{}".into())
            }
            ["agent", "read", ..] => Ok(if self.trust_dialog { "❯ No, exit\n  Yes, I trust this folder" } else { "" }.into()),
            ["agent", "get", ..] if !self.started.load(Ordering::SeqCst) => Err(HerdrError::new("agent_not_found", "none")),
            // As Claude Code 2.1.332 did: ready first, then its trust dialog.
            ["agent", "get", ..] if self.trust_dialog => Ok(r#"{"result":{"agent":{"agent_status":"blocked"}}}"#.into()),
            ["agent", "send-keys", _, "esc"] => {
                self.started.store(false, Ordering::SeqCst);
                Ok("{}".into())
            }
            ["agent", "get", ..] => {
                let mut states = self.states.lock().unwrap_or_else(PoisonError::into_inner);
                let state = if states.is_empty() { "idle" } else { states.remove(0) };
                let seq = if states.is_empty() { 1 } else { 0 };
                Ok(format!(r#"{{"result":{{"agent":{{"agent_status":"{state}","completion_seq":{seq}}}}}}}"#))
            }
            ["agent", "prompt", _, "/exit"] => Ok("{}".into()),
            ["agent", "prompt", ..] => {
                let mut blocked = self.blocked_prompts.lock().unwrap_or_else(PoisonError::into_inner);
                if *blocked > 0 {
                    *blocked -= 1;
                    return Err(HerdrError::new("agent_blocked", "waiting at a dialog"));
                }
                let id = self.session_arg();
                std::fs::create_dir_all(&self.sessions).unwrap();
                let user = serde_json::json!({"type": "user", "message": {"role": "user", "content": words[3]}}).to_string();
                let lines = std::iter::once(user).chain(self.turn.iter().cloned()).collect::<Vec<_>>().join("\n") + "\n";
                let mut f = std::fs::OpenOptions::new().create(true).append(true).open(self.sessions.join(format!("{id}.jsonl"))).unwrap();
                std::io::Write::write_all(&mut f, lines.as_bytes()).unwrap();
                Ok("{}".into())
            }
            _ => Ok("{}".into()),
        }
    }
}

fn spec(project: &Path, builder: bool, deep: bool) -> CliSpec {
    let config = BackendConfig {
        name: "Claude Code (herdr)".into(),
        kind: BackendKind::AgentCli,
        model: super::super::ID.into(),
        base_url: None,
        ssh_host: None,
        enabled: true,
    };
    let mut spec = CliSpec::new(config, BackendOptions::default(), false, Some(project.to_path_buf()));
    spec.deep = deep;
    if builder {
        spec.builder = Some(Builder { investigation: None, inbox: project.join("inbox") });
    }
    spec
}

struct Rig {
    _home: tempfile::TempDir,
    project: tempfile::TempDir,
    fake: Arc<Fake>,
    home: PathBuf,
}

fn rig(tune: impl FnOnce(&mut Fake)) -> Rig {
    let (home, project) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let sessions = home.path().join("projects").join(transcript_dir_name(&project.path().display().to_string()));
    let mut fake = Fake::new(sessions);
    tune(&mut fake);
    Rig { home: home.path().to_path_buf(), _home: home, project, fake: Arc::new(fake) }
}

fn conversation(r: &Rig, spec: CliSpec) -> HerdrConversation {
    HerdrConversation::with(spec, r.fake.clone(), Host::Native, r.home.clone(), Duration::from_millis(5))
}

/// Every event up to and including the turn's `TurnDone`.
fn turn(c: &mut HerdrConversation, text: &str) -> Vec<AgentEvent> {
    c.send(text);
    let mut events = Vec::new();
    let until = Instant::now() + Duration::from_secs(20);
    while Instant::now() < until {
        events.extend(c.poll());
        if events.iter().any(|e| matches!(e, AgentEvent::TurnDone { .. })) {
            return events;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("the turn never ended: {events:?}");
}

#[test]
fn a_builder_turn_is_read_from_the_transcript_and_its_session_kept() {
    let r = rig(|f| *f.states.lock().unwrap() = vec!["working", "working"]);
    let mut c = conversation(&r, spec(r.project.path(), true, true));
    let events = turn(&mut c, "Reply with the word ok");
    assert!(events.contains(&AgentEvent::TextDelta { text: "ok".into() }), "{events:?}");
    assert!(matches!(events.last(), Some(AgentEvent::TurnDone { output_tokens: 2, tokens_unknown: false, .. })), "{events:?}");
    let session = r.fake.session_arg();
    assert_eq!(c.session_id(), Some(session), "the session Claude Code was started with is the one to continue");
    let start = r.fake.called(&["agent", "start"]).unwrap();
    assert!(!start.iter().any(|a| a == "-p" || a == "--permission-prompt-tool"), "interactive, approvals in the pane: {start:?}");
    for flag in ["--append-system-prompt-file", "--mcp-config"] {
        let at = start.iter().position(|a| a == flag).unwrap_or_else(|| panic!("{flag} in {start:?}"));
        assert!(Path::new(&start[at + 1]).is_file(), "{flag} names a file Claude Code can read");
    }
    assert!(r.fake.called(&["pane", "split"]).is_some_and(|c| c.iter().any(|a| a.starts_with("BASH_MAX_TIMEOUT_MS="))), "the pane has the builder's limits");
    // A second message goes to the same Claude Code.
    turn(&mut c, "Again");
    assert_eq!(r.fake.calls().iter().filter(|c| c.starts_with(&["agent".to_string(), "start".to_string()])).count(), 1);
    drop(c);
    assert!(r.fake.called(&["pane", "run", "w9:p2", "/exit"]).is_some(), "Claude Code is ended and its name freed");
    assert!(r.fake.called(&["workspace", "close"]).is_none() && r.fake.called(&["pane", "close"]).is_none(), "the pane is left for the person");
}

#[test]
fn a_folder_claude_code_doesnt_trust_yet_is_reported_and_not_answered() {
    let r = rig(|f| f.trust_dialog = true);
    let mut c = conversation(&r, spec(r.project.path(), true, true));
    let events = turn(&mut c, "Build it");
    assert!(events.iter().any(|e| matches!(e, AgentEvent::Error { message, .. } if message.contains("asking to trust"))), "{events:?}");
    assert!(
        r.fake.calls().iter().any(|c| c.starts_with(&["agent".to_string(), "send-keys".to_string()]) && c.last().is_some_and(|k| k == "esc")),
        "the dialog is closed with No"
    );
    assert!(!r.fake.calls().iter().any(|c| c.iter().any(|a| a.contains("trust"))), "nothing typed says yes");
    assert!(r.fake.called(&["agent", "prompt"]).is_none(), "nothing is sent");
}

#[test]
fn waiting_at_a_dialog_is_reported_once_then_the_turn_goes_on() {
    let r = rig(|f| {
        *f.blocked_prompts.lock().unwrap() = 2;
        *f.states.lock().unwrap() = vec!["working", "blocked", "blocked", "working"];
    });
    let mut c = conversation(&r, spec(r.project.path(), true, true));
    let events = turn(&mut c, "Build it");
    let waits = events.iter().filter(|e| matches!(e, AgentEvent::Status { text } if text.starts_with(super::super::WAITING_STATUS))).count();
    assert_eq!(waits, 2, "once before the message went in, once while it worked: {events:?}");
    assert!(events.contains(&AgentEvent::TextDelta { text: "ok".into() }));
}

#[test]
fn a_session_that_isnt_on_this_computer_is_lost_before_anything_starts() {
    let r = rig(|_| {});
    let mut s = spec(r.project.path(), true, true);
    s.resume = Some("0b6f6c2e-0000-4000-8000-000000000000".into());
    let mut c = conversation(&r, s);
    turn(&mut c, "Carry on");
    assert!(c.session_lost() && c.session_id().is_none());
    assert!(r.fake.called(&["agent", "start"]).is_none());
}

#[test]
fn clean_room_building_never_reaches_herdr() {
    let r = rig(|_| {});
    let mut c = conversation(&r, spec(r.project.path(), true, false));
    let events = turn(&mut c, "Build it");
    assert!(events.iter().any(|e| matches!(e, AgentEvent::Error { message, .. } if message.contains("Clean room"))));
    assert!(r.fake.calls().is_empty());
}
