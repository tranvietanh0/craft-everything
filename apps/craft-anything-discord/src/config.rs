//! The bot's env file: `KEY=VALUE` lines, `#` comments, optional `export ` and matching quotes.
//! A missing or malformed key is an error that names the key. The token is never printed.

use crate::daily::Schedule;
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

/// Names the env file location override.
pub const ENV_FILE_VAR: &str = "CRAFT_DISCORD_ENV";
/// Default upload cap for an APK or XAPK, when `MAX_UPLOAD_MB` is not set.
pub const DEFAULT_MAX_UPLOAD_MB: u64 = 200;
const BYTES_PER_MB: u64 = 1024 * 1024;
/// How often a mirrored app project's files are looked at, when `MIRROR_POLL_SECS` is not set.
pub const DEFAULT_MIRROR_POLL_SECS: u64 = 5;
const MAX_MIRROR_POLL_SECS: u64 = 3_600;

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

/// A secret value (the bot token, a Plane API key). Its `Debug` output is redacted, so a `Config` can be logged safely.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

/// Plane sync settings. Present only when all three `PLANE_*` keys are set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaneSettings {
    /// The Plane instance root with no trailing slash, such as `http://localhost:8080`.
    pub url: String,
    pub api_key: Secret,
    pub workspace: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub token: Secret,
    pub app_id: u64,
    pub guild_id: u64,
    pub ports_category_id: u64,
    pub porter_role_id: u64,
    pub tester_role_id: u64,
    pub daily_channel_id: u64,
    pub ports_root: PathBuf,
    pub craft_cli: PathBuf,
    pub max_upload_bytes: u64,
    /// Optional Plane sync: on only when `PLANE_URL`, `PLANE_API_KEY` and `PLANE_WORKSPACE` are all set.
    pub plane: Option<PlaneSettings>,
    /// Optional GitHub repo per port, owned by this user or organisation. On only when `GITHUB_OWNER` is set.
    pub github_owner: Option<String>,
    /// The craft backend every rebuild uses (`--backend`): `CRAFT_BACKEND`, by default Claude Code
    /// in herdr. Set to an empty value, no `--backend` is passed and craft picks its own.
    pub backend: Option<String>,
    /// When the daily summary goes to `DAILY_CHANNEL_ID`: `DAILY_SUMMARY_TIME` (default 09:00) in
    /// `DAILY_SUMMARY_UTC_OFFSET`, or the system's time zone when that is not set.
    pub daily: Schedule,
    /// `PRETRUST_PORTS`: trust each port's project folder in Claude Code's config before a herdr
    /// build ([`crate::pretrust`]). Off unless set to `1`, `true`, `yes` or `on`.
    pub pretrust_ports: bool,
    /// `CLAUDE_CONFIG_JSON`: Claude Code's `.claude.json` for `PRETRUST_PORTS`, when it isn't in the
    /// home folder herdr runs Claude Code under.
    pub claude_config_json: Option<PathBuf>,
    /// `MIRROR_APP_PROJECTS`: mirror projects the craft desktop app (or CLI) registers when it
    /// creates them ([`crate::mirror`]). Off unless set to `1`.
    pub mirror_apps: bool,
    /// `MIRROR_REGISTER_DIR`: where craft writes the registrations; default `PORTS_ROOT/.bot/register`.
    pub mirror_register_dir: Option<PathBuf>,
    /// `MIRROR_POLL_SECS`: how often the register folder and each mirrored project are looked at.
    pub mirror_poll: std::time::Duration,
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
            token: Secret(required(map, "DISCORD_TOKEN")?.to_owned()),
            app_id: snowflake(map, "DISCORD_APP_ID")?,
            guild_id: snowflake(map, "DISCORD_GUILD_ID")?,
            ports_category_id: snowflake(map, "PORTS_CATEGORY_ID")?,
            porter_role_id: snowflake(map, "PORTER_ROLE_ID")?,
            tester_role_id: snowflake(map, "TESTER_ROLE_ID")?,
            daily_channel_id: snowflake(map, "DAILY_CHANNEL_ID")?,
            ports_root: PathBuf::from(required(map, "PORTS_ROOT")?),
            craft_cli: PathBuf::from(required(map, "CRAFT_CLI")?),
            max_upload_bytes: max_upload_bytes(map)?,
            plane: plane_settings(map)?,
            github_owner: github_owner(map)?,
            backend: backend(map),
            daily: Schedule::parse(optional(map, "DAILY_SUMMARY_TIME"), optional(map, "DAILY_SUMMARY_UTC_OFFSET"))
                .map_err(|(key, reason)| ConfigError::Invalid { key, reason })?,
            pretrust_ports: flag(map, "PRETRUST_PORTS")?,
            claude_config_json: optional(map, "CLAUDE_CONFIG_JSON").map(PathBuf::from),
            mirror_apps: flag(map, "MIRROR_APP_PROJECTS")?,
            mirror_register_dir: optional(map, "MIRROR_REGISTER_DIR").map(PathBuf::from),
            mirror_poll: mirror_poll(map)?,
        })
    }
}

