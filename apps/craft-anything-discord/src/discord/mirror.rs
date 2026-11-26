//! Mirroring projects made in the craft desktop app ([`craft_anything_discord::mirror`]). Every
//! `MIRROR_POLL_SECS`: each new registration gets its channel at once ("(from app)", with its
//! pinned intro and board and a "Log" thread), saved in `PORTS_ROOT/.bot/mirrors.json` before the
//! registration is moved to `done/`. Then every mirrored project's changed files become events,
//! folded into the port like a run's ([`runner::fold`]): the board, the usage, the tester cards and
//! Plane. The first report is a light look at what is rebuilt ([`first_look`]), once the study
//! wrote its notes.

use super::Bot;
use super::embeds;
use super::live::{self, Port};
use super::port::{pin, reserve_slug};
use super::runner;
use super::scheduler;
use super::sync::SyncJob;
use chrono::Utc;
use craft_anything_discord::board::Board;
use craft_anything_discord::coalesce::{MESSAGE_LIMIT, pack_lines};
use craft_anything_discord::cost::Usage;
use craft_anything_discord::events::Event;
use craft_anything_discord::first_look;
use craft_anything_discord::jobs::{JobKind, JobStatus};
use craft_anything_discord::log::log_line;
use craft_anything_discord::mirror::{self, Admit, Mirror, Mirrors, Poll, Registration, Seen};
use craft_anything_discord::run::Target;
use craft_anything_discord::slug;
use craft_anything_discord::state::PortState;
use craft_anything_discord::text::clip;
use serenity::all::{ChannelId, ChannelType, CreateChannel, CreateMessage, CreateThread, GuildId, Http};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Marks a mirrored project's channel and board.
const FROM_APP: &str = "(from app)";

async fn blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    tokio::task::spawn_blocking(work).await.map_err(|e| format!("the work stopped unexpectedly: {e}"))
}

/// Runs for the bot's life. A `mirrors.json` that can't be read turns mirroring off (logged):
/// going on without it would make every project's channel again.
pub async fn run(http: Arc<Http>, bot: Arc<Bot>) {
    let root = bot.config.ports_root.clone();
    let dir = mirror::register_dir(&root, bot.config.mirror_register_dir.as_deref());
    let load_root = root.clone();
    let mut mirrors = match blocking(move || Mirrors::load(&load_root)).await.and_then(|r| r) {
        Ok(m) => m,
        Err(e) => {
            log::error!("{e}; app projects are not mirrored until it is fixed");
            return;
        }
    };
    log::info!("mirroring app projects registered in {} ({} known)", dir.display(), mirrors.projects.len());
    let mut tick = tokio::time::interval(bot.config.mirror_poll);
    loop {
        tick.tick().await;
        let mut changed = admit_new(&http, &bot, &dir, &mut mirrors).await;
        for m in &mut mirrors.projects {
            changed |= follow(&http, &bot, m).await;
        }
        if changed {
            let (save_root, saved) = (root.clone(), mirrors.clone());
            if let Err(e) = blocking(move || saved.save(&save_root)).await.and_then(|r| r) {
                log::error!("{e}; a restart may report some changes again");
            }
        }
    }
}

