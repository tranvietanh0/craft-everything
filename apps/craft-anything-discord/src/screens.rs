//! Screenshots of the rebuilt app, posted to the port's channel. Craft's checks write them under
//! the project's `checks/round-<n>/` (the app check's `screenshot.png` and `window.png`, a
//! website's `screenshot-<width>.png` and `routes/<page>/…`; round 0 is the builder's own check,
//! rewritten each time it runs) and list them in `craft.json` as the check's `artifacts`, which
//! gives a picture its round, its check and pass or fail.
//!
//! The same code serves a `/port` run and a mirrored app project: a `checked` or `finished` event
//! of either looks for pictures not posted yet. What was posted is kept by path and modification
//! time in `PORTS_ROOT/.bot/screenshots.json` (written atomically), so a restart posts nothing
//! twice and a picture written again is a new one. Uploads go up to [`MOST_PER_MESSAGE`] to a
//! message and [`UPLOAD_LIMIT`] bytes to a file and to a message; a larger picture is scaled down
//! until it fits, or skipped with a note.

use crate::saved_queue::BOT_DIR;
use craft_anything_engine::tools::web_common::{decode_picture, encode_png_within, resize};
use craft_anything_engine::workspace::CheckResult;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

pub const SHOTS_FILE: &str = "screenshots.json";
/// Discord takes at most 10 attachments on one message.
pub const MOST_PER_MESSAGE: usize = 10;
/// The most bytes sent in one file and in one message: under Discord's 10 MiB upload limit for a
/// server without boosts, with room for the rest of the request.
pub const UPLOAD_LIMIT: u64 = 8 * 1024 * 1024;
/// The picture types posted.
pub const PICTURES: [&str; 5] = ["png", "jpg", "jpeg", "webp", "gif"];
/// Pictures are looked for this deep under each round's folder (`routes/<page>/` for a website).
const DEPTH: usize = 3;
/// A picture too large is halved at most this many times before it is skipped.
const HALVINGS: u32 = 4;

/// One picture a check wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shot {
    pub path: PathBuf,
    /// Relative to the project folder, with `/`: `checks/round-1/screenshot.png`.
    pub rel: String,
    pub modified_ms: u64,
    pub len: u64,
}

impl Shot {
    /// What [`Posted`] keeps: the path and the modification time.
    pub fn key(&self) -> String {
        format!("{}|{}", self.path.display(), self.modified_ms)
    }

    /// A file name unique within one message: `round-1-routes-about-screenshot-390.png`.
    pub fn upload_name(&self) -> String {
        self.rel.strip_prefix("checks/").unwrap_or(&self.rel).replace('/', "-")
    }
}

pub fn is_picture(path: &Path) -> bool {
    path.extension().is_some_and(|x| PICTURES.contains(&x.to_string_lossy().to_ascii_lowercase().as_str()))
}

/// The picture files under `dir`, `depth` folders deep, sorted.
pub fn pictures_in(dir: &Path, depth: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect(dir, depth, &mut out);
    out.sort();
    out
}

fn collect(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for path in entries.flatten().map(|e| e.path()) {
        if path.is_dir() {
            if depth > 0 {
                collect(&path, depth - 1, out);
            }
        } else if path.is_file() && is_picture(&path) {
            out.push(path);
        }
    }
}

/// The round a folder name stands for: `round-3` → 3.
fn round_of(name: &str) -> Option<u32> {
    name.strip_prefix("round-")?.parse().ok()
}

/// Every picture the checks wrote in `project/checks/round-*/`, by round, then path.
pub fn find(project: &Path) -> Vec<Shot> {
    let Ok(entries) = std::fs::read_dir(project.join("checks")) else { return Vec::new() };
    let mut rounds: Vec<(u32, PathBuf)> =
        entries.flatten().filter_map(|e| Some((round_of(&e.file_name().to_string_lossy())?, e.path()))).filter(|(_, p)| p.is_dir()).collect();
    rounds.sort();
    let mut shots = Vec::new();
    for (_, dir) in rounds {
        for path in pictures_in(&dir, DEPTH) {
            let Ok(meta) = std::fs::metadata(&path) else { continue };
            let Ok(rel) = path.strip_prefix(project) else { continue };
            let modified_ms = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis());
            let rel = rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
            shots.push(Shot { path, rel, modified_ms: u64::try_from(modified_ms).unwrap_or(u64::MAX), len: meta.len() });
        }
    }
    shots
}

/// The pictures already posted, by [`Shot::key`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Posted {
    pub posted: BTreeSet<String>,
}

impl Posted {
    pub fn path(ports_root: &Path) -> PathBuf {
        ports_root.join(BOT_DIR).join(SHOTS_FILE)
    }

