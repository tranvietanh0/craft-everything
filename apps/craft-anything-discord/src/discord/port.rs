//! One `/port`: porter-role check, the rights confirmation, the download and identification, the
//! `port-<slug>` channel with its pinned profile and board and its "Log" thread, then the queue.

use super::Bot;
use super::command::{self, PortOptions};
use super::embeds;
use super::live::Port;
use super::scheduler::{self, Job};
use super::sync::SyncJob;
use craft_anything_discord::board::Board;
use craft_anything_discord::cost::Usage;
use craft_anything_discord::identify::{self, AppIdentity};
use craft_anything_discord::intake::{self, Source};
use craft_anything_discord::jobs::JobKind;
use craft_anything_discord::slug;
use craft_anything_discord::state::PortState;
use serenity::all::{
    ButtonStyle, ChannelId, ChannelType, CommandInteraction, Context, CreateActionRow, CreateButton, CreateChannel, CreateInteractionResponse,
    CreateInteractionResponseMessage, CreateMessage, CreateThread, EditInteractionResponse, GuildId, RoleId,
};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

const CONFIRM_ID: &str = "port-confirm";
const CANCEL_ID: &str = "port-cancel";
const CONFIRM_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// Tries at reserving a free `PORTS_ROOT/<slug>` folder before giving up.
const SLUG_TRIES: usize = 20;
const CHANNEL_PREFIX: &str = "port-";

/// Handles one `/port`. Every failure is told to the person, ephemerally; nothing is posted in
/// public until the rights are confirmed and the file is in.
pub async fn handle(ctx: &Context, cmd: &CommandInteraction, bot: &Arc<Bot>) {
    let porter = RoleId::new(bot.config.porter_role_id);
    if !cmd.member.as_ref().is_some_and(|m| m.roles.contains(&porter)) {
        reply(ctx, cmd, "Only members with the porter role can use /port.").await;
        return;
    }
    let options = command::read_options(cmd);
    let source = match Source::choose(options.attachment.clone(), options.link.as_deref(), bot.config.max_upload_bytes) {
        Ok(s) => s,
        Err(e) => {
            reply(ctx, cmd, &format!("❌ {e}")).await;
            return;
        }
    };
    match confirm(ctx, cmd, &options, &source).await {
        Ok(true) => {}
        Ok(false) => return,
        Err(e) => {
            log::warn!("/port confirmation failed: {e}");
            return;
        }
    }
    if let Err(e) = start(ctx, cmd, bot, &options, &source).await {
        log::warn!("/port by {} failed: {e}", cmd.user.id);
        progress(ctx, cmd, &format!("❌ {e}")).await;
    }
}

async fn reply(ctx: &Context, cmd: &CommandInteraction, text: &str) {
    let message = CreateInteractionResponseMessage::new().content(text).ephemeral(true);
    if let Err(e) = cmd.create_response(&ctx.http, CreateInteractionResponse::Message(message)).await {
        log::warn!("couldn't answer /port: {e}");
    }
}

/// Edits the ephemeral answer (after the confirmation it is the person's progress line).
async fn progress(ctx: &Context, cmd: &CommandInteraction, text: &str) {
    if let Err(e) = cmd.edit_response(&ctx.http, EditInteractionResponse::new().content(text).components(Vec::new())).await {
        log::warn!("couldn't update the /port answer: {e}");
    }
}

/// Asks for the rights confirmation. `Ok(true)` once the person presses the confirm button.
async fn confirm(ctx: &Context, cmd: &CommandInteraction, options: &PortOptions, source: &Source) -> Result<bool, String> {
    let what = options.name.clone().or_else(|| source.suggested_name()).unwrap_or_else(|| "this app".to_owned());
    let buttons = CreateActionRow::Buttons(vec![
        CreateButton::new(CONFIRM_ID).style(ButtonStyle::Success).label("I have the rights to port this app"),
        CreateButton::new(CANCEL_ID).style(ButtonStyle::Secondary).label("Cancel"),
    ]);
    let ask = CreateInteractionResponseMessage::new()
        .content(format!("Port **{what}** to {}? Only port apps you own or have written permission to port.", options.target.label()))
        .components(vec![buttons])
        .ephemeral(true);
    cmd.create_response(&ctx.http, CreateInteractionResponse::Message(ask)).await.map_err(|e| e.to_string())?;
    let message = cmd.get_response(&ctx.http).await.map_err(|e| e.to_string())?;
    let Some(press) = message.await_component_interaction(&ctx.shard).author_id(cmd.user.id).timeout(CONFIRM_TIMEOUT).await else {
        progress(ctx, cmd, "Timed out: nothing was started.").await;
        return Ok(false);
    };
    let confirmed = press.data.custom_id == CONFIRM_ID;
    let text = if confirmed { "Starting: fetching the file…" } else { "Cancelled: nothing was started." };
    let update = CreateInteractionResponseMessage::new().content(text).components(Vec::new());
    press.create_response(&ctx.http, CreateInteractionResponse::UpdateMessage(update)).await.map_err(|e| e.to_string())?;
    Ok(confirmed)
}

