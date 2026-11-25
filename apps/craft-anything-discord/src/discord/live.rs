//! A port the bot knows in memory: where it posts, its saved state (`bot.json`), its board while
//! the bot has one, and the queue of its Plane and GitHub work. After a restart a port is loaded
//! again from its `bot.json` the first time one of its cards is pressed (it then has no board).

use super::Bot;
use super::sync::{self, SyncJob};
use craft_anything_discord::board::Board;
use craft_anything_discord::intake;
use craft_anything_discord::state::PortState;
use serenity::all::{ChannelId, Http, MessageId};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, mpsc};

pub struct Port {
    /// `PORTS_ROOT/<slug>`, where `bot.json` is.
    pub dir: PathBuf,
    /// `PORTS_ROOT/<slug>/project`, the rebuild's project folder.
    pub project: PathBuf,
    pub channel: ChannelId,
    pub thread: ChannelId,
    pub live: Mutex<Live>,
    sync: mpsc::UnboundedSender<SyncJob>,
}

pub struct Live {
    pub state: PortState,
    /// The board as the run left it; `None` for a port loaded again after a restart.
    pub board: Option<Board>,
    pub started: Option<Instant>,
    pub ended: Option<Duration>,
}

impl Live {
    /// Saves `bot.json`; a failure is logged (the port goes on with what is in memory).
    pub fn save(&self, port: &Port) {
        if let Err(e) = self.state.save(&port.dir) {
            log::warn!("{e}");
        }
    }

    /// The time the board shows.
    pub fn elapsed(&self) -> Duration {
        self.ended.or_else(|| self.started.map(|s| s.elapsed())).unwrap_or_default()
    }

    pub fn board_message(&self) -> MessageId {
        MessageId::new(self.state.board_message.max(1))
    }
}

impl Port {
    /// Makes the port (its folder is `PORTS_ROOT/<slug>`) and starts its Plane/GitHub queue.
    pub fn start(http: Arc<Http>, bot: &Arc<Bot>, state: PortState, board: Option<Board>) -> Arc<Port> {
        let (tx, rx) = mpsc::unbounded_channel();
        let root = &bot.config.ports_root;
        let port = Arc::new(Port {
            project: intake::project_dir(root, &state.slug),
            dir: root.join(&state.slug),
            channel: ChannelId::new(state.channel.max(1)),
            thread: ChannelId::new(state.thread.max(1)),
            live: Mutex::new(Live { state, board, started: None, ended: None }),
            sync: tx,
        });
        tokio::spawn(sync::run(http, Arc::clone(&bot.services), Arc::clone(&port), rx));
        port
    }

    /// Queues Plane or GitHub work; it runs in order, off the run's own loop.
    pub fn queue(&self, job: SyncJob) {
        if self.sync.send(job).is_err() {
            log::warn!("the Plane/GitHub queue of {} has stopped", self.channel);
        }
    }
}

/// The port whose channel is `channel`: in memory, else loaded from its `bot.json`.
pub async fn find(http: &Arc<Http>, bot: &Arc<Bot>, channel: ChannelId) -> Option<Arc<Port>> {
    let mut ports = bot.ports.lock().await;
    if let Some(port) = ports.get(&channel) {
        return Some(Arc::clone(port));
    }
    let root = bot.config.ports_root.clone();
    let found = tokio::task::spawn_blocking(move || PortState::find_by_channel(&root, channel.get())).await.ok().flatten()?;
    let (_, state) = found;
    let port = Port::start(Arc::clone(http), bot, state, None);
    ports.insert(channel, Arc::clone(&port));
    Some(port)
}
