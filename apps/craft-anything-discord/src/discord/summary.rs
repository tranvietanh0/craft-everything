//! The daily summary to `DAILY_CHANNEL_ID`: checked every minute, posted once a day at
//! `DAILY_SUMMARY_TIME`. The date is saved before the post, so a crash mid-post can't post a day
//! twice; a post that fails before anything went out gives the date back, to try again.

use super::Bot;
use chrono::Utc;
use craft_anything_discord::coalesce::{MESSAGE_LIMIT, pack_lines};
use craft_anything_discord::daily::{self, DailyState};
use craft_anything_discord::state::PortState;
use serenity::all::{ChannelId, CreateAllowedMentions, CreateMessage, Http};
use std::sync::Arc;
use std::time::Duration;

const CHECK_EVERY: Duration = Duration::from_secs(60);

pub async fn run(http: Arc<Http>, bot: Arc<Bot>) {
    let mut tick = tokio::time::interval(CHECK_EVERY);
    // The last error, so a lasting problem is logged once rather than every minute.
    let mut last_error: Option<String> = None;
    loop {
        tick.tick().await;
        let result = post_if_due(&http, &bot).await;
        match result {
            Err(e) if last_error.as_deref() != Some(e.as_str()) => {
                log::error!("daily summary: {e}");
                last_error = Some(e);
            }
            Err(_) => {}
            Ok(()) => last_error = None,
        }
    }
}

async fn blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    tokio::task::spawn_blocking(work).await.map_err(|e| format!("the work stopped unexpectedly: {e}"))
}

async fn post_if_due(http: &Http, bot: &Bot) -> Result<(), String> {
    let root = bot.config.ports_root.clone();
    let read = root.clone();
    let before = blocking(move || DailyState::load(&read)).await??;
    let now = Utc::now();
    let Some(date) = bot.config.daily.due(now, before.last_posted) else {
        return Ok(());
    };
    let read = root.clone();
    let ports = blocking(move || PortState::load_all(&read)).await?;
    let text = daily::render(date, &daily::summarize(&ports, now));
    let claim = root.clone();
    blocking(move || DailyState { last_posted: Some(date) }.save(&claim)).await??;
    let lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let channel = ChannelId::new(bot.config.daily_channel_id);
    for (index, part) in pack_lines(&lines, MESSAGE_LIMIT).into_iter().enumerate() {
        let message = CreateMessage::new().content(part).allowed_mentions(CreateAllowedMentions::new());
        if let Err(e) = channel.send_message(http, message).await {
            if index == 0 {
                blocking(move || before.save(&root)).await??;
                return Err(format!("couldn't post the summary for {date} in {channel} ({e}); it is tried again in a minute"));
            }
            return Err(format!("the summary for {date} was cut short in {channel}: {e}"));
        }
    }
    log::info!("posted the daily summary for {date}");
    Ok(())
}
