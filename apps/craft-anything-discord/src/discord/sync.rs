//! A port's Plane and GitHub work, one job at a time in the order queued, off the run's loop.
//! Every call is blocking and runs on a blocking thread. A failure never stops the port: it is
//! logged, and the first failure of an outage gets one line in the Log thread.

use super::Services;
use super::live::Port;
use craft_anything_discord::plane::{self, PlaneIds};
use craft_anything_discord::repo::{self, RepoInfo};
use craft_anything_discord::text::clip;
use serenity::all::{CreateAllowedMentions, CreateEmbed, CreateMessage, EditMessage, Http, MessageId};
use std::sync::Arc;
use tokio::sync::mpsc;

/// Discord's message limit.
const MESSAGE_LIMIT: usize = 2_000;

pub enum SyncJob {
    /// The port started: make its Plane project and GitHub repo, and link the repo on the profile.
    Setup,
    /// A feature changed state (from the builder or a tester).
    Feature { name: String, state: String, note: Option<String> },
    /// Commit and push the project folder.
    Commit { message: String },
}

/// Whether a service worked last time, so an outage is told once.
struct Health {
    service: &'static str,
    ok: bool,
}

impl Health {
    async fn report<T>(&mut self, http: &Http, port: &Port, result: Result<T, String>) -> Option<T> {
        match result {
            Ok(value) => {
                self.ok = true;
                Some(value)
            }
            Err(e) => {
                log::warn!("{} for {}: {e}", self.service, port.channel);
                if self.ok {
                    let text =
                        format!("⚠️ {}: {e}. The port goes on; until it works again, more {} errors go only to the bot's log.", self.service, self.service);
                    say(http, port, &text).await;
                }
                self.ok = false;
                None
            }
        }
    }
}

async fn say(http: &Http, port: &Port, text: &str) {
    let message = CreateMessage::new().content(clip(text, MESSAGE_LIMIT)).allowed_mentions(CreateAllowedMentions::new());
    if let Err(e) = port.thread.send_message(http, message).await {
        log::warn!("couldn't post to the Log thread of {}: {e}", port.channel);
    }
}

async fn blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    tokio::task::spawn_blocking(work).await.map_err(|e| format!("the work stopped unexpectedly: {e}"))
}

pub async fn run(http: Arc<Http>, services: Arc<Services>, port: Arc<Port>, mut jobs: mpsc::UnboundedReceiver<SyncJob>) {
    let mut plane_health = Health { service: "Plane", ok: true };
    let mut github_health = Health { service: "GitHub", ok: true };
    while let Some(job) = jobs.recv().await {
        match job {
            SyncJob::Setup => {
                if services.plane.is_some() {
                    let result = sync_plane(&services, &port, None).await;
                    plane_health.report(&http, &port, result).await;
                }
                if services.github_owner.is_some()
                    && let Some(repo) = github_health.report(&http, &port, ensure_repo(&services, &port).await).await
                {
                    link_repo(&http, &port, &repo).await;
                }
            }
            SyncJob::Feature { name, state, note } => {
                if services.plane.is_some() {
                    let result = sync_plane(&services, &port, Some((name, state, note))).await;
                    plane_health.report(&http, &port, result).await;
                }
            }
            SyncJob::Commit { message } => {
                if services.github_owner.is_some() {
                    let result = commit(&http, &services, &port, message).await;
                    github_health.report(&http, &port, result).await;
                }
            }
        }
    }
}

/// Makes the Plane project (no feature) or brings one feature up to date. What was made is saved
/// even when a later call failed.
async fn sync_plane(services: &Arc<Services>, port: &Port, feature: Option<(String, String, Option<String>)>) -> Result<(), String> {
    let (mut ids, slug, label): (Option<PlaneIds>, String, String) = {
        let live = port.live.lock().await;
        (live.state.plane.clone(), live.state.slug.clone(), live.state.label.clone())
    };
    let services = Arc::clone(services);
    let (ids, result) = blocking(move || {
        let Some(api) = services.plane.as_deref() else {
            return (ids, Ok(()));
        };
        let result = match &feature {
            None => plane::ensure_project(api, &mut ids, &slug, &label),
            Some((name, state, note)) => plane::sync_feature(api, &mut ids, (&slug, &label), name, state, note.as_deref()),
        };
        (ids, result.map_err(|e| e.to_string()))
    })
    .await?;
    let mut live = port.live.lock().await;
    if live.state.plane != ids {
        live.state.plane = ids;
        live.save(port);
    }
    result
}

/// The port's repo: the saved one, else made now (and saved).
async fn ensure_repo(services: &Arc<Services>, port: &Port) -> Result<RepoInfo, String> {
    let (saved, slug) = {
        let live = port.live.lock().await;
        (live.state.repo.clone(), live.state.slug.clone())
    };
    if let Some(repo) = saved {
        return Ok(repo);
    }
    let services = Arc::clone(services);
    let repo = blocking(move || match services.github_owner.as_deref() {
        Some(owner) => repo::create_repo(services.shell.as_ref(), owner, &slug),
        None => Err("GITHUB_OWNER is not set".to_owned()),
    })
    .await??;
    let mut live = port.live.lock().await;
    live.state.repo = Some(repo.clone());
    live.save(port);
    Ok(repo)
}

async fn commit(http: &Http, services: &Arc<Services>, port: &Port, message: String) -> Result<(), String> {
    let repo = ensure_repo(services, port).await?;
    let (shell_services, project) = (Arc::clone(services), port.project.clone());
    let pushed = blocking(move || repo::commit_and_push(shell_services.shell.as_ref(), &project, &repo, &message)).await??;
    if !pushed.left_out.is_empty() {
        let text = format!("📦 Left out of the GitHub repo, too large for it: {}", pushed.left_out.join(", "));
        say(http, port, &text).await;
    }
    Ok(())
}

/// Adds the repo link to the pinned app profile.
async fn link_repo(http: &Http, port: &Port, repo: &RepoInfo) {
    let profile = MessageId::new(port.live.lock().await.state.profile_message.max(1));
    let message = match port.channel.message(http, profile).await {
        Ok(m) => m,
        Err(e) => {
            log::warn!("couldn't read the app profile in {} to link the repo: {e}", port.channel);
            return;
        }
    };
    let Some(embed) = message.embeds.first().cloned() else {
        return;
    };
    if embed.fields.iter().any(|f| f.name == "GitHub") {
        return;
    }
    let edit = EditMessage::new().embed(CreateEmbed::from(embed).field("GitHub", format!("[{}]({})", repo.full_name, repo.url), false));
    if let Err(e) = port.channel.edit_message(http, profile, edit).await {
        log::warn!("couldn't add the repo link to the app profile in {}: {e}", port.channel);
    }
}
