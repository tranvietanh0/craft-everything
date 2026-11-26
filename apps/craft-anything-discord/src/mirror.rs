//! Projects made in the craft desktop app (or CLI), mirrored in Discord. Craft writes a
//! registration file when it creates a project (`CRAFT_DISCORD_REGISTER`, see
//! `craft_anything_engine::workspace::register`); the bot reads the register folder (default
//! `PORTS_ROOT/.bot/register`), makes the channel, and from then on polls that one project's
//! files: `craft.json` (status, note, checks), `inbox/features.json` (features and their guides)
//! and `usage.json` (tokens and cost). A file is read again only when its size or modification
//! time changed. What changed becomes the same [`Event`]s a `rebuild --events ndjson` run prints,
//! so the board, the tester cards, the Log thread and Plane treat both the same way.
//!
//! What the bot keeps about each mirrored project (where it posts and what it has already seen,
//! so a restart neither makes a second channel nor reports a change twice) is in
//! `PORTS_ROOT/.bot/mirrors.json`, written atomically.

use crate::events::Event;
use crate::saved_queue::BOT_DIR;
use craft_anything_engine::workspace::features::{self, Feature};
use craft_anything_engine::workspace::{MANIFEST, Manifest, ProjectUsage, Status, USAGE_FILE, UsageRead, read_manifest};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

pub const REGISTER_DIR: &str = "register";
/// Registrations already handled, kept for a person to look at.
pub const DONE_DIR: &str = "done";
pub const MIRRORS_FILE: &str = "mirrors.json";
/// The study's notes: once they exist the first look is posted.
pub const STUDY_NOTES: &str = "capture/README.md";
/// The files polled in a mirrored project, in the order they are read.
pub const WATCHED: [&str; 4] = [MANIFEST, features::FEATURES_FILE, USAGE_FILE, STUDY_NOTES];

/// One registration file, as craft writes it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Registration {
    pub folder: String,
    pub name: String,
    #[serde(default)]
    pub request: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub stack: String,
    #[serde(default)]
    pub created_ms: u64,
}

impl Registration {
    pub fn read(path: &Path) -> Result<Registration, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("couldn't read {}: {e}", path.display()))?;
        serde_json::from_slice(&bytes).map_err(|e| format!("{} is not a registration: {e}", path.display()))
    }

    /// The project folder as the bot (in WSL) reads it: a Windows path (`C:\…`) becomes `/mnt/c/…`.
    pub fn folder_path(&self) -> PathBuf {
        PathBuf::from(craft_anything_ai::cli_agents::herdr::paths::windows_to_linux(&self.folder).unwrap_or_else(|| self.folder.clone()))
    }
}

/// The register folder: `explicit`, else `PORTS_ROOT/.bot/register`.
pub fn register_dir(ports_root: &Path, explicit: Option<&Path>) -> PathBuf {
    explicit.map_or_else(|| ports_root.join(BOT_DIR).join(REGISTER_DIR), Path::to_path_buf)
}

/// The registration files waiting in `dir`, oldest first (the names start with the creation time).
pub fn pending(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<PathBuf> =
        entries.flatten().map(|e| e.path()).filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "json")).collect();
    files.sort();
    files
}

/// Moves a handled registration into `done/` beside it.
pub fn finish(file: &Path) -> Result<(), String> {
    let dir = file.parent().ok_or("the registration has no folder")?.join(DONE_DIR);
    std::fs::create_dir_all(&dir).map_err(|e| format!("couldn't create {}: {e}", dir.display()))?;
    let to = dir.join(file.file_name().ok_or("the registration has no name")?);
    std::fs::rename(file, &to).map_err(|e| format!("couldn't move {} to {}: {e}", file.display(), to.display()))
}

/// A file's size and modification time: when neither changed, it is not read again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fingerprint {
    pub len: u64,
    pub modified_ns: u128,
}

impl Fingerprint {
    pub fn of(path: &Path) -> Option<Fingerprint> {
        let meta = std::fs::metadata(path).ok()?;
        let modified_ns = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
        Some(Fingerprint { len: meta.len(), modified_ns })
    }
}

/// The usage last reported, so only the difference becomes a turn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageSeen {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub api_billed_usd: f64,
}