/// Handles the waiting registrations. Returns whether `mirrors` changed (it is saved already).
async fn admit_new(http: &Arc<Http>, bot: &Arc<Bot>, dir: &Path, mirrors: &mut Mirrors) -> bool {
    let scan = dir.to_path_buf();
    let files = blocking(move || mirror::pending(&scan)).await.unwrap_or_default();
    let mut changed = false;
    for file in files {
        let read = file.clone();
        let reg = match blocking(move || Registration::read(&read)).await.and_then(|r| r) {
            Ok(reg) => reg,
            Err(e) => {
                log::warn!("{e}; it is set aside in done/");
                done(file).await;
                continue;
            }
        };
        let folder = reg.folder_path();
        match mirror::admit(mirrors, &bot.config.ports_root, &folder) {
            Admit::Known => {}
            Admit::Skip(why) => log::info!("not mirroring {}: {why}", folder.display()),
            Admit::New => match open_channel(http, bot, &reg, &folder).await {
                Ok(m) => {
                    log::info!("mirroring {} in channel {}", folder.display(), m.channel);
                    mirrors.projects.push(m);
                    changed = true;
                    // Saved before the registration goes, so a crash in between can't make a second channel.
                    let (root, saved) = (bot.config.ports_root.clone(), mirrors.clone());
                    if let Err(e) = blocking(move || saved.save(&root)).await.and_then(|r| r) {
                        log::error!("{e}; the registration is kept and tried again");
                        continue;
                    }
                }
                Err(e) => {
                    log::warn!("couldn't make the channel for {}: {e}; tried again next time", folder.display());
                    continue;
                }
            },
        }
        done(file).await;
    }
    changed
}

async fn done(file: PathBuf) {
    if let Err(e) = blocking(move || mirror::finish(&file)).await.and_then(|r| r) {
        log::warn!("{e}");
    }
}

/// The channel, its pinned intro and board, the Log thread, and the port's `bot.json`.
async fn open_channel(http: &Arc<Http>, bot: &Arc<Bot>, reg: &Registration, folder: &Path) -> Result<Mirror, String> {
    let config = &bot.config;
    let guild = GuildId::new(config.guild_id);
    let channels = guild.channels(http).await.map_err(|e| format!("couldn't list the server's channels: {e}"))?;
    let taken: Vec<String> = channels.values().map(|c| c.name.clone()).collect();
    let (root, base) = (config.ports_root.clone(), slug::slugify(&reg.name));
    let (name, slug) = blocking(move || reserve_slug(&root, &base, taken)).await??;
    let label = format!("{} {FROM_APP}", reg.name);
    let topic = clip(&format!("{FROM_APP} Built in the craft desktop app · {}", folder.display()), 1_024);
    let create = CreateChannel::new(&name).kind(ChannelType::Text).category(ChannelId::new(config.ports_category_id)).topic(topic);
    let channel = guild.create_channel(http, create).await.map_err(|e| format!("couldn't create #{name}: {e}"))?.id;
    let intro = CreateMessage::new().content(clip(&intro_text(reg, folder), MESSAGE_LIMIT));
    let intro = channel.send_message(http, intro).await.map_err(|e| format!("couldn't post the intro: {e}"))?;
    pin(http, channel, intro.id).await;
    let board = CreateMessage::new().embed(embeds::board_embed(&label, &Board::new().render(Duration::ZERO, Usage::default().describe())));
    let board = channel.send_message(http, board).await.map_err(|e| format!("couldn't post the progress board: {e}"))?;
    pin(http, channel, board.id).await;
    let thread = channel
        .create_thread(http, CreateThread::new("Log").kind(ChannelType::PublicThread))
        .await
        .map_err(|e| format!("couldn't create the Log thread: {e}"))?
        .id;
    let mut state = PortState {
        slug: slug.clone(),
        label,
        channel: channel.get(),
        thread: thread.get(),
        board_message: board.id.get(),
        profile_message: intro.id.get(),
        board: Some(Board::new()),
        app_folder: Some(folder.to_path_buf()),
        ..PortState::default()
    };
    let job = state.new_job(JobKind::Port, folder.to_path_buf(), Target::App, Utc::now());
    state.start_job(job, Utc::now());
    if let Err(e) = state.save(&config.ports_root.join(&slug)) {
        log::warn!("{e}");
    }
    let port = Port::start(Arc::clone(http), bot, state);
    bot.ports.lock().await.insert(channel, Arc::clone(&port));
    // Plane as for /port; no GitHub repo (the folder is the person's own).
    port.queue(SyncJob::Setup);
    Ok(Mirror { folder: folder.to_path_buf(), slug, channel: channel.get(), seen: Seen::default() })
}

