//! The rebuild child: what `/port` asks for, the exact `CRAFT_CLI rebuild …` command line, how
//! its end is judged, and starting it with each stdout line handed to the caller.

use craft_anything_evidence::ToolError;
use craft_anything_platform::process::{Process, RunningProcess};
use std::ffi::OsString;
use std::path::Path;
use std::time::Duration;

/// Longest a port may run before it is stopped.
pub const RUN_LIMIT: Duration = Duration::from_secs(24 * 60 * 60);
/// Most characters of the child's stderr shown when it fails without finishing.
pub const STDERR_TAIL_CHARS: usize = 1_500;
/// stdout is streamed line by line; only this much is kept in memory as well.
const STDOUT_KEEP: usize = 256 * 1024;

/// What the port is rebuilt as (the `/port` `target` choice).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Target {
    #[default]
    Unity,
    Cocos,
    App,
}

impl Target {
    /// The choice values `/port` offers, in order.
    pub const CHOICES: [Target; 3] = [Target::Unity, Target::Cocos, Target::App];

    pub fn from_choice(value: &str) -> Option<Target> {
        Target::CHOICES.into_iter().find(|t| t.choice() == value)
    }

    pub fn choice(self) -> &'static str {
        match self {
            Target::Unity => "unity",
            Target::Cocos => "cocos",
            Target::App => "app",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Target::Unity => "Unity",
            Target::Cocos => "Cocos Creator",
            Target::App => "Mobile app",
        }
    }

    /// The `--build-with` value. Only Unity is a stack the CLI knows; Cocos Creator and a native
    /// app have none, so the request names them and the builder picks the stack.
    pub fn build_with(self) -> Option<&'static str> {
        match self {
            Target::Unity => Some("unity"),
            Target::Cocos | Target::App => None,
        }
    }

    /// The request given to `rebuild`.
    pub fn request(self, app_label: &str) -> String {
        let what = match self {
            Target::Unity => "a Unity project",
            Target::Cocos => "a Cocos Creator project in TypeScript",
            Target::App => "a native mobile app",
        };
        format!("Port the Android app \"{app_label}\" to {what}: rebuild it so it looks, plays and behaves like the original.")
    }
}

/// The arguments after `CRAFT_CLI` for one port (Deep mode, the person's own skills, unattended,
/// events on stdout, a fresh project folder).
pub fn rebuild_args(input: &Path, request: &str, project: &Path, target: Target) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec!["rebuild".into(), input.into(), request.into()];
    for flag in ["--deep", "--user-skills", "--unattended"] {
        args.push(flag.into());
    }
    args.extend(["--events".into(), "ndjson".into(), "--project-folder".into(), project.into()]);
    if let Some(stack) = target.build_with() {
        args.extend(["--build-with".into(), stack.into()]);
    }
    args
}

/// How the child ended, as the process reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildEnd {
    pub success: bool,
    pub code: Option<i32>,
    pub stderr: String,
}

/// The `finished` event, when the child printed one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finished {
    pub ok: bool,
    pub paused: bool,
    pub message: String,
}

/// The verdict posted when a port ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub ok: bool,
    pub paused: bool,
    pub message: String,
    /// Mention the tester role: only a run that finished OK and exited cleanly is ready to test.
    pub mention_testers: bool,
    /// The end of stderr, when the child failed without a clean `finished`.
    pub stderr_tail: Option<String>,
}

/// Judges a run from its `finished` event and its exit. A run is OK only when both say so; a
/// child that crashed or exited without `finished` failed, and its stderr tail is shown.
pub fn verdict(finished: Option<&Finished>, end: &ChildEnd) -> Verdict {
    let tail = || Some(tail_chars(end.stderr.trim(), STDERR_TAIL_CHARS)).filter(|t| !t.is_empty());
    match finished {
        Some(f) if f.ok && end.success => Verdict { ok: true, paused: false, message: f.message.clone(), mention_testers: true, stderr_tail: None },
        Some(f) if !f.ok => Verdict { ok: false, paused: f.paused, message: f.message.clone(), mention_testers: false, stderr_tail: None },
        Some(_) => Verdict { ok: false, paused: false, message: exit_message(end), mention_testers: false, stderr_tail: tail() },
        None => Verdict { ok: false, paused: false, message: format!("The rebuild stopped without finishing ({})", exit_text(end)), mention_testers: false, stderr_tail: tail() },
    }
}

fn exit_message(end: &ChildEnd) -> String {
    format!("The rebuild said it finished, but the program then failed ({})", exit_text(end))
}

