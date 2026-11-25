//! The rebuild queue's Discord side: a job and its stop signal, adding a job (the line is saved
//! after every change, `PORTS_ROOT/.bot/queue.json`), and what the bot does with the saved line
//! when it starts: the jobs that were waiting are queued again, the one that was running is marked
//! interrupted and its port offered `/resume`, and every port's board is drawn again.

use super::Bot;
use super::live::{self, Live, Port};
use super::runner;
use chrono::Utc;
use craft_anything_discord::jobs::JobKind;
use craft_anything_discord::queue::{Admission, RebuildQueue};
use craft_anything_discord::run::Target;
use craft_anything_discord::saved_queue::{Entry, Recovery, SavedQueue};
use craft_anything_discord::state::PortState;
use serenity::all::{ChannelId, CreateAllowedMentions, CreateMessage, Http};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::watch;

/// Asks a running job to stop. Cloned with the job, so the queue's copy reaches the runner's.
pub struct Stop(watch::Sender<bool>);

impl Default for Stop {
    fn default() -> Self {
        Stop(watch::channel(false).0)
    }
}

impl Stop {
    pub fn request(&self) {
        self.0.send_replace(true);
    }

    pub fn is_requested(&self) -> bool {
        *self.0.borrow()
    }

    /// Ends once a stop is requested (at once when it already was).
    pub async fn wait(&self) {
        let mut rx = self.0.subscribe();
        let closed = rx.wait_for(|stop| *stop).await.is_err();
        if closed {
            // The sender lives in `self`, so this can't end; never report a stop that wasn't asked.
            std::future::pending::<()>().await;
        }
    }
}

/// One queued run: where it reports, what it rebuilds and how.
#[derive(Clone)]
pub struct Job {
    pub port: Arc<Port>,
    /// The job's id in the port's `bot.json`.
    pub id: u64,
    pub kind: JobKind,
    pub input: PathBuf,
    pub target: Target,
    pub label: String,
    pub stop: Arc<Stop>,
}

impl Job {
    fn entry(&self) -> Entry {
        Entry {
            slug: self.port.slug.clone(),
            channel: self.port.channel.get(),
            job: self.id,
            kind: self.kind,
            input: self.input.clone(),
            target: self.target,
            label: self.label.clone(),
        }
    }

    /// Records a new job for `port` in its `bot.json`, waiting to run. `live` is the port's
    /// locked state, so a caller can check and record in one step.
    pub fn record(port: &Arc<Port>, live: &mut Live, kind: JobKind, input: PathBuf, target: Target) -> Job {
        let id = live.state.new_job(kind, input.clone(), target, Utc::now());
        let label = live.state.label.clone();
        live.save(port);
        Job { port: Arc::clone(port), id, kind, input, target, label, stop: Arc::new(Stop::default()) }
    }
}

/// Saves the line as it is now; called with the queue locked, so saves keep the queue's order.
pub fn save(queue: &RebuildQueue<Job>, ports_root: &Path) {
    let saved = SavedQueue { running: queue.running().map(Job::entry), waiting: queue.waiting_jobs().map(Job::entry).collect() };
    if let Err(e) = saved.save(ports_root) {
        log::error!("{e}; the queue would not survive a restart");
    }
}

/// Queues `job`. `None`: it started now; `Some(position)`: it waits there.
pub async fn enqueue(http: &Arc<Http>, bot: &Arc<Bot>, job: Job) -> Option<usize> {
    let admission = {
        let mut queue = bot.queue.lock().await;
        let admission = queue.submit(job);
        save(&queue, &bot.config.ports_root);
        admission
    };
    match admission {
        Admission::Start(job) => {
            tokio::spawn(runner::run_queue(Arc::clone(http), Arc::clone(bot), job));
            None
        }
        Admission::Queued { position } => Some(position),
    }
}

pub async fn say(http: &Http, channel: ChannelId, text: &str) {
    let message = CreateMessage::new().content(text).allowed_mentions(CreateAllowedMentions::new());
    if let Err(e) = channel.send_message(http, message).await {
        log::warn!("couldn't post in {channel}: {e}");
    }
}

/// Reads the saved queue before the bot signs in, so no new job's save can overwrite it first.
pub fn load_saved(ports_root: &Path) -> Recovery {
    let (saved, error) = SavedQueue::load_for_restart(ports_root);
    if let Some(e) = error {
        log::error!("{e}");
    }
    saved.recover()
}

/// What the bot does with the saved queue once it has signed in.
pub async fn recover(http: &Arc<Http>, bot: &Arc<Bot>, recovery: Recovery) {
    if let Some(entry) = &recovery.interrupted {
        interrupt(http, bot, entry).await;
    }
    redraw_all(http, bot).await;
    for entry in recovery.requeue {
        let Some(port) = live::find(http, bot, ChannelId::new(entry.channel.max(1))).await else {
            log::warn!("the queued port {} has no bot.json any more; it was dropped from the queue", entry.slug);
            continue;
        };
        let channel = port.channel;
        let job = Job { port, id: entry.job, kind: entry.kind, input: entry.input, target: entry.target, label: entry.label, stop: Arc::new(Stop::default()) };
        let text = match enqueue(http, bot, job).await {
            None => "🔁 The bot restarted; this port's job starts again now.".to_owned(),
            Some(position) => format!("🔁 The bot restarted; this port's job is back in the queue at position {position}."),
        };
        say(http, channel, &text).await;
    }
    save(&*bot.queue.lock().await, &bot.config.ports_root);
}

/// The job that was running when the bot stopped: interrupted, not started again.
async fn interrupt(http: &Arc<Http>, bot: &Arc<Bot>, entry: &Entry) {
    let Some(port) = live::find(http, bot, ChannelId::new(entry.channel.max(1))).await else {
        log::warn!("the interrupted port {} has no bot.json any more", entry.slug);
        return;
    };
    let changed = {
        let mut live = port.live.lock().await;
        let changed = live.state.mark_interrupted(entry.job, Utc::now());
        live.save(&port);
        changed
    };
    if changed {
        say(
            http,
            port.channel,
            "⚠️ The bot restarted while this port was running, so the run was interrupted. It is not restarted on its own: \
             use /resume to carry on from where it stopped, or /retry to run the request again.",
        )
        .await;
    }
}

/// Draws every port's pinned board again from its `bot.json`, editing the same message.
async fn redraw_all(http: &Http, bot: &Bot) {
    let root = bot.config.ports_root.clone();
    let states = match tokio::task::spawn_blocking(move || PortState::load_all(&root)).await {
        Ok(states) => states,
        Err(e) => {
            log::warn!("couldn't read the ports to draw their boards again: {e}");
            return;
        }
    };
    for state in states {
        if let Some(embed) = super::embeds::port_board(&state, Utc::now()) {
            live::draw_board(http, ChannelId::new(state.channel.max(1)), state.board_message, embed).await;
        }
    }
}
