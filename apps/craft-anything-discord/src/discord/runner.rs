//! Running the queue: one rebuild child at a time. Its stdout events update the board (edited at
//! most every [`EDIT_INTERVAL`]) and the "Log" thread (lines packed into 2000-character messages,
//! sent at most that often). `feature` events go to the tester cards ([`super::card`]); each check
//! and the end commit the project to the port's GitHub repo, when there is one; `turn_done` adds to
//! the job's usage. The job's status, the board and the usage are saved in the port's `bot.json`.
//! `/stop` stops the child cleanly ([`run::stop`]). The end posts the verdict, mentioning testers
//! when the port is ready.

use super::Bot;
use super::card;
use super::live::Port;
use super::scheduler::{self, Job};
use super::sync::SyncJob;
use chrono::Utc;
use craft_anything_ai::cli_agents::herdr::paths::Host;
use craft_anything_discord::board::Board;
use craft_anything_discord::coalesce::{Coalescer, EDIT_INTERVAL, MESSAGE_LIMIT, pack_lines};
use craft_anything_discord::cost::Usage;
use craft_anything_discord::events::{Event, Line, parse_line};
use craft_anything_discord::jobs::{JobKind, JobStatus};
use craft_anything_discord::log::log_line;
use craft_anything_discord::pretrust::{self, Granted};
use craft_anything_discord::run::{self, ChildEnd, Finished, How, Verdict};
use craft_anything_discord::text::clip;
use craft_anything_evidence::ToolError;
use craft_anything_platform::process::{Output, RunningProcess};
use serenity::all::{CreateAllowedMentions, CreateMessage, Http, RoleId};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// How often the loop checks the child and the edit gates.
const TICK: Duration = Duration::from_millis(500);
/// The board is re-rendered at least this often, so its elapsed time moves.
const REFRESH: Duration = Duration::from_secs(30);
/// Longest outcome message quoted in the final post.
const MESSAGE_CHARS: usize = 300;
const STOPPED_MESSAGE: &str = "Stopped with /stop.";

/// Runs `first`, then every job that queued behind it, until the queue is empty.
pub async fn run_queue(http: Arc<Http>, bot: Arc<Bot>, first: Job) {
    let mut job = first;
    loop {
        run_one(&http, &bot, &job).await;
        let next = {
            let mut queue = bot.queue.lock().await;
            let next = queue.finish();
            scheduler::save(&queue, &bot.config.ports_root);
            next
        };
        match next {
            Some(n) => job = n,
            None => break,
        }
    }
}

/// The run's own state; the board and the usage are the port's ([`super::live::Live`]), so a
/// tester's verdict shows on the board too.
struct Progress {
    finished: Option<Finished>,
    pending: Vec<String>,
    board_gate: Coalescer,
    log_gate: Coalescer,
    refreshed: Instant,
}

impl Progress {
    fn new() -> Self {
        Progress {
            finished: None,
            pending: Vec::new(),
            board_gate: Coalescer::new(EDIT_INTERVAL),
            log_gate: Coalescer::new(EDIT_INTERVAL),
            refreshed: Instant::now(),
        }
    }

    async fn take_line(&mut self, http: &Http, bot: &Bot, job: &Job, line: &str) {
        match parse_line(line) {
            Ok(Some(Line::Event(event))) => self.apply(http, bot, job, &event).await,
            Ok(Some(Line::Unknown(kind))) => log::debug!("ignored rebuild event type {kind:?}"),
            Ok(None) => {}
            Err(e) => log::warn!("the rebuild printed a line that is not an event: {e}"),
        }
    }

    async fn apply(&mut self, http: &Http, bot: &Bot, job: &Job, event: &Event) {
        if let Some(line) = log_line(event) {
            self.pending.push(line);
            self.log_gate.mark();
        }
        if let Event::Finished { ok, paused, message } = event {
            self.finished = Some(Finished { ok: *ok, paused: *paused, message: message.clone() });
        }
        fold(http, bot, &job.port, job.id, event).await;
        self.board_gate.mark();
        if let Event::Checked { round, check, passed, .. } = event {
            let verdict = if *passed { "passed" } else { "failed" };
            job.port.queue(SyncJob::Commit { message: format!("Check round {round}: {check} {verdict}") });
        }
    }

