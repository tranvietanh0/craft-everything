//! Claude Code's arguments for a session in a herdr pane: the headless adapter's, made interactive.
//! The tool lists, MCP server and resume stay the same; what only a headless run has goes, and
//! long or quoted values move into files so nothing hard to type reaches the pane's shell.

use super::paths::Host;
use crate::cli_agents::McpCommand;

/// Flags only a headless run has, each with whether a value follows it.
const HEADLESS_ONLY: &[(&str, bool)] = &[
    ("-p", false),
    ("--verbose", false),
    ("--output-format", true),
    // Approvals are the person's, in the pane (docs/claude-code.md, "Claude Code in herdr").
    ("--permission-prompt-tool", true),
    // A question's fork has to be written down: the transcript is how its answer is read.
    ("--no-session-persistence", false),
];

/// The headless arguments made interactive. The notes hook (`--settings` running `builder-notes`)
/// is left out: on Windows it would run in a shell that can't reach a WSL path reliably.
pub fn interactive(headless: Vec<String>) -> Vec<String> {
    let mut out = Vec::with_capacity(headless.len());
    let mut it = headless.into_iter().peekable();
    while let Some(arg) = it.next() {
        if let Some((_, takes_value)) = HEADLESS_ONLY.iter().find(|(flag, _)| *flag == arg) {
            if *takes_value {
                it.next();
            }
            continue;
        }
        if arg == "--settings" && it.peek().is_some_and(|v| v.contains("builder-notes")) {
            it.next();
            continue;
        }
        // An empty value can vanish when typed into a shell; `--tools=` keeps it one word.
        if arg == "--tools" && it.peek().is_some_and(|v| v.is_empty()) {
            it.next();
            out.push("--tools=".into());
            continue;
        }
        out.push(arg);
    }
    out
}

/// Which file a long value goes in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Preamble,
    Mcp,
}

/// `--append-system-prompt <text>` and `--mcp-config <json>` as files: `write` keeps the value and
/// returns the path Claude Code reads it from.
pub fn values_in_files<E>(args: Vec<String>, mut write: impl FnMut(FileKind, &str) -> Result<String, E>) -> Result<Vec<String>, E> {
    let mut out = Vec::with_capacity(args.len());
    let mut it = args.into_iter().peekable();
    while let Some(arg) = it.next() {
        let kind = match arg.as_str() {
            "--append-system-prompt" => Some((FileKind::Preamble, "--append-system-prompt-file")),
            "--mcp-config" => Some((FileKind::Mcp, "--mcp-config")),
            _ => None,
        };
        match kind.zip(it.peek().cloned()) {
            Some(((kind, flag), value)) => {
                it.next();
                let path = write(kind, &value)?;
                out.extend([flag.to_string(), path]);
            }
            None => out.push(arg),
        }
    }
    Ok(out)
}

/// Our tool server as the agent's side starts it: from Windows, through `wsl.exe` into the distro
/// Craft Anything runs in (`--exec`, so no shell reads the paths again).
pub fn mcp_for_agent(host: &Host, mcp: &McpCommand) -> McpCommand {
    match host {
        Host::Native => mcp.clone(),
        Host::Wsl { distro } => {
            let mut args = vec!["-d".to_string(), distro.clone(), "--exec".into(), mcp.command.clone()];
            args.extend(mcp.args.iter().cloned());
            McpCommand { command: "wsl.exe".into(), args, notes: mcp.notes.clone() }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| w.to_string()).collect()
    }

    #[test]
    fn headless_only_flags_go_and_restrictions_stay() {
        let headless = v(&[
            "-p",
            "--output-format",
            "stream-json",
            "--verbose",
            "--append-system-prompt",
            "P",
            "--tools",
            "Read,Write",
            "--permission-prompt-tool",
            "mcp__x__approve",
            "--disallowedTools",
            "AskUserQuestion",
            "--settings",
            "{\"hooks\":\"builder-notes\"}",
            "--strict-mcp-config",
            "--mcp-config",
            "{}",
            "--allowedTools",
            "mcp__craft-anything__*",
            "Read",
        ]);
        let out = interactive(headless);
        assert_eq!(
            out,
            v(&[
                "--append-system-prompt",
                "P",
                "--tools",
                "Read,Write",
                "--disallowedTools",
                "AskUserQuestion",
                "--strict-mcp-config",
                "--mcp-config",
                "{}",
                "--allowedTools",
                "mcp__craft-anything__*",
                "Read"
            ])
        );
        assert_eq!(interactive(v(&["--settings", "{\"a\":1}"])), v(&["--settings", "{\"a\":1}"]), "the person's own settings are kept");
        assert_eq!(interactive(v(&["--tools", "", "--strict-mcp-config"])), v(&["--tools=", "--strict-mcp-config"]), "no tools stays no tools");
    }

    #[test]
    fn long_values_are_handed_over_as_files() {
        let mut kept = Vec::new();
        let out = values_in_files::<()>(
            v(&["--model", "m", "--strict-mcp-config", "--append-system-prompt", "long\n\"text\"", "--mcp-config", "{\"a\":1}"]),
            |kind, value| {
                kept.push((kind, value.to_string()));
                Ok(format!("F{}", kept.len()))
            },
        )
        .unwrap();
        assert_eq!(out, v(&["--model", "m", "--strict-mcp-config", "--append-system-prompt-file", "F1", "--mcp-config", "F2"]));
        assert_eq!(kept, vec![(FileKind::Preamble, "long\n\"text\"".to_string()), (FileKind::Mcp, "{\"a\":1}".to_string())]);
    }

    #[test]
    fn from_windows_the_tool_server_starts_inside_the_distro() {
        let mcp = McpCommand { command: "/home/me/craft-anything-cli".into(), args: v(&["mcp", "--project", "/home/me/p"]), notes: None };
        let win = mcp_for_agent(&Host::Wsl { distro: "Ubuntu".into() }, &mcp);
        assert_eq!(win.command, "wsl.exe");
        assert_eq!(win.args, v(&["-d", "Ubuntu", "--exec", "/home/me/craft-anything-cli", "mcp", "--project", "/home/me/p"]));
        assert_eq!(mcp_for_agent(&Host::Native, &mcp), mcp);
    }
}