fn exit_text(end: &ChildEnd) -> String {
    end.code.map_or_else(|| "no exit code".to_owned(), |c| format!("exit code {c}"))
}

/// The last `max` characters of `text`, cut at a character boundary.
pub fn tail_chars(text: &str, max: usize) -> String {
    let count = text.chars().count();
    text.chars().skip(count.saturating_sub(max)).collect()
}

/// Starts `craft_cli` with `args`; every stdout line goes to `on_line`. stdin is closed (the run
/// is unattended). Drop or kill the handle to stop the whole process group.
pub fn spawn(craft_cli: &Path, args: Vec<OsString>, on_line: impl Fn(&str) + Send + Sync + 'static) -> Result<RunningProcess, ToolError> {
    Process::new(craft_cli)
        .args(args)
        .label("craft-anything-cli rebuild")
        .timeout(RUN_LIMIT)
        .stdout_cap(STDOUT_KEEP)
        .on_stdout_line(on_line)
        .spawn()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn args(target: Target) -> Vec<String> {
        let input = PathBuf::from("/ports/snake/input/snake.apk");
        let project = PathBuf::from("/ports/snake/project");
        rebuild_args(&input, "Port it", &project, target).iter().map(|a| a.to_string_lossy().into_owned()).collect()
    }

    fn after<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
        args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).map(String::as_str)
    }

    #[test]
    fn the_command_line_runs_rebuild_on_the_input_with_events() {
        let a = args(Target::Unity);
        assert_eq!(a.first().map(String::as_str), Some("rebuild"));
        assert_eq!(a.get(1).map(String::as_str), Some("/ports/snake/input/snake.apk"), "the target is the downloaded file");
        assert_eq!(a.get(2).map(String::as_str), Some("Port it"), "the request is one argument");
        for flag in ["--deep", "--user-skills", "--unattended"] {
            assert!(a.iter().any(|x| x == flag), "{flag} missing");
        }
        assert_eq!(after(&a, "--events"), Some("ndjson"));
        assert_eq!(after(&a, "--project-folder"), Some("/ports/snake/project"));
        assert_eq!(after(&a, "--build-with"), Some("unity"));
    }

    #[test]
    fn targets_without_a_stack_leave_the_choice_to_the_request() {
        for target in [Target::Cocos, Target::App] {
            let a = args(target);
            assert!(!a.iter().any(|x| x == "--build-with"), "{target:?} has no stack");
        }
        assert!(Target::Cocos.request("Snake").contains("Cocos Creator"));
        assert!(Target::App.request("Snake").contains("native mobile app"));
    }

    #[test]
    fn choices_round_trip() {
        for t in Target::CHOICES {
            assert_eq!(Target::from_choice(t.choice()), Some(t));
        }
        assert_eq!(Target::from_choice("unreal"), None);
    }

    fn end(success: bool, code: i32, stderr: &str) -> ChildEnd {
        ChildEnd { success, code: Some(code), stderr: stderr.to_owned() }
    }

    fn finished(ok: bool, paused: bool) -> Finished {
        Finished { ok, paused, message: "done".to_owned() }
    }

    #[test]
    fn testers_are_mentioned_only_for_a_clean_ok_finish() {
        assert!(verdict(Some(&finished(true, false)), &end(true, 0, "")).mention_testers);
        assert!(!verdict(Some(&finished(false, false)), &end(false, 1, "")).mention_testers);
        assert!(!verdict(Some(&finished(false, true)), &end(false, 1, "")).mention_testers, "a paused run is not ready to test");
        assert!(!verdict(Some(&finished(true, false)), &end(false, 101, "panic")).mention_testers, "a crash after finished is a failure");
        assert!(!verdict(None, &end(true, 0, "")).mention_testers, "no finished event is not a pass");
    }

    #[test]
    fn a_crash_shows_the_end_of_stderr() {
        let long = format!("{}THE END", "x".repeat(5_000));
        let v = verdict(None, &end(false, 101, &long));
        let tail = v.stderr_tail.unwrap_or_default();
        assert!(tail.ends_with("THE END"));
        assert!(tail.chars().count() <= STDERR_TAIL_CHARS);
        assert!(!v.ok && v.message.contains("101"));
        assert!(verdict(Some(&finished(false, false)), &end(false, 1, "noise")).stderr_tail.is_none(), "a reported failure needs no stderr");
    }

    #[test]
    fn tails_cut_on_character_boundaries() {
        assert_eq!(tail_chars("ééé", 2), "éé");
        assert_eq!(tail_chars("ab", 10), "ab");
    }
}
