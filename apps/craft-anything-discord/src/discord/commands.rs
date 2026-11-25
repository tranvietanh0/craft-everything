//! The porter's control commands in a port's channel: `/status`, `/stop`, `/resume`, `/retry` and
//! `/note`. Only members with the porter role may use them; every answer is private, and an
//! action also gets one public line in the channel. The rules are in
//! [`craft_anything_discord::control`].

use super::Bot;
use super::command::{self, NOTE, RESUME, RETRY, STATUS, STOP};
use super::live::{self, Port};
use super::scheduler::{self, Job};
use chrono::Utc;
use craft_anything_discord::control::{self, Place};
use craft_anything_discord::jobs::{JobKind, JobStatus};
use craft_anything_discord::review;
use craft_anything_discord::run;
use craft_anything_discord::text::clip;
use serenity::all::{CommandInteraction, Context, CreateInteractionResponse, CreateInteractionResponseMessage};
use std::sync::Arc;

const MESSAGE_LIMIT: usize = 2_000;

pub async fn handle(ctx: &Context, cmd: &CommandInteraction, bot: &Arc<Bot>) {
    let name = cmd.data.name.as_str();
    let roles: Vec<u64> = cmd.member.as_ref().map(|m| m.roles.iter().map(|r| r.get()).collect()).unwrap_or_default();
    if !control::may_control(&roles, bot.config.porter_role_id) {
        reply(ctx, cmd, &format!("Only members with the porter role can use /{name}.")).await;
        return;
    }
    let Some(port) = live::find(&ctx.http, bot, cmd.channel_id).await else {
        reply(ctx, cmd, &format!("Use /{name} in a port's channel.")).await;
        return;
    };
    let answer = match name {
        STATUS => status(bot, &port).await,
        STOP => stop(ctx, bot, &port).await,
        RESUME => requeue(ctx, bot, &port, JobKind::Resume).await,
        RETRY => requeue(ctx, bot, &port, JobKind::Retry).await,
        NOTE => note(ctx, cmd, &port).await,
        _ => return,
    };
    reply(ctx, cmd, &answer).await;
}

async fn reply(ctx: &Context, cmd: &CommandInteraction, text: &str) {
    let message = CreateInteractionResponseMessage::new().content(clip(text, MESSAGE_LIMIT)).ephemeral(true);
    if let Err(e) = cmd.create_response(&ctx.http, CreateInteractionResponse::Message(message)).await {
        log::warn!("couldn't answer /{}: {e}", cmd.data.name);
    }
}

fn is_port(job: &Job, port: &Port) -> bool {
    job.port.channel == port.channel
}

async fn status(bot: &Bot, port: &Port) -> String {
    let place = {
        let queue = bot.queue.lock().await;
        match (queue.running().is_some_and(|j| is_port(j, port)), queue.position(|j| is_port(j, port))) {
            (true, _) => Place::Running,
            (false, Some(n)) => Place::Waiting(n),
            (false, None) => Place::NotQueued,
        }
    };
    let live = port.live.lock().await;
    let elapsed = live.state.last_job().map(|j| j.elapsed(Utc::now())).unwrap_or_default();
    control::status_text(&live.state, place, elapsed)
}

async fn stop(ctx: &Context, bot: &Bot, port: &Port) -> String {
    let mut queue = bot.queue.lock().await;
    if let Some(job) = queue.running().filter(|j| is_port(j, port)) {
        job.stop.request();
        drop(queue);
        scheduler::say(&ctx.http, port.channel, "⏹️ Stopping the build: the builder is asked to stop first.").await;
        return format!("Stopping: the builder gets up to {} seconds to stop before it is ended.", run::STOP_GRACE.as_secs());
    }
    let Some(job) = queue.remove_waiting(|j| is_port(j, port)) else {
        drop(queue);
        let live = port.live.lock().await;
        return control::stoppable(&live.state).err().unwrap_or_else(|| "This port's job is not in the queue any more.".to_owned());
    };
    scheduler::save(&queue, &bot.config.ports_root);
    drop(queue);
    {
        let mut live = port.live.lock().await;
        live.state.end_job(job.id, JobStatus::Stopped, Utc::now());
        live.save(port);
    }
    scheduler::say(&ctx.http, port.channel, "⏹️ Taken out of the queue before it started.").await;
    "Taken out of the queue. /resume or /retry queues it again.".to_owned()
}

/// `/resume` or `/retry`: checks the port's last job and queues a new one in the same step.
async fn requeue(ctx: &Context, bot: &Arc<Bot>, port: &Arc<Port>, kind: JobKind) -> String {
    let project_exists = craft_anything_ai::rebuild::project::has_project(&port.project);
    let job = {
        let mut live = port.live.lock().await;
        let checked = match kind {
            JobKind::Resume => control::resumable(&live.state, project_exists),
            _ => control::retryable(&live.state),
        };
        let (input, target) = match checked {
            Ok(last) => (last.input.clone(), last.target),
            Err(why) => return why,
        };
        Job::record(port, &mut live, kind, input, target)
    };
    let what = if kind == JobKind::Resume { "Resume" } else { "Retry" };
    let text = match scheduler::enqueue(&ctx.http, bot, job).await {
        None => format!("🔁 {what} queued: it starts now."),
        Some(position) => format!("🔁 {what} queued at position {position}."),
    };
    scheduler::say(&ctx.http, port.channel, &text).await;
    text
}

async fn note(ctx: &Context, cmd: &CommandInteraction, port: &Port) -> String {
    let Some(text) = command::note_text(cmd) else {
        return "Write the note after /note.".to_owned();
    };
    let porter = cmd.member.as_ref().map_or_else(|| cmd.user.display_name().to_owned(), |m| m.display_name().to_owned());
    let (project, body) = (port.project.clone(), control::porter_note(&porter, &text));
    let left = tokio::task::spawn_blocking(move || review::leave_builder_note(&project, &body)).await.map_err(|e| e.to_string()).and_then(|r| r);
    if let Err(e) = left {
        return format!("❌ Couldn't leave the note: {e}");
    }
    // No mentions: the note is the porter's own words.
    scheduler::say(&ctx.http, port.thread, &clip(&format!("📝 {porter} left the builder a note: {text}"), MESSAGE_LIMIT)).await;
    "📝 Left for the builder: a running build reads it at its next step; otherwise /resume starts with it.".to_owned()
}
