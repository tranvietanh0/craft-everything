use super::*;
use serde_json::json;
use std::fs;

/// `PORTS_ROOT` with one port the bot made (`<root>/app/bot.json`).
fn ports() -> (tempfile::TempDir, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let port = root.path().join("app");
    fs::create_dir(&port).unwrap();
    fs::write(PortState::path(&port), b"{}").unwrap();
    (root, port)
}

fn config_with(value: &Value) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".claude.json");
    fs::write(&path, format!("{}\n", serde_json::to_string_pretty(value).unwrap())).unwrap();
    (dir, path)
}

fn sample() -> Value {
    json!({
        "numStartups": 42,
        "oauthAccount": {"emailAddress": "someone@example.com", "nested": [1, 2.5, null, "x"]},
        "projects": {
            "C:/Projects/other": {"allowedTools": ["Bash"], "hasTrustDialogAccepted": false, "lastCost": 0.25},
            "C:/Projects/ports/app/project": {"allowedTools": [], "lastSessionId": "abc", "hasTrustDialogAccepted": false}
        },
        "theme": "dark"
    })
}

#[test]
fn a_port_folder_inside_ports_root_gives_its_project_folder() {
    let (root, port) = ports();
    let project = port_project(root.path(), &port).expect("inside PORTS_ROOT");
    assert_eq!(project, fs::canonicalize(&port).unwrap().join("project"));
    assert!(project.is_dir(), "the project folder is made when missing");
}

#[test]
fn a_folder_outside_ports_root_is_refused() {
    let (root, _) = ports();
    let elsewhere = tempfile::tempdir().unwrap();
    fs::write(PortState::path(elsewhere.path()), b"{}").unwrap();
    assert!(matches!(port_project(root.path(), elsewhere.path()), Err(TrustError::Outside { .. })));
    assert!(!elsewhere.path().join("project").exists(), "nothing is made outside");
}

#[test]
fn ports_root_itself_and_deeper_folders_are_refused() {
    let (root, port) = ports();
    fs::write(PortState::path(root.path()), b"{}").unwrap();
    assert!(matches!(port_project(root.path(), root.path()), Err(TrustError::Outside { .. })), "PORTS_ROOT itself");
    let deeper = port.join("input");
    fs::create_dir(&deeper).unwrap();
    fs::write(PortState::path(&deeper), b"{}").unwrap();
    assert!(matches!(port_project(root.path(), &deeper), Err(TrustError::Outside { .. })), "only PORTS_ROOT/<slug>");
}

#[test]
fn a_path_with_dot_dot_is_refused_before_it_is_resolved() {
    let (root, _) = ports();
    let sneaky = root.path().join("app").join("..").join("app");
    assert!(matches!(port_project(root.path(), &sneaky), Err(TrustError::ParentDir(_))));
    let escaping = root.path().join("..").join("somewhere");
    assert!(matches!(port_project(root.path(), &escaping), Err(TrustError::ParentDir(_))));
}

#[test]
fn a_port_without_bot_json_is_not_ours() {
    let root = tempfile::tempdir().unwrap();
    let port = root.path().join("hand-made");
    fs::create_dir(&port).unwrap();
    assert!(matches!(port_project(root.path(), &port), Err(TrustError::NotOurPort(_))));
}

#[cfg(unix)]
#[test]
fn a_link_out_of_ports_root_is_refused() {
    let (root, port) = ports();
    let outside = tempfile::tempdir().unwrap();
    // The project folder is a link to a folder elsewhere.
    std::os::unix::fs::symlink(outside.path(), port.join("project")).unwrap();
    assert!(matches!(port_project(root.path(), &port), Err(TrustError::Outside { .. })));
    // The port folder itself is a link to a folder elsewhere that has a bot.json.
    fs::write(PortState::path(outside.path()), b"{}").unwrap();
    let linked = root.path().join("linked");
    std::os::unix::fs::symlink(outside.path(), &linked).unwrap();
    assert!(matches!(port_project(root.path(), &linked), Err(TrustError::Outside { .. })));
}

#[test]
fn only_that_folders_trust_changes_and_every_other_value_stays() {
    let (_dir, path) = config_with(&sample());
    assert_eq!(grant(&path, "C:/Projects/ports/app/project"), Ok(Granted::Added));
    let after: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let mut expected = sample();
    expected["projects"]["C:/Projects/ports/app/project"]["hasTrustDialogAccepted"] = json!(true);
    assert_eq!(after, expected);
    assert!(fs::read(&path).unwrap().ends_with(b"}\n"), "the final newline is kept");
}

