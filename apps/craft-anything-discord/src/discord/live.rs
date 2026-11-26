//! A port the bot knows in memory: where it posts, its saved state (`bot.json`, with its board and
//! jobs), and the queue of its Plane and GitHub work. After a restart a port is loaded again from
//! its `bot.json` the first time a command, a card press or the saved queue needs it.

use super::Bot;
use super::embeds;
use super::sync::{self, SyncJob};
use chrono::Utc;
use craft_anything_discord::intake;
use craft_anything_discord::state::PortState;
use serenity::all::{ChannelId, CreateEmbed, EditMessage, Http, MessageId};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

pub struct Port {
    pub slug: String,
    /// `PORTS_ROOT/<slug>`, where `bot.json` is.
    pub dir: PathBuf,
    /// `PORTS_ROOT/<slug>/project`, the rebuild's project folder; a mirrored app project's own folder.
    pub project: PathBuf,
    /// Mirrored from the craft desktop app ([`super::mirror`]): its runs are the app's, and it has
    /// no GitHub repo.
    pub app_owned: bool,
    pub channel: ChannelId,
    pub thread: ChannelId,
    pub live: Mutex<Live>,
    sync: mpsc::UnboundedSender<SyncJob>,
}

pub struct Live {
    pub state: PortState,
}

impl Live {
    /// Saves `bot.json`; a failure is logged (the port goes on with what is in memory).
    pub fn save(&self, port: &Port) {
        if let Err(e) = self.state.save(&port.dir) {
            log::warn!("{e}");
        }
    }

    /// The board embed as the port's state has it now, when it has a board.
    pub fn board_embed(&self) -> Option<CreateEmbed> {
        embeds::port_board(&self.state, Utc::now())
    }
}

/// Edits a port's pinned board message with `embed`. The same message is kept for every run.
pub async fn draw_board(http: &Http, channel: ChannelId, board_message: u64, embed: CreateEmbed) {
    if board_message == 0 {
        return;
    }
    if let Err(e) = channel.edit_message(http, MessageId::new(board_message), EditMessage::new().embed(embed)).await {
        log::warn!("couldn't update the board in {channel}: {e}");
    }
}

impl Port {
    /// Makes the port (its folder is `PORTS_ROOT/<slug>`) and starts its Plane/GitHub queue.
    pub fn start(http: Arc<Http>, bot: &Arc<Bot>, state: PortState) -> Arc<Port> {
        let (tx, rx) = mpsc::unbounded_channel();
        let root = &bot.config.ports_root;
        let port = Arc::new(Port {
            slug: state.slug.clone(),
            project: state.app_folder.clone().unwrap_or_else(|| intake::project_dir(root, &state.slug)),
            app_owned: state.app_folder.is_some(),
            dir: root.join(&state.slug),
            channel: ChannelId::new(state.channel.max(1)),
            thread: ChannelId::new(state.thread.max(1)),
            live: Mutex::new(Live { state }),
            sync: tx,
        });
        tokio::spawn(sync::run(http, Arc::clone(&bot.services), Arc::clone(&port), rx));
        port
    }

    /// Queues Plane or GitHub work; it runs in order, off the run's own loop.
    /// Redraws the pinned board from the port's state.
    pub async fn draw(&self, http: &Http) {
        let (embed, message) = {
            let live = self.live.lock().await;
            (live.board_embed(), live.state.board_message)
        };
        if let Some(embed) = embed {
            draw_board(http, self.channel, message, embed).await;
        }
    }

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
    let port = Port::start(Arc::clone(http), bot, state);
    ports.insert(channel, Arc::clone(&port));
    Some(port)
}
