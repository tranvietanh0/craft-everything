//! Claude Code in a herdr pane: the person's own interactive Claude Code (their sign-in, skills,
//! hooks and rules), started and prompted through herdr, a terminal workspace manager for coding
//! agents, so every run is a pane they can watch and answer in. Progress is read from Claude Code's
//! session transcript, turned into the same events as the headless adapter's stream. How it
//! behaves and what it can't do: docs/claude-code.md, "Claude Code in herdr".

pub mod args;
pub mod client;
mod conversation;
pub mod paths;
mod session;
pub mod transcript;

pub use conversation::HerdrConversation;

use super::adapters::ClaudeCode;
use super::{Access, AgentAdapter, ArgsCx};
use crate::BackendConfig;
use crate::config::BackendOptions;
use craft_anything_engine::CancelToken;
use craft_anything_evidence::{ErrorCode, ToolError};
use craft_anything_platform::process::Process;
use paths::Host;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

/// The agent id of Claude Code run through herdr.
pub const ID: &str = "claude-code-herdr";
/// Its name in settings and for `--backend`.
pub const DISPLAY: &str = "Claude Code (herdr)";
/// The environment variable naming the herdr program.
pub const HERDR_VAR: &str = "CRAFT_HERDR";
/// How a status that Claude Code waits for the person in herdr starts (the Discord board shows it
/// as paused).
pub const WAITING_STATUS: &str = "Waiting for approval in herdr";

/// Claude Code, interactive, in a herdr pane.
#[derive(Default)]
pub struct ClaudeCodeHerdr;

impl AgentAdapter for ClaudeCodeHerdr {
    fn ids(&self) -> &'static [&'static str] {
        &[ID]
    }

    /// The headless adapter's arguments, made interactive ([`args::interactive`]).
    fn args(&self, cx: &ArgsCx) -> Vec<String> {
        args::interactive(ClaudeCode::default().args(cx))
    }

    fn answers_questions(&self) -> bool {
        true
    }

    fn resumes(&self) -> bool {
        true
    }

    fn plain_fallback(&self) -> bool {
        false
    }

    /// A Clean room builder needs every read to go through our approval tool, and here the person
    /// approves in the pane instead, where one click could open the original. Chats and questions
    /// in Clean room keep their refusals: deny rules hold however the person answers.
    fn validate_access(&self, access: Access) -> Result<(), ToolError> {
        if access == Access::CleanRoomBuild {
            return Err(ToolError::new(
                ErrorCode::BadSettings,
                format!(
                    "{DISPLAY} can't build in Clean room: its approvals happen in the herdr pane, where reading inside the original can't be refused for you."
                ),
            )
            .with_remediation("Build in Deep mode (--deep), or choose Claude Code (without herdr) as the builder for Clean room."));
        }
        Ok(())
    }

    fn check_tool_server(&self, stop: &CancelToken) -> Result<(), ToolError> {
        let herdr = program(&BackendOptions::default(), &Host::current())?;
        if let Err(e) = client::status(&client::HerdrCli { program: herdr.clone() }) {
            return Err(ToolError::new(ErrorCode::EngineMissing, format!("herdr at {herdr} isn't answering: {}", e.message))
                .with_remediation("Start herdr on this computer (open it once), then try again."));
        }
        super::cli_can_start(craft_anything_platform::self_cli_path().as_deref(), stop)
    }

    fn model_in_use(&self, options: Option<&BackendOptions>) -> Option<String> {
        ClaudeCode::default().model_in_use(options)
    }

    fn model_choices(&self, config: &BackendConfig, options: &BackendOptions) -> Result<Vec<String>, ToolError> {
        ClaudeCode::default().model_choices(config, options)
    }

    fn offers_own_default(&self) -> bool {
        true
    }

    fn sign_in_fix(&self) -> Option<String> {
        Some("Sign in to Claude Code where herdr runs it: open a terminal there and run `claude auth login`.".into())
    }
}

/// The herdr program: the backend's chosen one, else `CRAFT_HERDR`, else `herdr` on PATH, else
/// the newest standalone install in the home folder herdr runs under.
pub fn program(options: &BackendOptions, host: &Host) -> Result<String, ToolError> {
    let chosen = options.command.as_deref().map(str::trim).filter(|c| !c.is_empty() && *c != "herdr").map(str::to_string);
    let env = std::env::var(HERDR_VAR).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    if let Some(p) = chosen.or(env) {
        return Ok(p);
    }
    if let Some(p) = craft_anything_platform::which("herdr").or_else(|| craft_anything_platform::which("herdr.exe")) {
        return Ok(p.display().to_string());
    }
    agent_home(host).and_then(|home| newest_install(&home)).map(|p| p.display().to_string()).ok_or_else(|| {
        ToolError::new(ErrorCode::EngineMissing, "herdr wasn't found.").with_remediation(format!("Install herdr, or set {HERDR_VAR} to its program."))
    })
}

