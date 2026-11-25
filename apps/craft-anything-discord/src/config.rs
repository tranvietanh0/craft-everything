//! The bot's env file: `KEY=VALUE` lines, `#` comments, optional `export ` and matching quotes.
//! A missing or malformed key is an error that names the key. The token is never printed.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

/// Names the env file location override.
pub const ENV_FILE_VAR: &str = "CRAFT_DISCORD_ENV";
/// Default upload cap for an APK or XAPK, when `MAX_UPLOAD_MB` is not set.
pub const DEFAULT_MAX_UPLOAD_MB: u64 = 200;
const BYTES_PER_MB: u64 = 1024 * 1024;

const REQUIRED_KEYS: [&str; 9] = [
    "DISCORD_TOKEN",
    "DISCORD_APP_ID",
    "DISCORD_GUILD_ID",
    "PORTS_CATEGORY_ID",
    "PORTER_ROLE_ID",
    "TESTER_ROLE_ID",
    "DAILY_CHANNEL_ID",
    "PORTS_ROOT",
    "CRAFT_CLI",
];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    #[error("{key} is missing from the env file")]
    Missing { key: &'static str },
    #[error("{key} is not valid: {reason}")]
    Invalid { key: &'static str, reason: String },
    #[error("line {line} of the env file is not KEY=VALUE")]
    BadLine { line: usize },
    #[error("no env file path: set CRAFT_DISCORD_ENV or HOME")]
    NoHome,
}

/// The bot token. Its `Debug` output is redacted, so a `Config` can be logged safely.
#[derive(Clone, PartialEq, Eq)]
pub struct Token(String);

impl Token {
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(<redacted>)")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub token: Token,
    pub app_id: u64,
    pub guild_id: u64,
    pub ports_category_id: u64,
    pub porter_role_id: u64,
    pub tester_role_id: u64,
    pub daily_channel_id: u64,
    pub ports_root: PathBuf,
    pub craft_cli: PathBuf,
    pub max_upload_bytes: u64,
}

/// Where the env file is: the `CRAFT_DISCORD_ENV` override, else `<home>/.config/craft-discord/.env`.
pub fn env_file_path(explicit: Option<&str>, home: Option<&Path>) -> Result<PathBuf, ConfigError> {
    if let Some(path) = explicit.map(str::trim).filter(|p| !p.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    let home = home.ok_or(ConfigError::NoHome)?;
    Ok(home.join(".config").join("craft-discord").join(".env"))
}

/// Parses the env file text. Blank lines and `#` comments are skipped; any other line must be `KEY=VALUE`.
pub fn parse_env(text: &str) -> Result<BTreeMap<String, String>, ConfigError> {
    let mut map = BTreeMap::new();
    for (index, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").map_or(line, str::trim_start);
        let bad = || ConfigError::BadLine { line: index + 1 };
        let (key, value) = line.split_once('=').ok_or_else(bad)?;
        let key = key.trim();
        if key.is_empty() {
            return Err(bad());
        }
        map.insert(key.to_owned(), unquote(value.trim()).to_owned());
    }
    Ok(map)
}

fn unquote(value: &str) -> &str {
    for quote in ['"', '\''] {
        if let Some(inner) = value.strip_prefix(quote).and_then(|v| v.strip_suffix(quote)) {
            return inner;
        }
    }
    value
}

impl Config {
    /// Checks every required key and converts it; the first problem found is reported.
    pub fn from_map(map: &BTreeMap<String, String>) -> Result<Config, ConfigError> {
        for key in REQUIRED_KEYS {
            required(map, key)?;
        }
        Ok(Config {
            token: Token(required(map, "DISCORD_TOKEN")?.to_owned()),
            app_id: snowflake(map, "DISCORD_APP_ID")?,
            guild_id: snowflake(map, "DISCORD_GUILD_ID")?,
            ports_category_id: snowflake(map, "PORTS_CATEGORY_ID")?,
            porter_role_id: snowflake(map, "PORTER_ROLE_ID")?,
            tester_role_id: snowflake(map, "TESTER_ROLE_ID")?,
            daily_channel_id: snowflake(map, "DAILY_CHANNEL_ID")?,
            ports_root: PathBuf::from(required(map, "PORTS_ROOT")?),
            craft_cli: PathBuf::from(required(map, "CRAFT_CLI")?),
            max_upload_bytes: max_upload_bytes(map)?,
        })
    }
}

fn required<'a>(map: &'a BTreeMap<String, String>, key: &'static str) -> Result<&'a str, ConfigError> {
    map.get(key).map(|v| v.trim()).filter(|v| !v.is_empty()).ok_or(ConfigError::Missing { key })
}

fn snowflake(map: &BTreeMap<String, String>, key: &'static str) -> Result<u64, ConfigError> {
    required(map, key)?
        .parse::<u64>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(|| ConfigError::Invalid { key, reason: "expected a numeric Discord ID".to_owned() })
}

fn max_upload_bytes(map: &BTreeMap<String, String>) -> Result<u64, ConfigError> {
    const KEY: &str = "MAX_UPLOAD_MB";
    let Some(raw) = map.get(KEY).map(|v| v.trim()).filter(|v| !v.is_empty()) else {
        return Ok(DEFAULT_MAX_UPLOAD_MB * BYTES_PER_MB);
    };
    let invalid = |reason: &str| ConfigError::Invalid { key: KEY, reason: reason.to_owned() };
    let mb = raw.parse::<u64>().ok().filter(|mb| *mb > 0).ok_or_else(|| invalid("expected a whole number of megabytes above zero"))?;
    mb.checked_mul(BYTES_PER_MB).ok_or_else(|| invalid("too large"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_env() -> String {
        [
            "# the bot",
            "DISCORD_TOKEN=\"abc.def.ghi\"",
            "export DISCORD_APP_ID=111",
            "DISCORD_GUILD_ID=222",
            "PORTS_CATEGORY_ID=333",
            "PORTER_ROLE_ID=444",
            "TESTER_ROLE_ID=555",
            "DAILY_CHANNEL_ID=666",
            "PORTS_ROOT=/home/tranv/ports",
            "CRAFT_CLI=/usr/local/bin/craft-anything-cli",
        ]
        .join("\n")
    }

    #[test]
    fn a_complete_env_file_loads() {
        let config = parse_env(&full_env()).and_then(|m| Config::from_map(&m));
        assert!(config.is_ok_and(|c| c.guild_id == 222 && c.max_upload_bytes == DEFAULT_MAX_UPLOAD_MB * BYTES_PER_MB));
    }

    #[test]
    fn a_missing_key_is_named() {
        let text = full_env().replace("PORTER_ROLE_ID=444", "");
        let err = parse_env(&text).and_then(|m| Config::from_map(&m));
        assert_eq!(err, Err(ConfigError::Missing { key: "PORTER_ROLE_ID" }));
    }

    #[test]
    fn a_non_numeric_id_is_named() {
        let text = full_env().replace("TESTER_ROLE_ID=555", "TESTER_ROLE_ID=role");
        let err = parse_env(&text).and_then(|m| Config::from_map(&m));
        assert!(matches!(err, Err(ConfigError::Invalid { key: "TESTER_ROLE_ID", .. })));
    }

    #[test]
    fn a_line_without_equals_is_reported_with_its_number() {
        assert_eq!(parse_env("A=1\nnot a pair\n"), Err(ConfigError::BadLine { line: 2 }));
    }

    #[test]
    fn the_token_never_appears_in_debug_output() {
        let config = parse_env(&full_env()).and_then(|m| Config::from_map(&m));
        let shown = format!("{config:?}");
        assert!(!shown.contains("abc.def.ghi"));
        assert!(shown.contains("<redacted>"));
    }

    #[test]
    fn the_env_override_beats_the_home_default() {
        let home = Path::new("/home/tranv");
        assert_eq!(env_file_path(Some(" /etc/bot.env "), Some(home)), Ok(PathBuf::from("/etc/bot.env")));
        assert_eq!(env_file_path(None, Some(home)), Ok(home.join(".config").join("craft-discord").join(".env")));
        assert_eq!(env_file_path(Some(""), None), Err(ConfigError::NoHome));
    }

    #[test]
    fn the_upload_cap_is_configurable() {
        let text = format!("{}\nMAX_UPLOAD_MB=50", full_env());
        let config = parse_env(&text).and_then(|m| Config::from_map(&m));
        assert!(config.is_ok_and(|c| c.max_upload_bytes == 50 * BYTES_PER_MB));
    }
}
