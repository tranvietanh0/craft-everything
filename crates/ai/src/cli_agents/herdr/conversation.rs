//! A conversation with Claude Code in a herdr pane: one Claude Code per conversation, started at
//! the first message and prompted for each, its transcript read for the turn's events.

use super::client::{self, AgentState, HerdrCall, HerdrCli};
use super::paths::Host;
use super::session::{self, Env, StartError, Started};
use super::transcript::Transcript;
use super::{ClaudeCodeHerdr, DISPLAY};
use crate::cli_agents::{AgentAdapter, CliSpec, access_for, end_turn, nothing_done};
use crate::model::Stop;
use crate::{AgentEvent, Conversation, FailureKind, TurnFailure};
use craft_anything_evidence::ToolError;
use craft_anything_platform::sync::Mutex;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, PoisonError};
use std::time::{Duration, Instant};

/// A message longer than this goes in a file: herdr takes it on its command line.
const LONGEST_TYPED: usize = 20_000;
/// Looks at a settled agent before a turn whose transcript never said it ended counts as over.
const QUIET_LOOKS: u32 = 20;

/// What every turn of the conversation uses.
struct Setup {
    herdr: Arc<dyn HerdrCall>,
    host: Host,
    claude_home: PathBuf,
    files: PathBuf,
    tick: Duration,
}

/// Claude Code's session and its pane, kept across turns.
#[derive(Default)]
struct Live {
    started: Option<Started>,
    /// A question's copies of the builder's session, removed with the conversation.
    forks: Vec<PathBuf>,
    messages: u32,
}

pub struct HerdrConversation {
    spec: CliSpec,
    setup: Result<Arc<Setup>, ToolError>,
    tx: Sender<AgentEvent>,
    rx: Receiver<AgentEvent>,
    busy: Arc<AtomicBool>,
    stop: Stop,
    live: Arc<Mutex<Live>>,
    /// The session to continue, or the one this conversation started (kept apart from `live`,
    /// which a turn holds while it runs).
    session: Arc<Mutex<Option<String>>>,
    lost: Arc<AtomicBool>,
    failure: Arc<Mutex<Option<TurnFailure>>>,
}

impl HerdrConversation {
    /// The conversation `spec` describes, through the herdr found on this computer.
    pub fn new(spec: CliSpec) -> Self {
        let host = Host::current();
        let setup = super::program(&spec.options, &host).and_then(|program| {
            let claude_home =
                super::claude_home(&host).ok_or_else(|| ToolError::invalid("Couldn't tell where Claude Code keeps its conversations for herdr."))?;
            Ok((Arc::new(HerdrCli { program }) as Arc<dyn HerdrCall>, claude_home))
        });
        match setup {
            Ok((herdr, home)) => Self::with(spec, herdr, host, home, Duration::from_millis(700)),
            Err(e) => Self::build(spec, Err(e)),
        }
    }

    /// With a given herdr and Claude Code folder (tests use a fake herdr and a short `tick`).
    fn with(spec: CliSpec, herdr: Arc<dyn HerdrCall>, host: Host, claude_home: PathBuf, tick: Duration) -> Self {
        let files = std::env::temp_dir().join(format!("craft-herdr-{}", super::new_session_id()));
        let setup =
            std::fs::create_dir_all(&files).map_err(|e| ToolError::io(files.display(), e)).map(|_| Arc::new(Setup { herdr, host, claude_home, files, tick }));
        Self::build(spec, setup)
    }