fn required<'a>(map: &'a BTreeMap<String, String>, key: &'static str) -> Result<&'a str, ConfigError> {
    map.get(key).map(|v| v.trim()).filter(|v| !v.is_empty()).ok_or(ConfigError::Missing { key })
}

fn optional<'a>(map: &'a BTreeMap<String, String>, key: &str) -> Option<&'a str> {
    map.get(key).map(|v| v.trim()).filter(|v| !v.is_empty())
}

/// An on/off key: off when absent or empty; `1`, `true`, `yes` or `on` (any case) is on, and
/// `0`, `false`, `no` or `off` is off. Anything else is an error, so a typo doesn't turn it off quietly.
fn flag(map: &BTreeMap<String, String>, key: &'static str) -> Result<bool, ConfigError> {
    match optional(map, key).map(str::to_ascii_lowercase).as_deref() {
        None | Some("0" | "false" | "no" | "off") => Ok(false),
        Some("1" | "true" | "yes" | "on") => Ok(true),
        Some(_) => Err(ConfigError::Invalid { key, reason: "expected 1 or 0".to_owned() }),
    }
}

/// The bot's default backend: the person's own Claude Code, in a herdr pane they can watch.
pub const DEFAULT_BACKEND: &str = craft_anything_ai::cli_agents::herdr::DISPLAY;

fn backend(map: &BTreeMap<String, String>) -> Option<String> {
    match map.get("CRAFT_BACKEND").map(|v| v.trim()) {
        None => Some(DEFAULT_BACKEND.to_owned()),
        Some("") => None,
        Some(name) => Some(name.to_owned()),
    }
}

const PLANE_KEYS: [&str; 3] = ["PLANE_URL", "PLANE_API_KEY", "PLANE_WORKSPACE"];

/// Plane is off when none of its keys is set. Once any is set, the others are required, so a
/// half-configured Plane is an error that names the missing key rather than a silent skip.
fn plane_settings(map: &BTreeMap<String, String>) -> Result<Option<PlaneSettings>, ConfigError> {
    if PLANE_KEYS.iter().all(|key| optional(map, key).is_none()) {
        return Ok(None);
    }
    let url = required(map, "PLANE_URL")?;
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(ConfigError::Invalid { key: "PLANE_URL", reason: "expected an http:// or https:// URL".to_owned() });
    }
    Ok(Some(PlaneSettings {
        url: url.trim_end_matches('/').to_owned(),
        api_key: Secret(required(map, "PLANE_API_KEY")?.to_owned()),
        workspace: required(map, "PLANE_WORKSPACE")?.to_owned(),
    }))
}

/// GitHub owner names: 1 to 39 letters, digits or hyphens, not starting or ending with a hyphen.
fn github_owner(map: &BTreeMap<String, String>) -> Result<Option<String>, ConfigError> {
    let Some(owner) = optional(map, "GITHUB_OWNER") else {
        return Ok(None);
    };
    let valid = owner.len() <= 39 && !owner.starts_with('-') && !owner.ends_with('-') && owner.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    if valid {
        return Ok(Some(owner.to_owned()));
    }
    Err(ConfigError::Invalid { key: "GITHUB_OWNER", reason: "expected a GitHub user or organisation name".to_owned() })
}

fn snowflake(map: &BTreeMap<String, String>, key: &'static str) -> Result<u64, ConfigError> {
    required(map, key)?.parse::<u64>().ok().filter(|id| *id > 0).ok_or_else(|| ConfigError::Invalid { key, reason: "expected a numeric Discord ID".to_owned() })
}