    /// Sends what is due; `force` sends everything pending now (the end of a run). The board is
    /// saved with each edit, so a restart draws it as it was last shown.
    async fn flush(&mut self, http: &Http, job: &Job, force: bool) {
        let now = Instant::now();
        if self.log_gate.is_pending() && (force || self.log_gate.is_due(now)) {
            for message in pack_lines(&std::mem::take(&mut self.pending), MESSAGE_LIMIT) {
                if let Err(e) = job.port.thread.say(http, message).await {
                    log::warn!("couldn't post to the Log thread of {}: {e}", job.port.channel);
                }
            }
            self.log_gate.sent(now);
        }
        if now.saturating_duration_since(self.refreshed) >= REFRESH {
            self.board_gate.mark();
        }
        if self.board_gate.is_pending() && (force || self.board_gate.is_due(now)) {
            {
                let live = job.port.live.lock().await;
                live.save(&job.port);
            }
            job.port.draw(http).await;
            self.board_gate.sent(now);
            self.refreshed = now;
        }
    }
}

/// Folds one event into a port: its board, the usage of its job `job_id`, and, for a `feature`
/// event, the tester cards and Plane ([`card::on_feature`]). A run's events and a mirrored app
/// project's ([`super::mirror`]) both go through here.
pub async fn fold(http: &Http, bot: &Bot, port: &Port, job_id: u64, event: &Event) {
    {
        let mut live = port.live.lock().await;
        live.state.board.get_or_insert_with(Board::new).apply(event);
        if let Event::TurnDone { input_tokens, output_tokens, tokens_unknown, cost_usd, cost_is_estimate, .. } = event {
            let mut turn = Usage::default();
            turn.add_turn(*input_tokens, *output_tokens, *tokens_unknown, *cost_usd, *cost_is_estimate);
            live.state.add_usage(job_id, &turn, Utc::now());
            live.save(port);
        }
    }
    if let Event::Feature { name, state, guide, similarity, .. } = event {
        card::on_feature(http, bot, port, name, state, guide.as_deref(), *similarity).await;
    }
}

fn opening(kind: JobKind) -> &'static str {
    match kind {
        JobKind::Port => "▶️ Starting the rebuild.",
        JobKind::Resume => "▶️ Resuming the rebuild where it stopped.",
        JobKind::Retry => "🔁 Running the request again on the same project folder.",
    }
}

async fn run_one(http: &Http, bot: &Bot, job: &Job) {
    scheduler::say(http, job.port.channel, opening(job.kind)).await;
    {
        let mut live = job.port.live.lock().await;
        live.state.start_job(job.id, Utc::now());
        live.state.board.get_or_insert_with(Board::new).restart();
        live.save(&job.port);
    }
    pretrust(http, bot, job).await;
    let project_exists = craft_anything_ai::rebuild::project::has_project(&job.port.project);
    let how = How::choose(job.kind == JobKind::Resume, project_exists);
    let request = job.target.request(&job.label);
    let args = run::rebuild_args(&job.input, &request, &job.port.project, job.target, bot.config.backend.as_deref(), how);
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let mut progress = Progress::new();
    let end = match run::spawn(&bot.config.craft_cli, args, move |line| {
        let _ = tx.send(line.to_owned());
    }) {
        Err(e) => ChildEnd { success: false, code: None, stderr: e.message },
        Ok(mut child) => {
            let mut tick = tokio::time::interval(TICK);
            loop {
                tokio::select! {
                    Some(line) = rx.recv() => progress.take_line(http, bot, job, &line).await,
                    () = job.stop.wait() => break stop_child(child).await,
                    _ = tick.tick() => {
                        if let Some(result) = child.try_result() {
                            break child_end(result);
                        }
                        progress.flush(http, job, false).await;
                    }
                }
            }
        }
    };
    while let Ok(line) = rx.try_recv() {
        progress.take_line(http, bot, job, &line).await;
    }
    let stopped = job.stop.is_requested();
    let mut verdict = run::verdict(progress.finished.as_ref(), &end);
    if stopped {
        verdict.message = STOPPED_MESSAGE.to_owned();
    }
    if !verdict.ok && (stopped || progress.finished.as_ref().is_none_or(|f| f.ok)) {
        progress.apply(http, bot, job, &Event::Finished { ok: false, paused: stopped, message: verdict.message.clone() }).await;
    }
    let status = match (stopped, verdict.ok, verdict.paused) {
        (true, _, _) => JobStatus::Stopped,
        (false, true, _) => JobStatus::Finished,
        (false, false, true) => JobStatus::Paused,
        (false, false, false) => JobStatus::Failed,
    };
    {
        let mut live = job.port.live.lock().await;
        live.state.end_job(job.id, status, Utc::now());
        live.save(&job.port);
    }
    progress.flush(http, job, true).await;
    job.port.queue(SyncJob::Commit { message: format!("Port {}: {}", status.label(), clip(&verdict.message, MESSAGE_CHARS)) });
    if stopped {
        scheduler::say(http, job.port.channel, "⏹️ Stopped. /resume carries on from where it stopped; /retry runs the request again.").await;
    } else {
        post_verdict(http, bot, job, &verdict).await;
    }
}

