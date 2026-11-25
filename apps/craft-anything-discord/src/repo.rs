//! A GitHub repo per port (optional, `GITHUB_OWNER`): `gh repo create <owner>/port-<slug>
//! --private`, then the project folder is committed and pushed over SSH after each check and when
//! the port finishes. The original app never goes in: it is kept outside the project folder, and
//! the `.gitignore` also keeps out package files, Craft's own records (which hold captures of the
//! original) and the originals an extractor takes out. `gh` and `git` run through [`Shell`], so the
//! steps are tested with a fake.

use craft_anything_engine::workspace::OWN_NAMES;
use craft_anything_platform::process::Process;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

const COMMAND_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// Staged files larger than this are left out: GitHub refuses files over 100 MB and warns from 50.
pub const MAX_FILE_BYTES: u64 = 50 * 1024 * 1024;
const ERROR_CHARS: usize = 300;
const BRANCH: &str = "main";
/// The `.gitignore` lines between these markers are the bot's; the rest of the file is left alone.
const BLOCK_START: &str = "# >>> craft porter bot";
const BLOCK_END: &str = "# <<< craft porter bot";

/// Never pushed, wherever they are in the project.
const IGNORED_ANYWHERE: &[&str] = &[
    "# The original app: package files",
    "*.apk",
    "*.xapk",
    "*.apks",
    "*.aab",
    "*.obb",
    "*.dex",
    "*.so",
    "# Build output",
    "Library/",
    "library/",
    "Temp/",
    "temp/",
    "Obj/",
    "obj/",
    "Logs/",
    "UserSettings/",
    "MemoryCaptures/",
    "Build/",
    "Builds/",
    "build/",
    "local/",
    "dist/",
    "out/",
    "target/",
    "node_modules/",
    ".gradle/",
    "*.ipa",
    "# Large binaries",
    "*.zip",
    "*.7z",
    "*.rar",
    "*.tar",
    "*.gz",
    "*.mp4",
    "*.mov",
    "*.mkv",
    "*.avi",
    "*.unitypackage",
];

/// The repo the port pushes to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoInfo {
    /// `<owner>/port-<slug>`.
    pub full_name: String,
    /// The page to link: `https://github.com/<owner>/port-<slug>`.
    pub url: String,
    /// The push remote: `git@github.com:<owner>/port-<slug>.git`.
    pub ssh: String,
}

impl RepoInfo {
    pub fn for_port(owner: &str, slug: &str) -> RepoInfo {
        let full_name = format!("{owner}/port-{slug}");
        RepoInfo { url: format!("https://github.com/{full_name}"), ssh: format!("git@github.com:{full_name}.git"), full_name }
    }
}

/// How a command ended.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ran {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Runs `gh` and `git`.
pub trait Shell: Send + Sync {
    fn run(&self, program: &str, args: &[&str], cwd: Option<&Path>) -> Result<Ran, String>;
}

/// The real programs, found on `PATH`.
pub struct SystemShell;

impl Shell for SystemShell {
    fn run(&self, program: &str, args: &[&str], cwd: Option<&Path>) -> Result<Ran, String> {
        let mut process = Process::new(program).args(args).label(program.to_owned()).timeout(COMMAND_TIMEOUT).env("GIT_TERMINAL_PROMPT", "0");
        if let Some(dir) = cwd {
            process = process.cwd(dir);
        }
        let out = process.run().map_err(|e| e.message)?;
        Ok(Ran { success: out.success, stdout: out.stdout_text(), stderr: out.stderr_text() })
    }
}

fn checked(shell: &dyn Shell, program: &str, args: &[&str], cwd: Option<&Path>) -> Result<Ran, String> {
    let ran = shell.run(program, args, cwd)?;
    if ran.success {
        return Ok(ran);
    }
    let said: String = ran.stderr.trim().chars().take(ERROR_CHARS).collect();
    Err(format!("`{program} {}` failed: {said}", args.first().copied().unwrap_or_default()))
}

/// The bot's `.gitignore` block: package files, build output and large binaries anywhere, and
/// Craft's own records and the extracted originals at the project root.
pub fn gitignore_block() -> String {
    let mut lines = vec![BLOCK_START.to_owned(), "# Kept out by the Craft porter bot: the original app never goes to GitHub.".to_owned()];
    lines.extend(IGNORED_ANYWHERE.iter().map(|l| (*l).to_owned()));
    lines.push("# Craft's own records (captures of the original, notes, transcripts)".to_owned());
    lines.extend(OWN_NAMES.iter().map(|name| format!("/{name}")));
    lines.push("# Files an extractor took out of the original".to_owned());
    lines.push("/app/Original/".to_owned());
    lines.push(BLOCK_END.to_owned());
    lines.join("\n") + "\n"
}

