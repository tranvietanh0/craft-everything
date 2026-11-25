//! The Discord side: sign in, register `/port` in the guild, and hand each `/port` to [`port`].
//! Only the `GUILDS` intent is asked for: slash commands and buttons arrive as interactions.

mod command;
mod embeds;
mod port;
mod runner;

use craft_anything_discord::config::Config;
use craft_anything_discord::queue::RebuildQueue;
use serenity::all::{ApplicationId, Client, Context, EventHandler, GatewayIntents, GuildId, Interaction, Ready};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Shared by every handler: the settings and the one-at-a-time rebuild queue.
pub struct Bot {
    pub config: Config,
    pub queue: Mutex<RebuildQueue<runner::Job>>,
}

struct Handler {
    bot: Arc<Bot>,
}

#[serenity::async_trait]
impl EventHandler for Handler {
    async fn ready(&self, ctx: Context, ready: Ready) {
        log::info!("logged in as {}", ready.user.name);
        let guild = GuildId::new(self.bot.config.guild_id);
        match guild.set_commands(&ctx.http, vec![command::port_command()]).await {
            Ok(commands) => {
                let names: Vec<String> = commands.iter().map(|c| format!("/{}", c.name)).collect();
                log::info!("registered {} in guild {guild}", names.join(", "));
            }
            Err(e) => log::error!("couldn't register /{} in guild {guild}: {e}", command::PORT),
        }
    }

    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        if let Interaction::Command(cmd) = interaction
            && cmd.data.name == command::PORT
        {
            port::handle(&ctx, &cmd, &self.bot).await;
        }
    }
}

/// Runs the bot until Ctrl+C or SIGTERM. The token is only handed to the client, never logged.
pub async fn serve(config: Config) -> Result<(), String> {
    let token = config.token.expose().to_owned();
    let app_id = ApplicationId::new(config.app_id);
    let bot = Arc::new(Bot { config, queue: Mutex::new(RebuildQueue::new()) });
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