fn mirror_poll(map: &BTreeMap<String, String>) -> Result<std::time::Duration, ConfigError> {
    const KEY: &str = "MIRROR_POLL_SECS";
    let Some(raw) = optional(map, KEY) else {
        return Ok(std::time::Duration::from_secs(DEFAULT_MIRROR_POLL_SECS));
    };
    raw.parse::<u64>()
        .ok()
        .filter(|s| (1..=MAX_MIRROR_POLL_SECS).contains(s))
        .map(std::time::Duration::from_secs)
        .ok_or_else(|| ConfigError::Invalid { key: KEY, reason: format!("expected whole seconds from 1 to {MAX_MIRROR_POLL_SECS}") })
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
            "PORTS_ROOT=/srv/craft/ports",
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
    fn rebuilds_use_herdr_unless_the_env_file_says_otherwise() {
        let backend = |extra: &str| parse_env(&format!("{}\n{extra}", full_env())).and_then(|m| Config::from_map(&m)).map(|c| c.backend);
        assert_eq!(backend(""), Ok(Some(DEFAULT_BACKEND.to_owned())));
        assert_eq!(backend("CRAFT_BACKEND=Claude Code"), Ok(Some("Claude Code".to_owned())));
        assert_eq!(backend("CRAFT_BACKEND="), Ok(None), "empty: craft chooses");
    }

    #[test]
    fn the_daily_summary_time_is_checked() {
        let at = |extra: &str| {
            load(&format!(
                "{}
{extra}",
                full_env()
            ))
            .map(|c| c.daily.at.to_string())
        };
        assert_eq!(at(""), Ok("09:00:00".to_owned()));
        assert_eq!(at("DAILY_SUMMARY_TIME=18:30"), Ok("18:30:00".to_owned()));
        assert!(matches!(at("DAILY_SUMMARY_TIME=6pm"), Err(ConfigError::Invalid { key: "DAILY_SUMMARY_TIME", .. })));
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
        let home = Path::new("/srv/example-home");
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

    fn load(text: &str) -> Result<Config, ConfigError> {
        parse_env(text).and_then(|m| Config::from_map(&m))
    }

    #[test]
    fn plane_and_github_are_off_when_their_keys_are_absent() {
        let config = load(&full_env()).expect("the required keys load");
        assert_eq!((config.plane, config.github_owner), (None, None));
    }

    #[test]
    fn a_full_plane_block_loads_with_a_trimmed_url() {
        let text = format!("{}\nPLANE_URL=http://localhost:8080/\nPLANE_API_KEY=plane-key\nPLANE_WORKSPACE=craft", full_env());
        let plane = load(&text).expect("the Plane keys load").plane.expect("Plane is on");
        assert_eq!((plane.url.as_str(), plane.workspace.as_str(), plane.api_key.expose()), ("http://localhost:8080", "craft", "plane-key"));
    }

    #[test]
    fn a_half_configured_plane_names_the_missing_key() {
        let text = format!("{}\nPLANE_URL=http://localhost:8080\nPLANE_WORKSPACE=craft", full_env());
        assert_eq!(load(&text), Err(ConfigError::Missing { key: "PLANE_API_KEY" }));
    }

    #[test]
    fn a_plane_url_without_a_scheme_is_refused() {
        let text = format!("{}\nPLANE_URL=localhost:8080\nPLANE_API_KEY=k\nPLANE_WORKSPACE=craft", full_env());
        assert!(matches!(load(&text), Err(ConfigError::Invalid { key: "PLANE_URL", .. })));
    }

    #[test]
    fn the_plane_key_never_appears_in_debug_output() {
        let text = format!("{}\nPLANE_URL=http://localhost:8080\nPLANE_API_KEY=plane-secret-7\nPLANE_WORKSPACE=craft", full_env());
        let shown = format!("{:?}", load(&text).expect("loads"));
        assert!(!shown.contains("plane-secret-7") && shown.contains("<redacted>"));
    }

    #[test]
    fn pretrusting_ports_is_off_unless_asked_for() {
        let flag = |extra: &str| load(&format!("{}\n{extra}", full_env())).map(|c| c.pretrust_ports);
        assert_eq!(flag(""), Ok(false));
        assert_eq!(flag("PRETRUST_PORTS="), Ok(false));
        assert_eq!(flag("PRETRUST_PORTS=0"), Ok(false));
        assert_eq!(flag("PRETRUST_PORTS=1"), Ok(true));
        assert_eq!(flag("PRETRUST_PORTS=Yes"), Ok(true));
        assert!(matches!(flag("PRETRUST_PORTS=sure"), Err(ConfigError::Invalid { key: "PRETRUST_PORTS", .. })));
        let config = load(&format!("{}\nCLAUDE_CONFIG_JSON=/mnt/c/Users/me/.claude.json", full_env())).expect("loads");
        assert_eq!(config.claude_config_json, Some(PathBuf::from("/mnt/c/Users/me/.claude.json")));
    }

    #[test]
    fn mirroring_app_projects_is_off_unless_asked_for_and_polls_every_five_seconds() {
        let config = load(&full_env()).expect("loads");
        assert_eq!((config.mirror_apps, config.mirror_register_dir, config.mirror_poll.as_secs()), (false, None, DEFAULT_MIRROR_POLL_SECS));
        let on = load(&format!("{}
MIRROR_APP_PROJECTS=1
MIRROR_REGISTER_DIR=/srv/reg
MIRROR_POLL_SECS=2", full_env())).expect("loads");
        assert_eq!((on.mirror_apps, on.mirror_register_dir, on.mirror_poll.as_secs()), (true, Some(PathBuf::from("/srv/reg")), 2));
        for bad in ["0", "abc", "3601"] {
            let text = format!("{}
MIRROR_POLL_SECS={bad}", full_env());
            assert!(matches!(load(&text), Err(ConfigError::Invalid { key: "MIRROR_POLL_SECS", .. })), "{bad}");
        }
    }

    #[test]
    fn a_github_owner_is_checked_against_github_name_rules() {
        let good = format!("{}\nGITHUB_OWNER=tranvietanh0", full_env());
        assert_eq!(load(&good).expect("loads").github_owner.as_deref(), Some("tranvietanh0"));
        let too_long = "a".repeat(40);
        for bad in ["bad owner!", "-leading", "trailing-", too_long.as_str()] {
            let text = format!("{}\nGITHUB_OWNER={bad}", full_env());
            assert!(matches!(load(&text), Err(ConfigError::Invalid { key: "GITHUB_OWNER", .. })), "{bad}");
        }
    }
}
