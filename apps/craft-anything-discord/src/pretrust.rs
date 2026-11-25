//! Trusting a port's project folder for Claude Code in herdr before its build (opt-in with
//! `PRETRUST_PORTS=1`). Claude Code asks "Do you trust this folder?" the first time it starts in a
//! folder, and its lookup of an earlier answer stops at the root of the git repository the folder
//! is in, so with `GITHUB_OWNER` set (each port's project a repository of its own) trusting
//! `PORTS_ROOT` once doesn't cover a new port. With the flag on, the bot records the answer for
//! the project folders it made itself, and nothing else: the folder must be `PORTS_ROOT/<slug>/project`
//! of a port with `bot.json`, checked after following every link. The herdr adapter itself still
//! never answers the dialog.
//!
//! Claude Code 2.1.332 keeps the answer in its global config, `<profile>/.claude.json`, as
//! `projects["<folder>"].hasTrustDialogAccepted = true`, the folder in Windows form with forward
//! slashes (`C:/Projects/ports/app/project`). Only that one key of that one folder is changed; the
//! file must exist and parse, and is replaced atomically. The rest of the file keeps its values,
//! but serde_json (without its `preserve_order` feature, which this workspace doesn't enable)
//! writes the keys sorted and with its own spacing. Claude Code may write the file at the same
//! time: it is read again right before the write, and a change seen then is edited again; what
//! remains is the moment between that read and the rename, and a running Claude Code that later
//! writes its own copy over ours (docs/discord-bot.md, "Claude Code in herdr").

use crate::state::PortState;
use craft_anything_ai::cli_agents::herdr::{self, paths::Host};
use serde_json::{Map, Value};
use std::path::{Component, Path, PathBuf};

/// The key Claude Code records a folder's trust under, in `projects["<folder>"]`.
pub const TRUST_KEY: &str = "hasTrustDialogAccepted";
/// The port's project folder's name inside `PORTS_ROOT/<slug>`.
const PROJECT_DIR: &str = "project";
/// How many times a write is retried when the file changed while it was being edited.
const ATTEMPTS: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TrustError {
    #[error("{0} contains `..`; only a port's own folder is trusted")]
    ParentDir(String),
    #[error("couldn't resolve {path}: {reason}")]
    Resolve { path: String, reason: String },
    #[error("{folder} isn't a port folder directly inside PORTS_ROOT ({root})")]
    Outside { folder: String, root: String },
    #[error("{0} has no bot.json: the bot didn't make this port")]
    NotOurPort(String),
    #[error("Claude Code's config {0} doesn't exist; it is never created here (start Claude Code once on that computer)")]
    ConfigMissing(String),
    #[error("Claude Code's config {path} can't be read as JSON ({reason}); it was left unchanged")]
    ConfigUnparseable { path: String, reason: String },
    #[error("Claude Code's config {path} isn't shaped as expected ({reason}); it was left unchanged")]
    ConfigShape { path: String, reason: String },
    #[error("Claude Code's home folder wasn't found; set CLAUDE_CONFIG_JSON to its .claude.json")]
    NoHome,
    #[error("couldn't write {path}: {reason}")]
    Io { path: String, reason: String },
    #[error("{0} kept changing while it was being edited; try again")]
    Busy(String),
}

/// What a grant did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Granted {
    /// The folder is trusted now.
    Added,
    /// Claude Code already trusted it; the file wasn't written.
    AlreadyTrusted,
}

/// Whether `backend` (`CRAFT_BACKEND`) is Claude Code in herdr, the only backend this applies to.
pub fn uses_herdr(backend: Option<&str>) -> bool {
    backend.map(str::trim).is_some_and(|b| b.eq_ignore_ascii_case(herdr::DISPLAY) || b.eq_ignore_ascii_case(herdr::ID))
}

/// With the flag on and the backend Claude Code in herdr, trusts the project folder of the port in
/// `port_dir` ([`port_project`]) in Claude Code's config ([`config_path`]); `None` when off. Gives
/// the folder as Claude Code knows it and what was done.
pub fn maybe_pretrust(
    enabled: bool,
    backend: Option<&str>,
    ports_root: &Path,
    port_dir: &Path,
    config_override: Option<&Path>,
    host: &Host,
) -> Result<Option<(String, Granted)>, TrustError> {
    if !enabled || !uses_herdr(backend) {
        return Ok(None);
    }
    let project = port_project(ports_root, port_dir)?;
    let key = trust_key(host, &project);
    let config = config_path(config_override, host)?;
    grant(&config, &key).map(|granted| Some((key, granted)))
}