/// With `PRETRUST_PORTS` on and a herdr build, trusts the port's project folder in Claude Code's
/// config first ([`pretrust`]), and says so in the Log thread. A failure is said there too and the
/// build goes on: Claude Code then asks in the pane and the run stops as it would have.
async fn pretrust(http: &Http, bot: &Bot, job: &Job) {
    let config = &bot.config;
    if !config.pretrust_ports || !pretrust::uses_herdr(config.backend.as_deref()) {
        return;
    }
    let (root, dir, explicit, backend) = (config.ports_root.clone(), job.port.dir.clone(), config.claude_config_json.clone(), config.backend.clone());
    let done = tokio::task::spawn_blocking(move || {
        pretrust::maybe_pretrust(true, backend.as_deref(), &root, &dir, explicit.as_deref(), &Host::current()).map_err(|e| e.to_string())
    })
    .await
    .unwrap_or_else(|e| Err(format!("the work stopped unexpectedly: {e}")));
    let line = match done {
        Ok(Some((folder, Granted::Added))) => format!("🔓 Trusted {folder} for Claude Code (PRETRUST_PORTS), so its trust question doesn't stop the build."),
        Ok(Some((folder, Granted::AlreadyTrusted))) => {
            log::debug!("{folder} was already trusted in Claude Code's config");
            return;
        }
        Ok(None) => return,
        Err(e) => format!("⚠️ Couldn't trust the project folder for Claude Code (PRETRUST_PORTS): {e}"),
    };
    log::info!("{}: {line}", job.port.slug);
    if let Err(e) = job.port.thread.say(http, clip(&line, MESSAGE_LIMIT)).await {
        log::warn!("couldn't post to the Log thread of {}: {e}", job.port.channel);
    }
}

/// `/stop`: the child is asked to stop, then killed after the grace, off the async threads.
async fn stop_child(mut child: RunningProcess) -> ChildEnd {
    let stopped = tokio::task::spawn_blocking(move || {
        let confirmed = run::stop(&mut child);
        (confirmed, child.try_result().map(child_end))
    })
    .await;
    match stopped {
        Ok((_, Some(end))) => end,
        Ok((confirmed, None)) => {
            log::warn!("the rebuild was asked to stop; its end was confirmed: {confirmed}");
            ChildEnd { success: false, code: None, stderr: "the rebuild did not report how it ended after /stop".to_owned() }
        }
        Err(e) => ChildEnd { success: false, code: None, stderr: format!("stopping the rebuild failed: {e}") },
    }
}

fn child_end(result: &Result<Output, ToolError>) -> ChildEnd {
    match result {
        Ok(out) => ChildEnd { success: out.success, code: out.code, stderr: out.stderr_end_text() },
        Err(e) => ChildEnd { success: false, code: None, stderr: e.message.clone() },
    }
}

async fn post_verdict(http: &Http, bot: &Bot, job: &Job, verdict: &Verdict) {
    let said = clip(&verdict.message, MESSAGE_CHARS);
    let message = if verdict.mention_testers {
        let testers = RoleId::new(bot.config.tester_role_id);
        CreateMessage::new()
            .content(format!("<@&{testers}> ✅ **{}** is ready to test: {said}", job.label))
            .allowed_mentions(CreateAllowedMentions::new().roles(vec![testers]))
    } else {
        let icon = if verdict.paused { "⏸️ Port paused" } else { "❌ Port failed" };
        let mut text = format!("{icon}: {said}");
        if let Some(tail) = &verdict.stderr_tail {
            text.push_str(&format!("\n```\n{}\n```", tail.replace("```", "'''")));
        }
        CreateMessage::new().content(text).allowed_mentions(CreateAllowedMentions::new())
    };
    if let Err(e) = job.port.channel.send_message(http, message).await {
        log::warn!("couldn't post the result in {}: {e}", job.port.channel);
    }
}