/// `<home>/.herdr/packages/standalone/releases/<version>/herdr(.exe)`, the newest version first.
fn newest_install(home: &Path) -> Option<PathBuf> {
    let releases = home.join(".herdr").join("packages").join("standalone").join("releases");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(releases).ok()?.filter_map(|e| e.ok().map(|e| e.path())).collect();
    dirs.sort_by_key(|d| version_key(d));
    dirs.iter().rev().flat_map(|d| [d.join("herdr.exe"), d.join("herdr")]).find(|p| p.is_file())
}

/// `0.9.10-x86_64…` sorts after `0.9.3-…`.
fn version_key(dir: &Path) -> Vec<u64> {
    let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    name.split('-').next().unwrap_or_default().split('.').map(|n| n.parse().unwrap_or(0)).collect()
}

/// The home folder of the account herdr and Claude Code run under, as this process reads it. In
/// WSL that is the Windows profile (asked of `cmd.exe` once).
pub fn agent_home(host: &Host) -> Option<PathBuf> {
    match host {
        Host::Native => dirs::home_dir(),
        Host::Wsl { .. } => {
            static HOME: OnceLock<Option<PathBuf>> = OnceLock::new();
            HOME.get_or_init(|| windows_profile().map(|p| host.from_agent(&p))).clone()
        }
    }
}

fn windows_profile() -> Option<String> {
    ["cmd.exe", "/mnt/c/Windows/System32/cmd.exe"].iter().find_map(|cmd| {
        let out = Process::new(cmd).args(["/c", "echo %USERPROFILE%"]).cwd("/mnt/c").timeout(Duration::from_secs(10)).run().ok()?;
        let line = out.stdout_text().lines().map(str::trim).find(|l| !l.is_empty() && !l.contains('%'))?.to_string();
        out.success.then_some(line)
    })
}

/// Claude Code's own folder on the agent's side (its `projects/` holds the transcripts).
pub fn claude_home(host: &Host) -> Option<PathBuf> {
    match host {
        Host::Native => super::adapters::claude_config_dir(),
        Host::Wsl { .. } => agent_home(host).map(|h| h.join(".claude")),
    }
}

/// Whether herdr answers here, for detection: its program and the server's reply.
pub fn reachable() -> Option<String> {
    let herdr = program(&BackendOptions::default(), &Host::current()).ok()?;
    client::status(&client::HerdrCli { program: herdr.clone() }).ok().map(|_| herdr)
}

/// A random version 4 UUID, for `--session-id`: the transcript is then `<id>.jsonl`.
pub fn new_session_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let mut b = [0u8; 16];
    for (i, chunk) in b.chunks_mut(8).enumerate() {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(nanos);
        h.write_u32(std::process::id());
        h.write_usize(i);
        chunk.copy_from_slice(&h.finish().to_le_bytes());
    }
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_ids_are_distinct_v4_uuids() {
        let (a, b) = (new_session_id(), new_session_id());
        assert_ne!(a, b);
        assert_eq!(a.len(), 36);
        assert_eq!(a.chars().nth(14), Some('4'), "{a}");
        assert!(a.split('-').map(str::len).eq([8, 4, 4, 4, 12]), "{a}");
    }

    #[test]
    fn the_newest_install_is_found_by_version_not_by_name() {
        let home = tempfile::tempdir().unwrap();
        let releases = home.path().join(".herdr/packages/standalone/releases");
        for v in ["0.9.3-x86_64-pc-windows-msvc", "0.9.10-x86_64-pc-windows-msvc", "0.9.9-x86_64-pc-windows-msvc"] {
            std::fs::create_dir_all(releases.join(v)).unwrap();
            std::fs::write(releases.join(v).join("herdr.exe"), b"").unwrap();
        }
        let found = newest_install(home.path()).unwrap();
        assert!(found.display().to_string().contains("0.9.10"), "{}", found.display());
    }

    #[test]
    fn clean_room_building_is_refused_and_nothing_else_is() {
        let a = ClaudeCodeHerdr;
        for access in [Access::Analyse, Access::CleanRoomAnalyse, Access::Build, Access::Question, Access::CleanRoomQuestion] {
            assert!(a.validate_access(access).is_ok(), "{access:?}");
        }
        assert!(a.validate_access(Access::CleanRoomBuild).is_err());
    }
}
