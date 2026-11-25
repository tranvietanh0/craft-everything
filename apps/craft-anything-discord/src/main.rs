//! `craft-anything-discord`: the porter bot. Loads the env file (`CRAFT_DISCORD_ENV`, default
//! `~/.config/craft-discord/.env`), signs in, registers `/port` in the guild and serves it until
//! Ctrl+C or SIGTERM. Setup and running: docs/discord-bot.md.

mod discord;

use craft_anything_discord::config::{self, Config};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

/// Names the log level variable (`error`, `warn`, `info`, `debug`; default `info`).
const LOG_VAR: &str = "CRAFT_DISCORD_LOG";
/// How long running work gets to stop once the bot is asked to quit.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

fn main() -> ExitCode {
    init_logging();
    let config = match load_config() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("craft-anything-discord: {e}");
            return ExitCode::FAILURE;
        }
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("craft-anything-discord: couldn't start the async runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    let served = runtime.block_on(discord::serve(config));
    // A running port's task is dropped here, which stops its rebuild child.
    runtime.shutdown_timeout(SHUTDOWN_GRACE);
    match served {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("craft-anything-discord: {e}");
            ExitCode::FAILURE
        }
    }
}

fn load_config() -> Result<Config, String> {
    let explicit = std::env::var(config::ENV_FILE_VAR).ok();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let path = config::env_file_path(explicit.as_deref(), home.as_deref()).map_err(|e| e.to_string())?;
    let text = std::fs::read_to_string(&path).map_err(|e| format!("couldn't read the env file {}: {e}", path.display()))?;
    let map = config::parse_env(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    Config::from_map(&map).map_err(|e| format!("{}: {e}", path.display()))
}

struct Stderr(log::LevelFilter);

/// Our own log targets start with this; everything else (serenity's request spans) is only
/// shown from `warn` up, as it logs every HTTP request at `info`.
const OWN_TARGETS: &str = "craft_anything";

impl log::Log for Stderr {
    fn enabled(&self, m: &log::Metadata<'_>) -> bool {
        let limit = if m.target().starts_with(OWN_TARGETS) { self.0 } else { self.0.min(log::LevelFilter::Warn) };
        m.level() <= limit
    }

    fn log(&self, r: &log::Record<'_>) {
        if self.enabled(r.metadata()) {
            eprintln!("[{}] {}: {}", r.level(), r.target(), r.args());
        }
    }

    fn flush(&self) {}
}

fn init_logging() {
    let level = match std::env::var(LOG_VAR).unwrap_or_default().to_ascii_lowercase().as_str() {
        "error" => log::LevelFilter::Error,
        "warn" => log::LevelFilter::Warn,
        "debug" => log::LevelFilter::Debug,
        "trace" => log::LevelFilter::Trace,
        "off" => log::LevelFilter::Off,
        _ => log::LevelFilter::Info,
    };
    if log::set_logger(Box::leak(Box::new(Stderr(level)))).is_ok() {
        log::set_max_level(level);
    }
}