    fn build(spec: CliSpec, setup: Result<Arc<Setup>, ToolError>) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let session = Arc::new(Mutex::new(spec.resume.clone()));
        HerdrConversation {
            spec,
            setup,
            tx,
            rx,
            busy: Arc::default(),
            stop: Stop::default(),
            live: Arc::default(),
            session,
            lost: Arc::default(),
            failure: Arc::default(),
        }
    }

    fn send_prompt(&mut self, text: &str, retry: bool) {
        if self.busy.swap(true, Ordering::SeqCst) {
            let _ = self.tx.send(AgentEvent::Error {
                message: "Still working on the last message.".into(),
                remediation: Some("Wait for it to finish or press stop.".into()),
                log: None,
            });
            return;
        }
        self.stop.reset();
        self.lost.store(false, Ordering::SeqCst);
        *self.failure.lock().unwrap_or_else(PoisonError::into_inner) = None;
        let setup = match &self.setup {
            Ok(s) => s.clone(),
            Err(e) => {
                let _ = self.tx.send(AgentEvent::Error { message: e.message.clone(), remediation: e.remediation.clone(), log: None });
                *self.failure.lock().unwrap_or_else(PoisonError::into_inner) = Some(TurnFailure::new(FailureKind::MissingProgram));
                end_turn(&self.busy, nothing_done(&self.spec.config.name), |e| {
                    let _ = self.tx.send(e);
                });
                return;
            }
        };
        let turn = Turn {
            spec: self.spec.clone(),
            setup,
            tx: self.tx.clone(),
            stop: self.stop.clone(),
            session: self.session.clone(),
            lost: self.lost.clone(),
            failure: self.failure.clone(),
        };
        let (live, busy, text) = (self.live.clone(), self.busy.clone(), text.to_string());
        let spawned = std::thread::Builder::new().name("ai-herdr".into()).spawn(move || {
            let mut l = live.lock().unwrap_or_else(PoisonError::into_inner);
            let done = turn.run(&mut l, &text, retry);
            drop(l);
            end_turn(&busy, done, |e| {
                let _ = turn.tx.send(e);
            });
        });
        if spawned.is_err() {
            let _ = self.tx.send(AgentEvent::Error { message: "Couldn't start a worker thread.".into(), remediation: None, log: None });
            end_turn(&self.busy, nothing_done(&self.spec.config.name), |e| {
                let _ = self.tx.send(e);
            });
        }
    }
}

/// One message's run.
struct Turn {
    spec: CliSpec,
    setup: Arc<Setup>,
    tx: Sender<AgentEvent>,
    stop: Stop,
    session: Arc<Mutex<Option<String>>>,
    lost: Arc<AtomicBool>,
    failure: Arc<Mutex<Option<TurnFailure>>>,
}

impl Turn {
    fn session(&self) -> Option<String> {
        self.session.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn set_session(&self, id: Option<String>) {
        *self.session.lock().unwrap_or_else(PoisonError::into_inner) = id;
    }

    fn say(&self, e: AgentEvent) {
        let _ = self.tx.send(e);
    }

    fn error(&self, e: ToolError, kind: FailureKind) -> AgentEvent {
        self.say(AgentEvent::Error { message: e.message, remediation: e.remediation, log: None });
        *self.failure.lock().unwrap_or_else(PoisonError::into_inner) = Some(TurnFailure::new(kind));
        nothing_done(&self.spec.config.name)
    }

    fn env(&self) -> Env<'_> {
        let s = &self.setup;
        Env { herdr: s.herdr.as_ref(), host: &s.host, claude_home: &s.claude_home, files: &s.files, tick: s.tick }
    }

    fn run(&self, live: &mut Live, text: &str, retry: bool) -> AgentEvent {
        let access = access_for(self.spec.builder.is_some(), self.spec.deep, self.spec.question);
        if let Err(e) = ClaudeCodeHerdr.validate_access(access) {
            return self.error(e, FailureKind::Configuration);
        }
        if live.started.is_none() {
            self.say(AgentEvent::Status { text: format!("Starting {DISPLAY}…") });
            match session::start(&self.env(), &self.spec, self.session().as_deref()) {
                Ok(s) => {
                    if s.fork {
                        live.forks.push(s.transcript.clone());
                    } else {
                        self.set_session(Some(s.session.clone()));
                    }
                    live.started = Some(s);
                }
                Err(StartError::Lost) => {
                    // Said before the error, so whoever reads the error can tell (as the headless run does).
                    self.lost.store(true, Ordering::SeqCst);
                    self.set_session(None);
                    let e = ToolError::invalid("Claude Code's earlier conversation about this project isn't on this computer.").with_remediation(
                        "It was started on another computer or in another folder. Ask again: a new conversation starts from what the project already has.",
                    );
                    return self.error(e, FailureKind::Configuration);
                }
                Err(StartError::Failed(e)) => return self.error(e, FailureKind::Configuration),
            }
        }
        let Some(started) = live.started.clone() else {
            return nothing_done(&self.spec.config.name);
        };
        let text = if retry && self.session().is_some() { crate::failure::CONTINUE_TURN } else { text };
        live.messages += 1;
        let sent = match self.typed(text, live.messages) {
            Ok(sent) => sent,
            Err(e) => return self.error(e, FailureKind::Configuration),
        };
        let offset = std::fs::metadata(&started.transcript).map(|m| m.len()).unwrap_or(0);
        let gate = (started.fork && offset == 0).then_some(sent.as_str());
        let mut transcript = Transcript::new(started.transcript.clone(), offset, gate);
        let done = self.watch(live, &started, &sent, &mut transcript);
        if let Some(f) = transcript.failure() {
            *self.failure.lock().unwrap_or_else(PoisonError::into_inner) = Some(f);
        }
        done
    }