/// The `.gitignore` with the bot's block put in or brought up to date; other lines are kept.
pub fn merged_gitignore(existing: &str) -> String {
    let block = gitignore_block();
    if let (Some(start), Some(end)) = (existing.find(BLOCK_START), existing.find(BLOCK_END))
        && start < end
    {
        let after = existing[end + BLOCK_END.len()..].trim_start_matches(['\r', '\n']);
        return format!("{}{block}{after}", &existing[..start]);
    }
    if existing.trim().is_empty() {
        return block;
    }
    let sep = if existing.ends_with('\n') { "\n" } else { "\n\n" };
    format!("{existing}{sep}{block}")
}

/// Creates the private repo, or uses it when it is already there (a port started again).
pub fn create_repo(shell: &dyn Shell, owner: &str, slug: &str) -> Result<RepoInfo, String> {
    let repo = RepoInfo::for_port(owner, slug);
    let description = format!("Port made by the Craft porter bot (port-{slug})");
    let created = shell.run("gh", &["repo", "create", &repo.full_name, "--private", "--description", &description], None)?;
    if created.success {
        return Ok(repo);
    }
    if shell.run("gh", &["repo", "view", &repo.full_name, "--json", "name"], None)?.success {
        log::info!("{} already exists; the port pushes to it", repo.full_name);
        return Ok(repo);
    }
    let said: String = created.stderr.trim().chars().take(ERROR_CHARS).collect();
    Err(format!("`gh repo create {}` failed: {said}", repo.full_name))
}

/// Makes `project` a git repository pushing to `repo` (when it is not one yet) and writes the
/// `.gitignore`. A remote the folder already has is kept.
pub fn prepare(shell: &dyn Shell, project: &Path, repo: &RepoInfo) -> Result<(), String> {
    if !project.is_dir() {
        return Err(format!("the project folder {} is not there yet", project.display()));
    }
    if !project.join(".git").exists() {
        checked(shell, "git", &["init", "-q", "-b", BRANCH], Some(project))?;
    }
    let path = project.join(".gitignore");
    let existing = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("couldn't read {}: {e}", path.display())),
    };
    let merged = merged_gitignore(&existing);
    if merged != existing {
        std::fs::write(&path, merged).map_err(|e| format!("couldn't write {}: {e}", path.display()))?;
    }
    if !shell.run("git", &["remote", "get-url", "origin"], Some(project))?.success {
        checked(shell, "git", &["remote", "add", "origin", &repo.ssh], Some(project))?;
    }
    Ok(())
}

/// What a commit-and-push did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pushed {
    pub committed: bool,
    /// Staged files over [`MAX_FILE_BYTES`] that were left out.
    pub left_out: Vec<String>,
}