fn intro_text(reg: &Registration, folder: &Path) -> String {
    let target = if reg.target.is_empty() { reg.kind.clone() } else { format!("{} · {}", reg.kind, reg.target) };
    format!(
        "📱 **{}** {FROM_APP}: a project made in the craft desktop app, mirrored here.\nFolder: `{}`\nTarget: {target}\n\
         It is built, stopped and resumed in the app. Here the board, feature progress and tester cards follow it; /status and /note work.",
        reg.name,
        folder.display()
    )
}

/// Looks at one mirrored project's files and shows what changed. Returns whether `m.seen` changed.
async fn follow(http: &Arc<Http>, bot: &Arc<Bot>, m: &mut Mirror) -> bool {
    let Some(port) = live::find(http, bot, ChannelId::new(m.channel.max(1))).await else {
        log::warn!("the mirrored project {} has no bot.json any more", m.folder.display());
        return false;
    };
    let (folder, mut seen) = (m.folder.clone(), m.seen.clone());
    let Ok((seen, found)) = blocking(move || {
        let found = mirror::poll(&folder, &mut seen);
        (seen, found)
    })
    .await
    else {
        return false;
    };
    show(http, bot, &port, &found).await;
    let mut seen = seen;
    if first_look::due(&seen) && post_first_look(http, &port, &m.folder, &seen).await {
        seen.first_look_posted = true;
    }
    let changed = seen != m.seen;
    m.seen = seen;
    changed
}

/// Folds the poll's events into the port, posts their log lines and redraws the board.
async fn show(http: &Http, bot: &Bot, port: &Port, found: &Poll) {
    if found.restarted {
        let mut live = port.live.lock().await;
        let job = live.state.new_job(JobKind::Port, port.project.clone(), Target::App, Utc::now());
        live.state.start_job(job, Utc::now());
        live.state.board.get_or_insert_with(Board::new).restart();
        live.save(port);
    }
    let mut lines: Vec<String> = found.problems.iter().map(|p| format!("⚠️ Couldn't read {p}")).collect();
    for event in &found.events {
        lines.extend(log_line(event));
        let job = port.live.lock().await.state.last_job().map_or(0, |j| j.id);
        runner::fold(http, bot, port, job, event).await;
        if let Event::Finished { ok, paused, .. } = event {
            let status = match (ok, paused) {
                (true, _) => JobStatus::Finished,
                (false, true) => JobStatus::Paused,
                (false, false) => JobStatus::Failed,
            };
            let mut live = port.live.lock().await;
            live.state.end_job(job, status, Utc::now());
            live.save(port);
        }
    }
    for message in pack_lines(&lines, MESSAGE_LIMIT) {
        scheduler::say(http, port.thread, &message).await;
    }
    if !found.events.is_empty() {
        port.draw(http).await;
    }
}

/// Posts the first look in the channel. `false` when it couldn't be (tried again next poll).
async fn post_first_look(http: &Http, port: &Port, folder: &Path, seen: &Seen) -> bool {
    let (folder, features) = (folder.to_path_buf(), seen.features.clone());
    let text = blocking(move || {
        let bytes = std::fs::read(folder.join(craft_anything_engine::workspace::MANIFEST)).map_err(|e| e.to_string())?;
        let m = craft_anything_engine::workspace::read_manifest(&bytes).map_err(|e| e.message)?;
        Ok::<_, String>(first_look::text(&folder, &m, &features))
    })
    .await
    .and_then(|r| r);
    match text {
        Ok(text) => match port.channel.say(http, clip(&text, MESSAGE_LIMIT)).await {
            Ok(_) => true,
            Err(e) => {
                log::warn!("couldn't post the first look in {}: {e}", port.channel);
                false
            }
        },
        Err(e) => {
            log::warn!("couldn't read the project for the first look in {}: {e}", port.channel);
            false
        }
    }
}