    /// What is typed into the pane: the message, or for a very long one, where to read it.
    fn typed(&self, text: &str, n: u32) -> Result<String, ToolError> {
        // A leading slash would be read as one of Claude Code's own commands.
        let text = if text.starts_with('/') { format!(" {text}") } else { text.to_string() };
        if text.chars().count() <= LONGEST_TYPED {
            return Ok(text);
        }
        let file = self.setup.files.join(format!("message-{n}.md"));
        std::fs::write(&file, &text).map_err(|e| ToolError::io(file.display(), e))?;
        Ok(format!("This message is too long to type here, so it is in a file: read {} and do what it asks.", self.setup.host.to_agent(&file)))
    }

    /// Submit `sent` and follow the turn until herdr and the transcript say it's over.
    fn watch(&self, live: &mut Live, started: &Started, sent: &str, t: &mut Transcript) -> AgentEvent {
        let h = self.setup.herdr.as_ref();
        let name = &started.name;
        let mut waiting = false;
        let report_waiting = |waiting: &mut bool| {
            if !*waiting {
                *waiting = true;
                self.say(AgentEvent::Status { text: session::waiting_text(&started.label) });
            }
        };
        self.say(AgentEvent::Status { text: format!("Asking {DISPLAY}…") });
        // Waiting at a dialog (an approval left open), the prompt is refused until it is answered.
        loop {
            match client::prompt(h, name, sent) {
                Ok(()) => break,
                Err(e) if e.code == "agent_blocked" => {
                    if let Some(e) = session::refuse_trust(h, name, &started.cwd, &started.label) {
                        live.started = None;
                        return self.error(e, FailureKind::Configuration);
                    }
                    report_waiting(&mut waiting)
                }
                Err(e) if e.code == "agent_not_found" => return self.gone(live, t),
                Err(e) => return self.error(ToolError::invalid(format!("herdr couldn't send the message: {}", e.message)), FailureKind::Transient),
            }
            if self.stop.is_stopped() {
                self.say(AgentEvent::Status { text: "Stopped.".into() });
                return nothing_done(&self.spec.config.name);
            }
            std::thread::sleep(self.setup.tick);
        }
        waiting = false;
        let seq0 = client::agent_state(h, name).map(|(_, seq)| seq).unwrap_or(0);
        let limit = self.spec.options.timeout_secs.unwrap_or(if self.spec.builder.is_some() { 24 * 3600 } else { 30 * 60 });
        let deadline = Instant::now() + Duration::from_secs(limit);
        let (mut active, mut settled, mut misses, mut finished) = (false, 0u32, 0u32, false);
        loop {
            for e in t.read_new() {
                self.say(e);
            }
            if self.stop.is_stopped() {
                let _ = client::keys(h, name, &["esc"]);
                self.say(AgentEvent::Status { text: "Stopped.".into() });
                break;
            }
            if Instant::now() >= deadline {
                let _ = client::keys(h, name, &["esc"]);
                let e = ToolError::invalid(format!("{DISPLAY} didn't finish within {limit} seconds, so the turn was interrupted."));
                self.error(e, FailureKind::Transient);
                break;
            }
            match client::agent_state(h, name) {
                Err(e) if e.code == "agent_not_found" => return self.gone(live, t),
                Err(e) => {
                    misses += 1;
                    if misses >= 10 {
                        self.error(ToolError::invalid(format!("herdr stopped answering: {}", e.message)), FailureKind::Transient);
                        break;
                    }
                }
                Ok((state, seq)) => {
                    misses = 0;
                    match state {
                        AgentState::Blocked => {
                            settled = 0;
                            if !waiting && let Some(e) = session::refuse_trust(h, name, &started.cwd, &started.label) {
                                live.started = None;
                                self.error(e, FailureKind::Configuration);
                                break;
                            }
                            report_waiting(&mut waiting);
                        }
                        AgentState::Working => {
                            if waiting {
                                self.say(AgentEvent::Status { text: format!("{DISPLAY} is working again.") });
                            }
                            (settled, active, waiting) = (0, true, false);
                        }
                        s if s.settled() => {
                            settled += 1;
                            waiting = false;
                            active |= seq > seq0;
                            if t.ended || (active && ((t.answered && settled >= 2) || settled >= QUIET_LOOKS)) {
                                for e in t.read_new() {
                                    self.say(e);
                                }
                                finished = true;
                                break;
                            }
                        }
                        _ => {}
                    }
                }
            }
            std::thread::sleep(self.setup.tick);
        }
        if finished && !t.seen {
            self.say(AgentEvent::Error {
                message: format!("{DISPLAY} finished, but its transcript wasn't found at {}, so what it did can't be shown here.", t.path().display()),
                remediation: Some(format!("Look at the \"{}\" workspace in herdr.", started.label)),
                log: None,
            });
        }
        let (input_tokens, output_tokens, cost_usd) = t.totals();
        AgentEvent::TurnDone { backend: self.spec.config.name.clone(), input_tokens, output_tokens, tokens_unknown: !t.seen, cost_usd, cost_is_estimate: true }
    }