/// Commits everything not ignored (files too large for GitHub are left out) and pushes. Nothing
/// new is not an error; with no commit at all yet there is nothing to push.
pub fn commit_and_push(shell: &dyn Shell, project: &Path, repo: &RepoInfo, message: &str) -> Result<Pushed, String> {
    prepare(shell, project, repo)?;
    checked(shell, "git", &["add", "-A"], Some(project))?;
    let staged = checked(shell, "git", &["diff", "--cached", "--name-only", "-z"], Some(project))?;
    let mut left_out = Vec::new();
    for file in staged.stdout.split('\0').filter(|f| !f.is_empty()) {
        if std::fs::metadata(project.join(file)).is_ok_and(|m| m.len() > MAX_FILE_BYTES) {
            checked(shell, "git", &["rm", "--cached", "-q", "--", file], Some(project))?;
            left_out.push(file.to_owned());
        }
    }
    let changed = !shell.run("git", &["diff", "--cached", "--quiet"], Some(project))?.success;
    if changed {
        checked(shell, "git", &["commit", "-q", "-m", message], Some(project))?;
    }
    if shell.run("git", &["rev-parse", "--verify", "-q", "HEAD"], Some(project))?.success {
        checked(shell, "git", &["push", "-q", "-u", "origin", "HEAD"], Some(project))?;
    }
    Ok(Pushed { committed: changed, left_out })
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft_anything_platform::sync::Mutex;

    /// Answers like git and gh would for a folder whose `git diff --cached --quiet` says `changed`.
    struct FakeShell {
        calls: Mutex<Vec<String>>,
        changed: bool,
        staged: String,
        repo_exists: bool,
    }

    impl FakeShell {
        fn new(changed: bool) -> FakeShell {
            FakeShell { calls: Mutex::new(Vec::new()), changed, staged: String::new(), repo_exists: false }
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Shell for FakeShell {
        fn run(&self, program: &str, args: &[&str], cwd: Option<&Path>) -> Result<Ran, String> {
            let line = format!("{program} {}", args.join(" "));
            self.calls.lock().unwrap().push(line.clone());
            if let (Some(dir), true) = (cwd, line == "git init -q -b main") {
                std::fs::create_dir_all(dir.join(".git")).map_err(|e| e.to_string())?;
            }
            let success = match line.as_str() {
                l if l.starts_with("gh repo create") => !self.repo_exists,
                l if l.starts_with("gh repo view") => self.repo_exists,
                "git remote get-url origin" => false,
                "git diff --cached --quiet" => !self.changed,
                "git rev-parse --verify -q HEAD" => self.changed,
                _ => true,
            };
            let stdout = if line.starts_with("git diff --cached --name-only") { self.staged.clone() } else { String::new() };
            Ok(Ran { success, stdout, stderr: if success { String::new() } else { "exists".into() } })
        }
    }

    #[test]
    fn the_gitignore_keeps_the_original_app_and_crafts_records_out() {
        let block = gitignore_block();
        let lines: Vec<&str> = block.lines().collect();
        for kept_out in ["*.apk", "*.xapk", "*.apks", "*.aab", "/capture", "/reference", "/inbox", "/app/Original/", "Library/", "*.zip"] {
            assert!(lines.contains(&kept_out), "{kept_out} is not ignored:\n{block}");
        }
        assert!(OWN_NAMES.iter().all(|name| lines.contains(&format!("/{name}").as_str())), "every Craft record is ignored");
    }

    #[test]
    fn the_gitignore_block_is_added_once_and_other_lines_are_kept() {
        let mine = "*.log\n";
        let merged = merged_gitignore(mine);
        assert!(merged.starts_with("*.log\n") && merged.contains("*.apk"));
        assert_eq!(merged_gitignore(&merged), merged, "a second run changes nothing");
        let stale = format!("{mine}{BLOCK_START}\nold\n{BLOCK_END}\nmore.txt\n");
        let refreshed = merged_gitignore(&stale);
        assert!(!refreshed.contains("\nold\n") && refreshed.contains("more.txt") && refreshed.contains("*.aab"), "{refreshed}");
    }

    #[test]
    fn a_new_project_is_initialised_ignored_and_pushed_over_ssh() {
        let dir = tempfile::tempdir().unwrap();
        let shell = FakeShell::new(true);
        let repo = RepoInfo::for_port("studio", "snake");
        let pushed = commit_and_push(&shell, dir.path(), &repo, "Check round 1").unwrap();
        assert!(pushed.committed);
        let calls = shell.calls();
        assert!(calls.contains(&"git init -q -b main".to_owned()));
        assert!(calls.contains(&"git remote add origin git@github.com:studio/port-snake.git".to_owned()), "{calls:?}");
        assert!(calls.contains(&"git commit -q -m Check round 1".to_owned()));
        assert_eq!(calls.last().map(String::as_str), Some("git push -q -u origin HEAD"));
        let ignore = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert!(ignore.contains("*.apk"), "the ignore file is in place before anything is added");
    }

    #[test]
    fn nothing_new_makes_no_commit() {
        let dir = tempfile::tempdir().unwrap();
        let shell = FakeShell::new(false);
        let pushed = commit_and_push(&shell, dir.path(), &RepoInfo::for_port("studio", "snake"), "Finished").unwrap();
        assert!(!pushed.committed);
        assert!(!shell.calls().iter().any(|c| c.starts_with("git commit") || c.starts_with("git push")), "{:?}", shell.calls());
    }

    #[test]
    fn a_file_too_large_for_github_is_left_out() {
        let dir = tempfile::tempdir().unwrap();
        let big = std::fs::File::create(dir.path().join("huge.bin")).unwrap();
        big.set_len(MAX_FILE_BYTES + 1).unwrap();
        std::fs::write(dir.path().join("small.txt"), "hi").unwrap();
        let shell = FakeShell { staged: "huge.bin\0small.txt\0".into(), ..FakeShell::new(true) };
        let pushed = commit_and_push(&shell, dir.path(), &RepoInfo::for_port("studio", "snake"), "Check").unwrap();
        assert_eq!(pushed.left_out, ["huge.bin"]);
        assert!(shell.calls().contains(&"git rm --cached -q -- huge.bin".to_owned()));
    }

    #[test]
    fn an_existing_repo_is_used_and_a_missing_owner_is_an_error() {
        let existing = FakeShell { repo_exists: true, ..FakeShell::new(true) };
        assert_eq!(create_repo(&existing, "studio", "snake").map(|r| r.full_name), Ok("studio/port-snake".to_owned()));
        let created = FakeShell::new(true);
        assert!(create_repo(&created, "studio", "snake").is_ok());
        assert!(created.calls()[0].contains("--private"));
        struct Refusing;
        impl Shell for Refusing {
            fn run(&self, _: &str, _: &[&str], _: Option<&Path>) -> Result<Ran, String> {
                Ok(Ran { success: false, stdout: String::new(), stderr: "no such owner".into() })
            }
        }
        assert!(create_repo(&Refusing, "studio", "snake").is_err_and(|e| e.contains("no such owner")));
    }
}