async fn start(ctx: &Context, cmd: &CommandInteraction, bot: &Arc<Bot>, options: &PortOptions, source: &Source) -> Result<(), String> {
    let config = &bot.config;
    let guild = GuildId::new(config.guild_id);
    let base_name =
        options.name.clone().or_else(|| source.suggested_name().map(|n| identify::name_from_file(Path::new(&n)))).unwrap_or_else(|| "app".to_owned());
    let channels = guild.channels(&ctx.http).await.map_err(|e| format!("couldn't list the server's channels: {e}"))?;
    let taken: Vec<String> = channels.values().map(|c| c.name.clone()).collect();
    let (name, slug) = reserve_slug(&config.ports_root, &slug::slugify(&base_name), taken)?;

    let input_dir = intake::input_dir(&config.ports_root, &slug);
    let (dl_source, cap) = (source.clone(), config.max_upload_bytes);
    let file = blocking(move || intake::download(&dl_source, &input_dir, cap)).await?.map_err(|e| e.to_string())?;
    let bytes = std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
    progress(ctx, cmd, "Reading the app's name and package…").await;
    let (cli, read) = (config.craft_cli.clone(), file.clone());
    let app: AppIdentity = blocking(move || identify::identify(&cli, &read)).await?;
    let label = options.name.clone().unwrap_or_else(|| app.label.clone());

    let topic = format!("{} · target {}", app.package.as_deref().unwrap_or("unknown package"), options.target.choice());
    let create = CreateChannel::new(&name).kind(ChannelType::Text).category(ChannelId::new(config.ports_category_id)).topic(topic);
    let channel = guild.create_channel(&ctx.http, create).await.map_err(|e| format!("couldn't create #{name}: {e}"))?.id;
    let file_name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let profile = CreateMessage::new().embed(embeds::profile_embed(&app, options.target, cmd.user.id, &file_name, bytes));
    let profile = channel.send_message(&ctx.http, profile).await.map_err(|e| format!("couldn't post the app profile: {e}"))?;
    pin(ctx, channel, profile.id).await;
    let board = CreateMessage::new().embed(embeds::board_embed(&label, &Board::new().render(Duration::ZERO, Usage::default().describe())));
    let board = channel.send_message(&ctx.http, board).await.map_err(|e| format!("couldn't post the progress board: {e}"))?;
    pin(ctx, channel, board.id).await;
    let thread = channel
        .create_thread(&ctx.http, CreateThread::new("Log").kind(ChannelType::PublicThread))
        .await
        .map_err(|e| format!("couldn't create the Log thread: {e}"))?
        .id;

    let state = PortState {
        slug: slug.clone(),
        label,
        channel: channel.get(),
        thread: thread.get(),
        board_message: board.id.get(),
        profile_message: profile.id.get(),
        board: Some(Board::new()),
        ..PortState::default()
    };
    if let Err(e) = state.save(&config.ports_root.join(&slug)) {
        log::warn!("{e}");
    }
    let port = Port::start(ctx.http.clone(), bot, state);
    bot.ports.lock().await.insert(channel, Arc::clone(&port));
    // The Plane project and the GitHub repo are made now, not when the port's turn comes.
    port.queue(SyncJob::Setup);
    let job = {
        let mut live = port.live.lock().await;
        Job::record(&port, &mut live, JobKind::Port, file, options.target)
    };
    match scheduler::enqueue(&ctx.http, bot, job).await {
        None => {
            progress(ctx, cmd, &format!("✅ Started: follow it in <#{channel}>.")).await;
        }
        Some(position) => {
            let waiting = format!("⏳ Waiting in line: position {position}. It starts when the ports before it finish.");
            if let Err(e) = channel.say(&ctx.http, &waiting).await {
                log::warn!("couldn't post the queue position in {channel}: {e}");
            }
            progress(ctx, cmd, &format!("⏳ Queued at position {position}: follow it in <#{channel}>.")).await;
        }
    }
    Ok(())
}

/// Picks `port-<slug>` free among `taken` channel names and reserves `PORTS_ROOT/<slug>` by
/// creating it, so two ports never share a folder. Returns the channel name and the slug.
fn reserve_slug(ports_root: &Path, base: &str, mut taken: Vec<String>) -> Result<(String, String), String> {
    std::fs::create_dir_all(ports_root).map_err(|e| format!("couldn't create {}: {e}", ports_root.display()))?;
    for _ in 0..SLUG_TRIES {
        let name = slug::channel_name(base, &taken);
        let slug = name.strip_prefix(CHANNEL_PREFIX).unwrap_or(&name).to_owned();
        match std::fs::create_dir(ports_root.join(&slug)) {
            Ok(()) => return Ok((name, slug)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => taken.push(name),
            Err(e) => return Err(format!("couldn't create the port folder: {e}")),
        }
    }
    Err("couldn't find a free name for the port's folder".to_owned())
}

async fn pin(ctx: &Context, channel: ChannelId, message: serenity::all::MessageId) {
    if let Err(e) = channel.pin(&ctx.http, message).await {
        log::warn!("couldn't pin a message in {channel}: {e}");
    }
}

/// Runs blocking work (a download, a CLI call) off the async threads.
async fn blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    tokio::task::spawn_blocking(work).await.map_err(|e| format!("the work stopped unexpectedly: {e}"))
}