#[test]
fn a_folder_with_no_entry_gets_one_with_only_the_trust_key() {
    let (_dir, path) = config_with(&json!({"theme": "dark"}));
    assert_eq!(grant(&path, "C:/Projects/ports/new/project"), Ok(Granted::Added));
    let after: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(after, json!({"theme": "dark", "projects": {"C:/Projects/ports/new/project": {"hasTrustDialogAccepted": true}}}));
}

#[test]
fn an_already_trusted_folder_leaves_the_file_unwritten() {
    let mut value = sample();
    value["projects"]["C:/Projects/ports/app/project"]["hasTrustDialogAccepted"] = json!(true);
    let (_dir, path) = config_with(&value);
    let before = fs::read(&path).unwrap();
    assert_eq!(grant(&path, "C:/Projects/ports/app/project"), Ok(Granted::AlreadyTrusted));
    assert_eq!(fs::read(&path).unwrap(), before);
}

#[test]
fn an_unparseable_or_oddly_shaped_config_is_refused_and_left_alone() {
    for (text, unparseable) in [("{\"projects\": {", true), ("[1, 2]", false), ("{\"projects\": 3}", false)] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".claude.json");
        fs::write(&path, text).unwrap();
        let err = grant(&path, "C:/x").unwrap_err();
        let expected = if unparseable { matches!(err, TrustError::ConfigUnparseable { .. }) } else { matches!(err, TrustError::ConfigShape { .. }) };
        assert!(expected, "{text}: {err:?}");
        assert_eq!(fs::read_to_string(&path).unwrap(), text, "{text}");
    }
}

#[test]
fn a_missing_config_is_never_created() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".claude.json");
    assert!(matches!(grant(&path, "C:/x"), Err(TrustError::ConfigMissing(_))));
    assert!(!path.exists());
}

#[test]
fn the_write_leaves_no_temporary_file_behind() {
    let (dir, path) = config_with(&sample());
    assert_eq!(grant(&path, "C:/Projects/ports/app/project"), Ok(Granted::Added));
    let names: Vec<String> = fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(names, vec![".claude.json".to_owned()]);
}

#[test]
fn with_the_flag_off_or_another_backend_nothing_is_touched() {
    let (root, port) = ports();
    let (_dir, path) = config_with(&sample());
    let before = fs::read(&path).unwrap();
    for (enabled, backend) in [(false, Some(herdr::DISPLAY)), (true, Some("Claude Code")), (true, None)] {
        let done = maybe_pretrust(enabled, backend, root.path(), &port, Some(&path), &Host::Native);
        assert_eq!(done, Ok(None), "{enabled} {backend:?}");
    }
    assert_eq!(fs::read(&path).unwrap(), before);
    assert!(!port.join("project").exists(), "not even the project folder is made");
}

#[test]
fn with_the_flag_on_the_ports_project_folder_is_trusted() {
    let (root, port) = ports();
    let (_dir, path) = config_with(&sample());
    let done = maybe_pretrust(true, Some(" claude code (herdr) "), root.path(), &port, Some(&path), &Host::Native).expect("trusted");
    let (key, granted) = done.expect("the flag is on");
    assert_eq!(granted, Granted::Added);
    assert_eq!(key, trust_key(&Host::Native, &fs::canonicalize(&port).unwrap().join("project")));
    let after: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(after["projects"][key.as_str()][TRUST_KEY], json!(true));
}

#[test]
fn the_key_is_the_folder_as_windows_claude_code_writes_it() {
    let wsl = Host::Wsl { distro: "Ubuntu-24.04".to_owned() };
    assert_eq!(trust_key(&wsl, Path::new("/mnt/c/Projects/ports/app/project")), "C:/Projects/ports/app/project");
    assert_eq!(trust_key(&wsl, Path::new("/home/me/ports/app/project")), "//wsl.localhost/Ubuntu-24.04/home/me/ports/app/project");
    assert_eq!(config_path(Some(Path::new("C:\\Users\\me\\.claude.json")), &wsl), Ok(PathBuf::from("/mnt/c/Users/me/.claude.json")));
}

#[test]
fn only_the_herdr_backend_counts() {
    assert!(uses_herdr(Some(herdr::DISPLAY)) && uses_herdr(Some(herdr::ID)));
    assert!(!uses_herdr(Some("Claude Code")) && !uses_herdr(None) && !uses_herdr(Some("")));
}