    /// No file yet is none posted; a file that can't be read is an error (never silently empty,
    /// which would post every picture again).
    pub fn load(ports_root: &Path) -> Result<Posted, String> {
        let path = Self::path(ports_root);
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| format!("{} is not valid: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Posted::default()),
            Err(e) => Err(format!("couldn't read {}: {e}", path.display())),
        }
    }

    pub fn save(&self, ports_root: &Path) -> Result<(), String> {
        let path = Self::path(ports_root);
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| format!("couldn't encode the posted screenshots: {e}"))?;
        craft_anything_platform::fs::write_atomic(&path, &bytes).map_err(|e| format!("couldn't save {}: {}", path.display(), e.message))
    }

    pub fn contains(&self, shot: &Shot) -> bool {
        self.posted.contains(&shot.key())
    }

    pub fn mark(&mut self, shot: &Shot) {
        self.posted.insert(shot.key());
    }
}

/// The shots in `found` not posted yet, in order.
pub fn unposted(found: Vec<Shot>, posted: &Posted) -> Vec<Shot> {
    found.into_iter().filter(|s| !posted.contains(s)).collect()
}

/// The check that made a picture: the last one in `craft.json` that lists it.
pub fn made_by<'a>(rel: &str, checks: &'a [CheckResult]) -> Option<&'a CheckResult> {
    checks.iter().rev().find(|c| c.artifacts.iter().any(|a| a == Path::new(rel)))
}

/// What a picture shows, from its name: `screenshot` is the app's screen, `window` the app
/// started normally, `screenshot-390` a page 390 px wide, and folders under `routes/` the page.
pub fn screen_name(rel: &str) -> String {
    let parts: Vec<&str> = rel.split('/').collect();
    let inner = parts.iter().position(|p| round_of(p).is_some()).map_or(&parts[..], |i| &parts[i + 1..]);
    let (file, folders) = inner.split_last().map_or(("", &[][..]), |(f, d)| (*f, d));
    let stem = Path::new(file).file_stem().map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    let what = match stem.as_str() {
        "screenshot" => "app screen".to_owned(),
        "window" => "app started normally".to_owned(),
        s => match s.strip_prefix("screenshot-").filter(|w| w.parse::<u32>().is_ok()) {
            Some(width) => format!("{width} px wide"),
            None => s.replace(['-', '_'], " "),
        },
    };
    let page: Vec<&str> = folders.iter().copied().filter(|f| *f != "routes").collect();
    if page.is_empty() { what } else { format!("{} · {what}", page.join(" / ")) }
}

/// A picture's caption: the round, what it shows, and the check's verdict when craft linked one.
pub fn caption(shot: &Shot, check: Option<&CheckResult>) -> String {
    let round = check.map(|c| c.round).or_else(|| shot.rel.split('/').find_map(round_of));
    let round = match round {
        Some(0) => "Builder's own check".to_owned(),
        Some(n) => format!("Round {n}"),
        None => "Check".to_owned(),
    };
    let verdict = match check {
        Some(c) if c.passed => format!("✅ {} passed", c.check),
        Some(c) => format!("❌ {} failed", c.check),
        None => "no verdict yet".to_owned(),
    };
    format!("{round} · {} · {verdict}", screen_name(&shot.rel))
}

/// The groups to send, as indexes into `sizes`, in order: at most `most` to a group and at most
/// `limit` bytes in all (a single item over `limit` goes alone).
pub fn batches(sizes: &[u64], most: usize, limit: u64) -> Vec<Vec<usize>> {
    let mut out: Vec<Vec<usize>> = Vec::new();
    let mut total = 0u64;
    for (i, &size) in sizes.iter().enumerate() {
        match out.last_mut() {
            Some(group) if group.len() < most.max(1) && total.saturating_add(size) <= limit => {
                group.push(i);
                total = total.saturating_add(size);
            }
            _ => {
                out.push(vec![i]);
                total = size;
            }
        }
    }
    out
}

/// A picture's bytes for upload: as it is when it fits in `limit`, else scaled down (as a PNG)
/// until it does. The error says why it can't be sent. Blocking.
pub fn upload_bytes(path: &Path, limit: u64) -> Result<(Vec<u8>, bool), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("couldn't read it: {e}"))?;
    if bytes.len() as u64 <= limit {
        return Ok((bytes, false));
    }
    let size = bytes.len() / (1024 * 1024);
    let mut img = decode_picture(&bytes).map_err(|e| format!("it is {size} MB and couldn't be read to scale it down: {}", e.message))?;
    let max = usize::try_from(limit).unwrap_or(usize::MAX);
    for _ in 0..HALVINGS {
        img = resize(&img, img.width / 2, img.height / 2);
        if let Some(png) = encode_png_within(&img, max).map_err(|e| e.message)? {
            return Ok((png, true));
        }
    }
    Err(format!("it is {size} MB, over Discord's limit even scaled down"))
}

#[cfg(test)]
#[path = "screens_tests.rs"]
mod tests;