/// The project folder of the port in `port_dir`, every link followed, after checking that the port
/// folder is directly inside `ports_root` (also resolved; not `ports_root` itself), holds the
/// bot's `bot.json`, and that its `project` folder (made when it doesn't exist yet, as the build
/// would) resolves to inside it. A path with `..` is refused before anything is resolved.
pub fn port_project(ports_root: &Path, port_dir: &Path) -> Result<PathBuf, TrustError> {
    for path in [ports_root, port_dir] {
        if path.components().any(|c| matches!(c, Component::ParentDir)) {
            return Err(TrustError::ParentDir(path.display().to_string()));
        }
    }
    let root = resolve(ports_root)?;
    let port = resolve(port_dir)?;
    if port.parent() != Some(root.as_path()) {
        return Err(TrustError::Outside { folder: port.display().to_string(), root: root.display().to_string() });
    }
    if !PortState::path(&port).is_file() {
        return Err(TrustError::NotOurPort(port.display().to_string()));
    }
    let wanted = port.join(PROJECT_DIR);
    match std::fs::create_dir(&wanted) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(TrustError::Resolve { path: wanted.display().to_string(), reason: e.to_string() }),
    }
    let project = resolve(&wanted)?;
    if project.parent() != Some(port.as_path()) || !project.is_dir() {
        return Err(TrustError::Outside { folder: project.display().to_string(), root: root.display().to_string() });
    }
    Ok(project)
}

fn resolve(path: &Path) -> Result<PathBuf, TrustError> {
    std::fs::canonicalize(path).map_err(|e| TrustError::Resolve { path: path.display().to_string(), reason: e.to_string() })
}

/// `folder` as Claude Code keys it in `projects`: the path its side sees, with forward slashes
/// when that side is Windows (Claude Code turns `\` into `/` there).
pub fn trust_key(host: &Host, folder: &Path) -> String {
    let agent = host.to_agent(folder);
    if matches!(host, Host::Wsl { .. }) || cfg!(windows) { agent.replace('\\', "/") } else { agent }
}

/// Claude Code's global config: `CLAUDE_CONFIG_JSON` when set (a Windows path is mapped in WSL),
/// else `.claude.json` in the home folder of the account herdr runs Claude Code under.
pub fn config_path(explicit: Option<&Path>, host: &Host) -> Result<PathBuf, TrustError> {
    if let Some(path) = explicit {
        return Ok(host.from_agent(&path.display().to_string()));
    }
    herdr::agent_home(host).map(|home| home.join(".claude.json")).ok_or(TrustError::NoHome)
}

/// Sets `projects[key].hasTrustDialogAccepted = true` in the config at `config`. The file is read
/// again right before the write; when it changed, the edit is made again on what it holds now.
pub fn grant(config: &Path, key: &str) -> Result<Granted, TrustError> {
    let shown = config.display().to_string();
    let mut before = read(config)?;
    for _ in 0..ATTEMPTS {
        let Some(edited) = with_trust(&before, key, &shown)? else {
            return Ok(Granted::AlreadyTrusted);
        };
        let now = read(config)?;
        if now != before {
            before = now;
            continue;
        }
        craft_anything_platform::fs::write_atomic(config, &edited).map_err(|e| TrustError::Io { path: shown.clone(), reason: e.message })?;
        return Ok(Granted::Added);
    }
    Err(TrustError::Busy(shown))
}

fn read(config: &Path) -> Result<Vec<u8>, TrustError> {
    std::fs::read(config).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => TrustError::ConfigMissing(config.display().to_string()),
        _ => TrustError::Io { path: config.display().to_string(), reason: e.to_string() },
    })
}

/// The config `bytes` with the folder `key` trusted; `None` when it already is. Nothing but that
/// key of that folder changes value; a folder without an entry gets one holding only that key, as
/// Claude Code's own runner writes it.
pub fn with_trust(bytes: &[u8], key: &str, shown: &str) -> Result<Option<Vec<u8>>, TrustError> {
    let shape = |reason: &str| TrustError::ConfigShape { path: shown.to_owned(), reason: reason.to_owned() };
    let mut root: Value = serde_json::from_slice(bytes).map_err(|e| TrustError::ConfigUnparseable { path: shown.to_owned(), reason: e.to_string() })?;
    let top = root.as_object_mut().ok_or_else(|| shape("not a JSON object"))?;
    let projects = top.entry("projects").or_insert_with(|| Value::Object(Map::new())).as_object_mut().ok_or_else(|| shape("`projects` isn't an object"))?;
    let entry = projects.entry(key).or_insert_with(|| Value::Object(Map::new())).as_object_mut().ok_or_else(|| shape("the folder's entry isn't an object"))?;
    if entry.get(TRUST_KEY) == Some(&Value::Bool(true)) {
        return Ok(None);
    }
    entry.insert(TRUST_KEY.to_owned(), Value::Bool(true));
    let mut out = serde_json::to_vec_pretty(&root).map_err(|e| shape(&e.to_string()))?;
    if bytes.ends_with(b"\n") {
        out.push(b'\n');
    }
    Ok(Some(out))
}

#[cfg(test)]
#[path = "pretrust_tests.rs"]
mod tests;