/// What the bot has already seen of one project.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Seen {
    pub files: BTreeMap<String, Fingerprint>,
    pub status: Option<Status>,
    pub note: String,
    pub checks: usize,
    pub features: Vec<Feature>,
    pub usage: UsageSeen,
    pub first_look_posted: bool,
}

/// One mirrored project.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Mirror {
    pub folder: PathBuf,
    pub slug: String,
    pub channel: u64,
    #[serde(default)]
    pub seen: Seen,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Mirrors {
    pub projects: Vec<Mirror>,
}

impl Mirrors {
    pub fn path(ports_root: &Path) -> PathBuf {
        ports_root.join(BOT_DIR).join(MIRRORS_FILE)
    }

    /// No file yet is none; a file that can't be read is an error (never silently empty, which
    /// would make every project's channel again).
    pub fn load(ports_root: &Path) -> Result<Mirrors, String> {
        let path = Self::path(ports_root);
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| format!("{} is not valid: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Mirrors::default()),
            Err(e) => Err(format!("couldn't read {}: {e}", path.display())),
        }
    }

    pub fn save(&self, ports_root: &Path) -> Result<(), String> {
        let path = Self::path(ports_root);
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| format!("couldn't encode the mirrors: {e}"))?;
        craft_anything_platform::fs::write_atomic(&path, &bytes).map_err(|e| format!("couldn't save {}: {}", path.display(), e.message))
    }

    pub fn find(&self, folder: &Path) -> Option<&Mirror> {
        self.projects.iter().find(|m| m.folder == folder)
    }

    pub fn find_mut(&mut self, folder: &Path) -> Option<&mut Mirror> {
        self.projects.iter_mut().find(|m| m.folder == folder)
    }
}

/// What a registration asks of the bot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admit {
    /// Make the channel and start mirroring.
    New,
    /// Already mirrored (a registration seen again after a restart): no second channel.
    Known,
    /// Not mirrored: the reason.
    Skip(String),
}

/// Decides what to do with a registration for `folder`. A folder inside `PORTS_ROOT` is one of
/// the bot's own ports; a folder without `craft.json` is not a project.
pub fn admit(mirrors: &Mirrors, ports_root: &Path, folder: &Path) -> Admit {
    if mirrors.find(folder).is_some() {
        return Admit::Known;
    }
    if folder.starts_with(ports_root) {
        return Admit::Skip(format!("{} is one of the bot's own ports", folder.display()));
    }
    if !folder.join(MANIFEST).is_file() {
        return Admit::Skip(format!("{} has no {MANIFEST}", folder.display()));
    }
    Admit::New
}

/// The files whose fingerprint changed since `seen` (new or gone count too); `seen` is updated.
pub fn changed_files(folder: &Path, seen: &mut Seen) -> Vec<&'static str> {
    let mut changed = Vec::new();
    for name in WATCHED {
        let now = Fingerprint::of(&folder.join(name));
        if seen.files.get(name).copied() != now {
            match now {
                Some(f) => seen.files.insert(name.to_owned(), f),
                None => seen.files.remove(name),
            };
            changed.push(name);
        }
    }
    changed
}

/// What one poll found.
#[derive(Debug, Default, PartialEq)]
pub struct Poll {
    pub events: Vec<Event>,
    /// The project became active again after it had ended: a new run on the board.
    pub restarted: bool,
    /// The manifest as last read, when it was read this time.
    pub manifest: Option<Manifest>,
    /// Files that couldn't be read (reported, then tried again when they change).
    pub problems: Vec<String>,
}

/// Reads the changed files of `folder` and turns what changed into events.
pub fn poll(folder: &Path, seen: &mut Seen) -> Poll {
    let mut out = Poll::default();
    for name in changed_files(folder, seen) {
        let read = match name {
            MANIFEST => read_project(folder).map(|m| manifest_events(&m, seen, &mut out)),
            features::FEATURES_FILE => features::load(folder).map(|f| feature_events(&f, seen, &mut out.events)).map_err(|e| e.message),
            USAGE_FILE => usage_events(ProjectUsage::read(folder), seen, &mut out.events),
            _ => Ok(()),
        };
        if let Err(e) = read {
            out.problems.push(format!("{name}: {e}"));
        }
    }
    out
}

fn read_project(folder: &Path) -> Result<Manifest, String> {
    let bytes = std::fs::read(folder.join(MANIFEST)).map_err(|e| e.to_string())?;
    read_manifest(&bytes).map_err(|e| e.message)
}

