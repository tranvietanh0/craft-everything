//! Posting pictures ([`craft_anything_discord::screens`]): the rebuilt app's screenshots after a
//! check (a `/port` run's and a mirrored app project's alike, both through [`super::runner::fold`]),
//! the original's pictures with the first look, and the pictures a guide links on its tester card.

use super::Bot;
use super::live::Port;
use super::scheduler;
use craft_anything_discord::coalesce::MESSAGE_LIMIT;
use craft_anything_discord::screens::{self, MOST_PER_MESSAGE, Posted, Shot, UPLOAD_LIMIT};
use craft_anything_discord::text::clip;
use craft_anything_engine::workspace::{CheckResult, MANIFEST, read_manifest};
use serenity::all::{CreateAllowedMentions, CreateAttachment, CreateMessage, Http};
use std::path::{Path, PathBuf};

/// A picture ready to send.
pub struct Upload {
    pub key: String,
    pub name: String,
    pub bytes: Vec<u8>,
    pub caption: String,
}

impl Upload {
    pub fn attachment(&self) -> CreateAttachment {
        CreateAttachment::bytes(self.bytes.clone(), self.name.clone()).description(clip(&self.caption, 1_024))
    }
}

/// One picture to read: its key, upload name, caption and file.
pub struct Item {
    pub key: String,
    pub name: String,
    pub caption: String,
    pub path: PathBuf,
}

/// Reads `items` for upload, each within [`UPLOAD_LIMIT`]: the uploads, and `(key, note)` for each
/// one that can't be sent. A scaled-down picture is sent as a PNG and its caption says so. Blocking.
pub fn prepare(items: Vec<Item>) -> (Vec<Upload>, Vec<(String, String)>) {
    let (mut ready, mut skipped) = (Vec::new(), Vec::new());
    for item in items {
        match screens::upload_bytes(&item.path, UPLOAD_LIMIT) {
            Ok((bytes, false)) => ready.push(Upload { key: item.key, name: item.name, bytes, caption: item.caption }),
            Ok((bytes, true)) => ready.push(Upload {
                key: item.key,
                name: Path::new(&item.name).with_extension("png").to_string_lossy().into_owned(),
                bytes,
                caption: format!("{} (scaled down to fit Discord)", item.caption),
            }),
            Err(why) => skipped.push((item.key, format!("{}: not posted, {why}", item.caption))),
        }
    }
    (ready, skipped)
}

/// The uploads, grouped into messages ([`screens::batches`]).
pub fn grouped(uploads: Vec<Upload>, most: usize, limit: u64) -> Vec<Vec<Upload>> {
    let sizes: Vec<u64> = uploads.iter().map(|u| u.bytes.len() as u64).collect();
    let mut slots: Vec<Option<Upload>> = uploads.into_iter().map(Some).collect();
    screens::batches(&sizes, most, limit).into_iter().map(|g| g.into_iter().filter_map(|i| slots[i].take()).collect()).collect()
}

/// The pictures `paths`, read for one message: the first group that fits. Blocking.
pub fn gallery(paths: Vec<PathBuf>, most: usize, limit: u64) -> Vec<Upload> {
    let items = paths
        .into_iter()
        .take(most)
        .map(|path| {
            let name = path.file_name().map_or_else(|| "picture.png".to_owned(), |n| n.to_string_lossy().into_owned());
            let caption = path.file_stem().map_or_else(String::new, |s| s.to_string_lossy().replace(['-', '_'], " "));
            Item { key: String::new(), name, caption, path }
        })
        .collect();
    let (ready, _) = prepare(items);
    grouped(ready, most, limit).into_iter().next().unwrap_or_default()
}

async fn blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    tokio::task::spawn_blocking(work).await.map_err(|e| format!("the work stopped unexpectedly: {e}"))
}

fn checks_of(project: &Path) -> Vec<CheckResult> {
    std::fs::read(project.join(MANIFEST)).ok().and_then(|b| read_manifest(&b).ok()).map(|m| m.checks).unwrap_or_default()
}

/// Posts the port's screenshots not posted yet, in messages of up to ten, each picture captioned
/// with its round, what it shows and its check's verdict. Each message is recorded as posted
/// once it went out; one that fails stops here and is tried at the next check.
pub async fn post_new(http: &Http, bot: &Bot, port: &Port) {
    // One at a time across ports: they share the record of what was posted.
    let _one = bot.shots.lock().await;
    let (root, project) = (bot.config.ports_root.clone(), port.project.clone());
    let found = blocking(move || Posted::load(&root).map(|posted| (screens::unposted(screens::find(&project), &posted), checks_of(&project), posted))).await;
    let (new, checks, mut posted) = match found.and_then(|r| r) {
        Ok(found) => found,
        Err(e) => {
            log::error!("{e}; screenshots are not posted until it is fixed");
            return;
        }
    };
    let by_key: std::collections::HashMap<String, Shot> = new.iter().map(|s| (s.key(), s.clone())).collect();
    for chunk in new.chunks(MOST_PER_MESSAGE) {
        let items = chunk
            .iter()
            .map(|s| Item { key: s.key(), name: s.upload_name(), caption: screens::caption(s, screens::made_by(&s.rel, &checks)), path: s.path.clone() })
            .collect();
        let Ok((ready, skipped)) = blocking(move || prepare(items)).await else { return };
        if !skipped.is_empty() {
            let notes: Vec<String> = skipped.iter().map(|(_, note)| format!("⚠️ {note}")).collect();
            scheduler::say(http, port.channel, &clip(&notes.join("\n"), MESSAGE_LIMIT)).await;
        }
        let mut sent: Vec<String> = skipped.into_iter().map(|(key, _)| key).collect();
        for group in grouped(ready, MOST_PER_MESSAGE, UPLOAD_LIMIT) {
            let lines: Vec<String> = group.iter().map(|u| format!("• `{}`: {}", u.name, u.caption)).collect();
            let text = clip(&format!("📸 **Screenshots of the rebuilt app**\n{}", lines.join("\n")), MESSAGE_LIMIT);
            let message = CreateMessage::new().content(text).allowed_mentions(CreateAllowedMentions::new()).add_files(group.iter().map(Upload::attachment));
            if let Err(e) = port.channel.send_message(http, message).await {
                log::warn!("couldn't post screenshots in {}: {e}; tried again at the next check", port.channel);
                break;
            }
            sent.extend(group.into_iter().map(|u| u.key));
        }
        let done = sent.len() == chunk.len();
        for key in sent {
            if let Some(shot) = by_key.get(&key) {
                posted.mark(shot);
            }
        }
        let (root, saved) = (bot.config.ports_root.clone(), posted.clone());
        if let Err(e) = blocking(move || saved.save(&root)).await.and_then(|r| r) {
            log::error!("{e}; a restart may post some screenshots again");
        }
        if !done {
            return;
        }
    }
}
