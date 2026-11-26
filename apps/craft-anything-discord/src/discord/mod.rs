//! The Discord side: sign in, register the commands in the guild, hand each `/port` to [`port`],
//! each control command to [`commands`] and each tester-card press to [`card`], and mirrors projects the craft desktop app registers ([`mirror`]). On the first
//! sign-in the saved queue is restored ([`scheduler::recover`]) and the daily summary starts
//! ([`summary`]). Only the `GUILDS` intent is asked for: slash commands, buttons and forms arrive
//! as interactions.

mod card;
mod command;
mod commands;
mod embeds;
mod live;
mod mirror;
mod port;
mod runner;
mod scheduler;
mod screens;
mod summary;
mod sync;

use craft_anything_discord::config::Config;
use craft_anything_discord::plane::{HttpPlane, PlaneApi};
use craft_anything_discord::queue::RebuildQueue;
use craft_anything_discord::repo::{Shell, SystemShell};
use craft_anything_discord::review::{CliRecorder, FeatureRecorder};
use craft_anything_discord::saved_queue::Recovery;
use craft_anything_discord::tester::Press;
use serenity::all::{ApplicationId, ChannelId, Client, Context, EventHandler, GatewayIntents, GuildId, Interaction, Ready};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Mutex;

/// Shared by every handler: the settings, the one-at-a-time rebuild queue, the ports the bot
/// knows in memory (by channel) and the outside services.
pub struct Bot {
    pub config: Config,
    pub queue: Mutex<RebuildQueue<scheduler::Job>>,
    pub ports: Mutex<HashMap<ChannelId, Arc<live::Port>>>,
    pub services: Arc<Services>,
    /// Held while screenshots are posted: every port shares the record of what was posted
    /// (`PORTS_ROOT/.bot/screenshots.json`).
    pub shots: Mutex<()>,
    /// Set on the first sign-in: a reconnect signs in again, but the queue is restored and the
    /// summary started once.
    started: AtomicBool,
    /// The saved queue, read before signing in and restored on the first sign-in.
    recovery: Mutex<Option<Recovery>>,
}

/// What the bot calls outside Discord, behind traits (the library tests them with fakes).
pub struct Services {
    /// Records a tester's verdict through the engine.
    pub recorder: Box<dyn FeatureRecorder>,
    /// `None` when the `PLANE_*` keys are not set.
    pub plane: Option<Box<dyn PlaneApi>>,
    /// `gh` and `git`, used when `GITHUB_OWNER` is set.
    pub shell: Box<dyn Shell>,
    pub github_owner: Option<String>,
}

impl Services {
    fn from_config(config: &Config) -> Services {
        Services {
            recorder: Box::new(CliRecorder { craft_cli: config.craft_cli.clone() }),
            plane: config.plane.as_ref().map(|p| Box::new(HttpPlane::new(p)) as Box<dyn PlaneApi>),
            shell: Box::new(SystemShell),
            github_owner: config.github_owner.clone(),
        }
    }
}

struct Handler {
    bot: Arc<Bot>,
}

#[serenity::async_trait]
impl EventHandler for Handler {
    async fn ready(&self, ctx: Context, ready: Ready) {
        log::info!("logged in as {}", ready.user.name);
        let guild = GuildId::new(self.bot.config.guild_id);
        match guild.set_commands(&ctx.http, command::commands()).await {
            Ok(commands) => {
                let names: Vec<String> = commands.iter().map(|c| format!("/{}", c.name)).collect();
                log::info!("registered {} in guild {guild}", names.join(", "));
            }
            Err(e) => log::error!("couldn't register the commands in guild {guild}: {e}"),
        }
        if !self.bot.started.swap(true, Ordering::SeqCst) {
            let (http, bot) = (Arc::clone(&ctx.http), Arc::clone(&self.bot));
            if let Some(recovery) = self.bot.recovery.lock().await.take() {
                tokio::spawn(async move { scheduler::recover(&http, &bot, recovery).await });
            }
            tokio::spawn(summary::run(Arc::clone(&ctx.http), Arc::clone(&self.bot)));
            if self.bot.config.mirror_apps {
                tokio::spawn(mirror::run(Arc::clone(&ctx.http), Arc::clone(&self.bot)));
            }
        }
    }

    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        match interaction {
            Interaction::Command(cmd) if cmd.data.name == command::PORT => port::handle(&ctx, &cmd, &self.bot).await,
            Interaction::Command(cmd) if command::CONTROL.iter().any(|(name, _)| cmd.data.name == *name) => commands::handle(&ctx, &cmd, &self.bot).await,
            // Other buttons (the `/port` confirmation) are read by their own collector.
            Interaction::Component(press) => {
                if let Some(which) = Press::parse(&press.data.custom_id) {
                    card::on_press(&ctx, &press, which, &self.bot).await;
                }
            }
            Interaction::Modal(form) => {
                if let Some(Press::FailForm(card)) = Press::parse(&form.data.custom_id) {
                    card::on_form(&ctx, &form, card, &self.bot).await;
                }
            }
            _ => {}
        }
    }
}

/// Runs the bot until Ctrl+C or SIGTERM. The token is only handed to the client, never logged.
pub async fn serve(config: Config) -> Result<(), String> {
    let token = config.token.expose().to_owned();
    let app_id = ApplicationId::new(config.app_id);
    let services = Arc::new(Services::from_config(&config));
    let recovery = scheduler::load_saved(&config.ports_root);
    let bot = Arc::new(Bot {
        config,
        queue: Mutex::new(RebuildQueue::new()),
        ports: Mutex::new(HashMap::new()),
        services,
        shots: Mutex::new(()),
        started: AtomicBool::new(false),
        recovery: Mutex::new(Some(recovery)),
    });
    let mut client = Client::builder(&token, GatewayIntents::GUILDS)
        .application_id(app_id)
        .event_handler(Handler { bot })
        .await
        .map_err(|e| format!("couldn't create the Discord client: {e}"))?;
    let shards = client.shard_manager.clone();
    tokio::spawn(async move {
        wait_for_stop().await;
        log::info!("stopping");
        shards.shutdown_all().await;
    });
    client.start().await.map_err(|e| format!("the Discord connection ended: {e}"))
}

#[cfg(unix)]
async fn wait_for_stop() {
    use tokio::signal::unix::{SignalKind, signal};
    match signal(SignalKind::terminate()) {
        Ok(mut term) => {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = term.recv() => {}
            }
        }
        Err(e) => {
            log::warn!("couldn't listen for SIGTERM ({e}); Ctrl+C still stops the bot");
            let _ = tokio::signal::ctrl_c().await;
        }
    }
}

#[cfg(not(unix))]
async fn wait_for_stop() {
    let _ = tokio::signal::ctrl_c().await;
}