fn ended(status: Option<Status>) -> bool {
    matches!(status, Some(Status::Ready | Status::Stopped))
}

/// `craft.json`: a new status is a stage, an end or a wait; a new note is the activity line; new
/// checks are `checked` events.
fn manifest_events(m: &Manifest, seen: &mut Seen, out: &mut Poll) {
    let note = m.note.trim().to_owned();
    if seen.status != Some(m.status) {
        out.restarted |= ended(seen.status) && !ended(Some(m.status)) && m.status != Status::New;
        out.events.extend(status_events(m.status, &note));
    } else if seen.note != note && !note.is_empty() {
        out.events.push(Event::Status { text: note.clone() });
    }
    for c in m.checks.iter().skip(seen.checks) {
        out.events.push(Event::Checked {
            round: u64::from(c.round),
            check: c.check.clone(),
            passed: c.passed,
            similarity: c.similarity,
            note: Some(c.note.clone()).filter(|n| !n.is_empty()),
        });
    }
    seen.checks = m.checks.len();
    seen.status = Some(m.status);
    seen.note = note;
    out.manifest = Some(m.clone());
}

fn step(step: &str, label: &str, note: &str) -> Event {
    Event::Step { step: step.to_owned(), label: label.to_owned(), text: note.to_owned() }
}

/// The events a status change stands for.
pub fn status_events(status: Status, note: &str) -> Vec<Event> {
    let or = |fallback: &str| if note.is_empty() { fallback.to_owned() } else { note.to_owned() };
    match status {
        Status::New => Vec::new(),
        Status::Inspecting => vec![step("study", "Study", &or("Studying the original…"))],
        Status::Building => vec![step("build", "Build", &or("Building…"))],
        Status::Checking => vec![step("check", "Check", &or("Checking the build…"))],
        Status::NeedsYou => vec![Event::Status { text: format!("Waiting for you in the desktop app: {}", or("it needs an answer")) }],
        Status::Ready => vec![step("done", "Done", &or("Ready")), Event::Finished { ok: true, paused: false, message: or("Ready to test") }],
        Status::Stopped => vec![Event::Finished { ok: false, paused: true, message: or("Paused in the desktop app") }],
    }
}

/// `inbox/features.json`: one `feature` event per feature whose record changed.
fn feature_events(now: &[Feature], seen: &mut Seen, events: &mut Vec<Event>) {
    for f in now {
        if seen.features.iter().any(|s| s == f) {
            continue;
        }
        events.push(Event::Feature {
            name: f.name.clone(),
            state: f.state.id().to_owned(),
            guide: f.guide.clone(),
            note: f.note.clone(),
            similarity: f.similarity.map(f64::from),
        });
    }
    seen.features = now.to_vec();
}

/// `usage.json`: what was added since last time, as one `turn_done`. An unreadable file is
/// reported and the baseline kept, so nothing is counted twice when it reads again.
fn usage_events(read: UsageRead, seen: &mut Seen, events: &mut Vec<Event>) -> Result<(), String> {
    let u = match read {
        UsageRead::Unreadable => return Err("it couldn't be read".to_owned()),
        UsageRead::None => return Ok(()),
        UsageRead::Known(u) => u,
    };
    let was = seen.usage;
    let input = u.input_tokens.saturating_sub(was.input_tokens);
    let output = u.output_tokens.saturating_sub(was.output_tokens);
    let cost = (u.cost_usd - was.cost_usd).max(0.0);
    let billed = (u.api_billed_usd - was.api_billed_usd).max(0.0);
    seen.usage = UsageSeen { input_tokens: u.input_tokens, output_tokens: u.output_tokens, cost_usd: u.cost_usd, api_billed_usd: u.api_billed_usd };
    if input == 0 && output == 0 && cost <= 0.0 {
        return Ok(());
    }
    events.push(Event::TurnDone {
        backend: "desktop app".to_owned(),
        input_tokens: input,
        output_tokens: output,
        tokens_unknown: u.tokens_incomplete,
        cost_usd: (u.priced && cost > 0.0).then_some(cost),
        cost_is_estimate: cost > billed,
    });
    Ok(())
}

#[cfg(test)]
#[path = "mirror_tests.rs"]
mod tests;