    /// Claude Code isn't running any more (closed in the pane, or it couldn't go on).
    fn gone(&self, live: &mut Live, t: &mut Transcript) -> AgentEvent {
        for e in t.read_new() {
            self.say(e);
        }
        let label = live.started.take().map(|s| s.label).unwrap_or_default();
        let e = ToolError::invalid(format!("{DISPLAY} closed before the turn finished."))
            .with_remediation(format!("Look at the \"{label}\" workspace in herdr, then send the message again."));
        self.error(e, FailureKind::Unknown)
    }
}

impl Conversation for HerdrConversation {
    fn send(&mut self, text: &str) {
        self.send_prompt(text, false);
    }

    fn retry(&mut self, text: &str) {
        self.send_prompt(text, true);
    }

    fn last_failure(&self) -> Option<TurnFailure> {
        *self.failure.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn session_id(&self) -> Option<String> {
        self.session.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn session_lost(&self) -> bool {
        self.lost.load(Ordering::SeqCst)
    }

    fn poll(&mut self) -> Vec<AgentEvent> {
        self.rx.try_iter().collect()
    }

    /// Claude Code asks the person in its pane; nothing comes here to answer.
    fn answer_consent(&mut self, _call_id: &str, _allow: bool) {}

    fn stop(&mut self) {
        self.stop.stop();
    }

    fn is_busy(&self) -> bool {
        self.busy.load(Ordering::SeqCst)
    }
}

/// Claude Code is ended and its name freed; the pane and workspace stay for the person to look
/// at. A question's copy of the builder's session isn't kept, as with the headless adapter.
impl Drop for HerdrConversation {
    fn drop(&mut self) {
        self.stop.stop();
        let Ok(setup) = &self.setup else {
            return;
        };
        for _ in 0..100 {
            if !self.busy.load(Ordering::SeqCst) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        // A turn that didn't stop in time still holds Claude Code: it is left in its pane.
        if self.busy.load(Ordering::SeqCst) {
            return;
        }
        let mut live = self.live.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(started) = live.started.take() {
            session::release(setup.herdr.as_ref(), &started);
        }
        for fork in live.forks.drain(..) {
            let _ = std::fs::remove_file(fork);
        }
        let _ = std::fs::remove_dir_all(&setup.files);
    }
}

#[cfg(test)]
#[path = "conversation_tests.rs"]
mod tests;
