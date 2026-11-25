//! `rebuild --events ndjson` on a real run. A stand-in `claude` (the builder) answers one turn and
//! exits, so the run goes through its real steps without a network or a signed-in agent. The
//! stream on stdout must be one JSON object per line, start with `project`, include a `step` and
//! end with `finished`. Without the flag, stdout keeps the human text.
#![cfg(unix)]

use craft_anything_platform::process::{Output, Process};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Duration;

fn cli(args: &[&str], config: &Path, home: &Path, bin: &Path) -> Output {
    let path = format!("{}:/usr/bin:/bin", bin.display());
    Process::new(env!("CARGO_BIN_EXE_craft-anything-cli"))
        .args(args)
        .env_clear()
        .env("CRAFT_ANYTHING_CONFIG_DIR", config)
        .env("HOME", home)
        .env("PATH", path)
        .timeout(Duration::from_secs(180))
        .run()
        .unwrap()
}

/// A builder that answers `--version` and `--help` like Claude Code, then one finished turn. The
/// stack's toolchain (`cargo`) is a stand-in too, so the run doesn't stop on a computer without it.
fn fake_builder(bin: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let toolchain = bin.join("cargo");
    std::fs::write(&toolchain, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&toolchain, std::fs::Permissions::from_mode(0o700)).unwrap();
    let program = bin.join("claude");
    std::fs::write(
        &program,
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'fake-claude 1.0'; exit 0; fi\nif [ \"$1\" = \"--help\" ]; then echo '--output-format stream-json'; exit 0; fi\ncat >/dev/null\nprintf '%s\\n' '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"fake\"}'\nprintf '%s\\n' '{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"Built it.\\nAll done.\"}]}}'\nprintf '%s\\n' '{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"num_turns\":1}'\n",
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
    program
}

/// Runs `rebuild - "make a small page"` into `projects` with no rounds and no questions, plus any
/// extra flags.
fn run_rebuild(projects: &Path, extra: &[&str]) -> (Output, tempfile::TempDir, tempfile::TempDir, tempfile::TempDir) {
    let config = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let bin = tempfile::tempdir().unwrap();
    fake_builder(bin.path());
    let mut args = vec!["rebuild", "-", "make a small page", "--rounds", "0", "--unattended", "--backend", "Claude Code", "--projects-dir"];
    let projects = projects.display().to_string();
    args.push(&projects);
    args.extend_from_slice(extra);
    let out = cli(&args, config.path(), home.path(), bin.path());
    (out, config, home, bin)
}

fn json_lines(stdout: &str) -> Vec<Value> {
    stdout.lines().map(|l| serde_json::from_str::<Value>(l).unwrap_or_else(|e| panic!("stdout line is not JSON ({e}): {l}"))).collect()
}

#[test]
fn ndjson_stream_is_one_typed_object_per_line_from_project_to_finished() {
    let projects = tempfile::tempdir().unwrap();
    let (out, _config, _home, _bin) = run_rebuild(projects.path(), &["--events", "ndjson"]);
    let stdout = out.stdout_text();
    let log = out.stderr_text();

    let lines = json_lines(&stdout);
    assert!(lines.len() >= 3, "a run has a project, steps and a finish:\nstdout:\n{stdout}\nstderr:\n{log}");
    assert!(lines.iter().all(|v| v["type"].is_string()), "every line has a type:\n{stdout}");

    let folder = std::fs::read_dir(projects.path()).unwrap().filter_map(Result::ok).map(|e| e.path()).find(|p| p.is_dir()).expect("a project folder");
    assert_eq!(lines[0]["type"], "project", "the first line names the project:\n{stdout}");
    assert_eq!(Path::new(lines[0]["dir"].as_str().unwrap()), folder.as_path());
    assert!(lines.iter().any(|v| v["type"] == "step"), "at least one step:\n{stdout}");
    let last = lines.last().unwrap();
    assert_eq!(last["type"], "finished", "the stream ends with finished:\n{stdout}\nstderr:\n{log}");
}

#[test]
fn ndjson_run_keeps_agent_text_out_of_the_human_lines_and_moves_human_text_to_stderr() {
    let projects = tempfile::tempdir().unwrap();
    let (out, _config, _home, _bin) = run_rebuild(projects.path(), &["--events", "ndjson"]);
    let stdout = out.stdout_text();
    let lines = json_lines(&stdout);
    assert!(!stdout.contains("Project: "), "human text must not reach stdout:\n{stdout}");
    let texts: Vec<&str> = lines.iter().filter(|v| v["type"] == "agent_text").filter_map(|v| v["text"].as_str()).collect();
    assert!(texts.contains(&"Built it."), "agent text is one line per line of the reply:\n{stdout}");
    assert!(texts.contains(&"All done."), "the second line arrives as its own agent_text:\n{stdout}");
}

#[test]
fn without_the_flag_stdout_keeps_the_human_project_line() {
    let projects = tempfile::tempdir().unwrap();
    let (out, _config, _home, _bin) = run_rebuild(projects.path(), &[]);
    let stdout = out.stdout_text();
    assert!(stdout.starts_with("Project: "), "human mode is unchanged:\n{stdout}");
    assert!(stdout.lines().any(|l| l.starts_with("Done.") || l.starts_with("Stopped:")), "the human run ends with its result:\n{stdout}");
}
