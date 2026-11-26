//! Whether the Discord porter bot follows a project, from the bot's own files (docs/discord-bot.md).
//! No agent posts to Discord: the bot reads the project's files and posts what changed, so this is
//! read from what the bot saved, never from a conversation.

use serde_json::Value;
use std::path::{Path, PathBuf};

/// How the bot knows a project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BotLink {
    /// One of the bot's own ports (`PORTS_ROOT/<slug>/project`, with the bot's `bot.json` beside).
    Port { slug: String, label: String, runs: usize, cards: usize },
    /// A project made in the app that the bot mirrors (`PORTS_ROOT/.bot/mirrors.json`).
    Mirror { slug: String, status: Option<String>, features: usize, checks: usize, first_look_posted: bool },
    /// Registered for the bot (`CRAFT_DISCORD_REGISTER`), not picked up by it yet.
    Waiting { file: PathBuf },
    /// Not followed: why.
    None { why: String },
    /// A bot file that should say couldn't be read.
    Unreadable { why: String },
}

/// How the bot knows `project`. `register_dir` is `CRAFT_DISCORD_REGISTER` (the bot's
/// `PORTS_ROOT/.bot/register` when set up as the docs say).
pub fn bot_link(project: &Path, register_dir: Option<&Path>) -> BotLink {
    if let Some(port) = project.parent().filter(|_| project.file_name().is_some_and(|n| n == "project")) {
        let bot = port.join("bot.json");
        if bot.is_file() {
            return match read(&bot) {
                Ok(v) => BotLink::Port {
                    slug: text(&v, "slug"),
                    label: text(&v, "label"),
                    runs: v.get("jobs").and_then(Value::as_array).map_or(0, Vec::len),
                    cards: v.get("cards").and_then(Value::as_array).map_or(0, Vec::len),
                },
                Err(why) => BotLink::Unreadable { why },
            };
        }
    }
    let Some(register) = register_dir else {
        return BotLink::None { why: "CRAFT_DISCORD_REGISTER isn't set, so new projects aren't offered to the bot.".into() };
    };
    if let Some(ports_root) = register.parent().filter(|b| b.file_name().is_some_and(|n| n == ".bot")).and_then(Path::parent) {
        let mirrors = ports_root.join(".bot").join("mirrors.json");
        match read_optional(&mirrors) {
            Ok(Some(v)) => {
                let found = v.get("projects").and_then(Value::as_array).into_iter().flatten().find(|m| same_folder(m.get("folder"), project));
                if let Some(m) = found {
                    let seen = m.get("seen").unwrap_or(&Value::Null);
                    return BotLink::Mirror {
                        slug: text(m, "slug"),
                        status: seen.get("status").and_then(Value::as_str).map(str::to_string),
                        features: seen.get("features").and_then(Value::as_array).map_or(0, Vec::len),
                        checks: seen.get("checks").and_then(Value::as_u64).map_or(0, |n| usize::try_from(n).unwrap_or(usize::MAX)),
                        first_look_posted: seen.get("first_look_posted").and_then(Value::as_bool).unwrap_or(false),
                    };
                }
            }
            Ok(None) => {}
            Err(why) => return BotLink::Unreadable { why },
        }
    }
    if let Some(file) = waiting_registration(register, project) {
        return BotLink::Waiting { file };
    }
    BotLink::None { why: "The bot has no record of this project. Projects made before CRAFT_DISCORD_REGISTER was set aren't picked up.".into() }
}

/// A registration file in `dir` (not yet moved to `done/` by the bot) for `project`.
fn waiting_registration(dir: &Path, project: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|e| e == "json") && read(p).is_ok_and(|v| same_folder(v.get("folder"), project)))
}

fn same_folder(folder: Option<&Value>, project: &Path) -> bool {
    folder.and_then(Value::as_str).is_some_and(|f| Path::new(f.trim_end_matches('/')) == project)
}

fn read(path: &Path) -> Result<Value, String> {
    read_optional(path)?.ok_or_else(|| format!("{} isn't there", path.display()))
}

/// `None` when the file isn't there.
fn read_optional(path: &Path) -> Result<Option<Value>, String> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|e| format!("{} isn't valid: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("couldn't read {}: {e}", path.display())),
    }
}

fn text(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}
