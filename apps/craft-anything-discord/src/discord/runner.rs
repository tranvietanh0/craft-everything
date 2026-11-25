//! Running the queue: one rebuild child at a time. Its stdout events update the board (edited at
//! most every [`EDIT_INTERVAL`]) and the "Log" thread (lines packed into 2000-character messages,
//! sent at most that often). The end posts the verdict, mentioning testers when the port is ready.

use super::Bot;
use super::embeds;
use craft_anything_discord::board::Board;
use craft_anything_discord::coalesce::{Coalescer, EDIT_INTERVAL, MESSAGE_LIMIT, pack_lines};
use craft_anything_discord::events::{Event, Line, parse_line};
use craft_anything_discord::log::log_line;
use craft_anything_discord::run::{self, ChildEnd, Finished, Target, Verdict};
use craft_anything_discord::text::clip;
use craft_anything_evidence::ToolError;
use craft_anything_platform::process::Output;
use serenity::all::{ChannelId, CreateAllowedMentions, CreateMessage, EditMessage, Http, MessageId, RoleId};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// How often the loop checks the child and the edit gates.
const TICK: Duration = Duration::from_millis(500);
/// The board is re-rendered at least this often, so its elapsed time moves.
const REFRESH: Duration = Duration::from_secs(30);
/// Longest outcome message quoted in the final post.
const MESSAGE_CHARS: usize = 300;

/// One queued port: where it reports and what it rebuilds.
#[derive(Debug, Clone)]
pub struct Job {
    pub channel: ChannelId,
    pub board_message: MessageId,
    pub thread: ChannelId,
    pub input: PathBuf,
    pub project: PathBuf,
    pub target: Target,
    pub label: String,
}

/// Runs `first`, then every job that queued behind it, until the queue is empty.
pub async fn run_queue(http: Arc<Http>, bot: Arc<Bot>, first: Job) {
    let mut job = first;
    loop {
        run_one(&http, &bot, &job).await;
        let next = bot.queue.lock().await.finish();
        match next {
            Some(n) => job = n,
            None => break,
        }
    }
}

struct Progress {
    board: Board,
    finished: Option<Finished>,
    pending: Vec<String>,
    board_gate: Coalescer,
    log_gate: Coalescer,
    refreshed: Instant,
    started: Instant,
}

impl Progress {
    fn new() -> Self {
        let now = Instant::now();
        Progress {
            board: Board::new(),
            finished: None,
            pending: Vec::new(),
            board_gate: Coalescer::new(EDIT_INTERVAL),
            log_gate: Coalescer::new(EDIT_INTERVAL),
            refreshed: now,
            started: now,
        }
    }

    fn take_line(&mut self, line: &str) {
        match parse_line(line) {
            Ok(Some(Line::Event(event))) => self.apply(&event),
            Ok(Some(Line::Unknown(kind))) => log::debug!("ignored rebuild event type {kind:?}"),
            Ok(None) => {}
            Err(e) => log::warn!("the rebuild printed a line that is not an event: {e}"),
        }
    }

    fn apply(&mut self, event: &Event) {
        if let Some(line) = log_line(event) {
            self.pending.push(line);
            self.log_gate.mark();
        }
        if let Event::Finished { ok, paused, message } = event {
            self.finished = Some(Finished { ok: *ok, paused: *paused, message: message.clone() });
        }
        self.board.apply(event);
        self.board_gate.mark();
    }

    /// Sends what is due; `force` sends everything pending now (the end of a run).
    async fn flush(&mut self, http: &Http, job: &Job, force: bool) {
        let now = Instant::now();
        if self.log_gate.is_pending() && (force || self.log_gate.is_due(now)) {
            for message in pack_lines(&std::mem::take(&mut self.pending), MESSAGE_LIMIT) {
                if let Err(e) = job.thread.say(http, message).await {
                    log::warn!("couldn't post to the Log thread of {}: {e}", job.channel);
                }
            }
            self.log_gate.sent(now);
        }
        if now.saturating_duration_since(self.refreshed) >= REFRESH {
            self.board_gate.mark();
        }
        if self.board_gate.is_pending() && (force || self.board_gate.is_due(now)) {
            let view = self.board.render(now.saturating_duration_since(self.started));
            let edit = EditMessage::new().embed(embeds::board_embed(&job.label, &view));
            if let Err(e) = job.channel.edit_message(http, job.board_message, edit).await {
                log::warn!("couldn't update the board in {}: {e}", job.channel);
            }
            self.board_gate.sent(now);
            self.refreshed = now;
        }
    }
}

async fn run_one(http: &Http, bot: &Bot, job: &Job) {
    if let Err(e) = job.channel.say(http, "▶️ Starting the rebuild.").await {
        log::warn!("couldn't post in {}: {e}", job.channel);
    }
    let args = run::rebuild_args(&job.input, &job.target.request(&job.label), &job.project, job.target);
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
                    Some(line) = rx.recv() => progress.take_line(&line),
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
        progress.take_line(&line);
    }
    let verdict = run::verdict(progress.finished.as_ref(), &end);
    if !verdict.ok && progress.finished.as_ref().is_none_or(|f| f.ok) {
        progress.apply(&Event::Finished { ok: false, paused: false, message: verdict.message.clone() });
    }
    progress.flush(http, job, true).await;
    post_verdict(http, bot, job, &verdict).await;
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
    if let Err(e) = job.channel.send_message(http, message).await {
        log::warn!("couldn't post the result in {}: {e}", job.channel);
    }
}
