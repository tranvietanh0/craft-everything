use super::*;
use crate::board::Board;
use std::fs;

/// `craft.json` as craft 6 writes it (fields from a real desktop-app project, trimmed).
fn manifest(status: &str, note: &str, checks: &str) -> String {
    format!(
        r#"{{"version":6,"name":"Tani","request":"Port this game","target":{{"kind":"local","path":"/mnt/c/Users/me/Downloads/Tani.apks"}},
        "stack":"Rust + egui (eframe)","status":"{status}","note":"{note}","created_ms":1764080000000,"updated_ms":1764080001000,
        "checks":[{checks}],"builder":"Claude Code (herdr)"}}"#
    )
}

const CHECK: &str = r#"{"round":1,"check":"build","passed":true,"similarity":72.0,"note":"It builds.","artifacts":[],"at_ms":1764080002000}"#;
const FEATURES: &str = r#"{"version":1,"features":[
  {"name":"Title screen","state":"ready_to_test","guide":"tests/title.md","similarity":80},
  {"name":"Map","state":"in_progress"}]}"#;

fn usage(input: u64, output: u64, cost: f64, priced: bool) -> String {
    format!(
        r#"{{"version":1,"work_ms":2255188,"input_tokens":{input},"output_tokens":{output},"tokens_incomplete":false,"cost_usd":{cost},"api_billed_usd":0.0,"priced":{priced}}}"#
    )
}

fn project() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    fs::write(d.path().join(MANIFEST), manifest("building", "Building the first version…", "")).unwrap();
    d
}

fn write(dir: &Path, name: &str, text: &str) {
    let path = dir.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

#[test]
fn a_windows_folder_in_a_registration_is_read_as_wsl_sees_it() {
    let d = tempfile::tempdir().unwrap();
    let file = d.path().join("1-tani.json");
    fs::write(&file, r#"{"version":1,"folder":"C:\\Projects\\MyProject\\tani","name":"Tani","kind":"local","target":"x.apks","stack":"","created_ms":1}"#)
        .unwrap();
    let reg = Registration::read(&file).unwrap();
    assert_eq!(reg.folder_path(), PathBuf::from("/mnt/c/Projects/MyProject/tani"));
    let linux = Registration { folder: "/mnt/c/Projects/x".into(), ..reg };
    assert_eq!(linux.folder_path(), PathBuf::from("/mnt/c/Projects/x"), "a WSL path stays");
    assert!(Registration::read(&d.path().join("missing.json")).is_err());
}

#[test]
fn registrations_are_listed_oldest_first_and_moved_to_done() {
    let d = tempfile::tempdir().unwrap();
    for name in ["20-b.json", "10-a.json", "notes.txt"] {
        fs::write(d.path().join(name), "{}").unwrap();
    }
    let files = pending(d.path());
    let names: Vec<_> = files.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert_eq!(names, ["10-a.json", "20-b.json"]);
    finish(&files[0]).unwrap();
    assert!(d.path().join(DONE_DIR).join("10-a.json").is_file());
    assert_eq!(pending(d.path()).len(), 1, "done/ is not read again");
    assert_eq!(register_dir(Path::new("/p"), None), PathBuf::from("/p/.bot/register"));
}

#[test]
fn a_project_needs_craft_json_and_the_bots_own_ports_are_skipped() {
    let root = tempfile::tempdir().unwrap();
    let ports = root.path().join("ports");
    let mine = project();
    assert_eq!(admit(&Mirrors::default(), &ports, mine.path()), Admit::New);
    let empty = tempfile::tempdir().unwrap();
    assert!(matches!(admit(&Mirrors::default(), &ports, empty.path()), Admit::Skip(why) if why.contains("craft.json")));
    let owned = ports.join("snake").join("project");
    write(&owned, MANIFEST, &manifest("building", "", ""));
    assert!(matches!(admit(&Mirrors::default(), &ports, &owned), Admit::Skip(why) if why.contains("own ports")));
}

#[test]
fn a_restart_finds_the_mirror_again_so_no_second_channel_is_made() {
    let root = tempfile::tempdir().unwrap();
    let folder = project();
    let mut mirrors = Mirrors::load(root.path()).unwrap();
    assert!(mirrors.projects.is_empty(), "no file yet is no mirrors");
    let mut seen = Seen::default();
    let _ = poll(folder.path(), &mut seen);
    mirrors.projects.push(Mirror { folder: folder.path().to_path_buf(), slug: "tani".into(), channel: 42, seen });
    mirrors.save(root.path()).unwrap();
    let again = Mirrors::load(root.path()).unwrap();
    assert_eq!(again, mirrors, "everything seen survives the restart");
    assert_eq!(admit(&again, root.path(), folder.path()), Admit::Known);
    let mut reloaded = again.find(folder.path()).unwrap().seen.clone();
    assert!(poll(folder.path(), &mut reloaded).events.is_empty(), "nothing is reported twice after a restart");
    fs::write(Mirrors::path(root.path()), "{ not json").unwrap();
    assert!(Mirrors::load(root.path()).is_err(), "an unreadable file is an error, never no mirrors");
}

#[test]
fn only_files_whose_size_or_time_changed_are_read_again() {
    let folder = project();
    let mut seen = Seen::default();
    assert_eq!(changed_files(folder.path(), &mut seen), [MANIFEST], "only craft.json exists");
    assert!(changed_files(folder.path(), &mut seen).is_empty(), "nothing changed");
    write(folder.path(), USAGE_FILE, &usage(10, 5, 0.0, false));
    assert_eq!(changed_files(folder.path(), &mut seen), [USAGE_FILE]);
    fs::remove_file(folder.path().join(USAGE_FILE)).unwrap();
    assert_eq!(changed_files(folder.path(), &mut seen), [USAGE_FILE], "a file that went away counts as changed");
}

#[test]
fn the_project_files_become_the_events_a_rebuild_run_prints() {
    let folder = project();
    write(folder.path(), features::FEATURES_FILE, FEATURES);
    write(folder.path(), USAGE_FILE, &usage(1000, 200, 0.5, true));
    let mut seen = Seen::default();
    let first = poll(folder.path(), &mut seen);
    assert!(first.problems.is_empty(), "{:?}", first.problems);
    assert_eq!(first.events[0], Event::Step { step: "build".into(), label: "Build".into(), text: "Building the first version…".into() });
    assert!(first.events.contains(&Event::Feature {
        name: "Title screen".into(),
        state: "ready_to_test".into(),
        guide: Some("tests/title.md".into()),
        note: None,
        similarity: Some(80.0)
    }));
    assert!(first.events.iter().any(|e| matches!(e, Event::Feature { name, state, .. } if name == "Map" && state == "in_progress")));
    assert!(first.events.iter().any(
        |e| matches!(e, Event::TurnDone { input_tokens: 1000, output_tokens: 200, cost_usd: Some(c), cost_is_estimate: true, .. } if (*c - 0.5).abs() < 1e-9)
    ));

    // A later write: only what changed is reported, and usage only by its difference.
    write(folder.path(), MANIFEST, &manifest("checking", "Checking it", CHECK));
    write(folder.path(), USAGE_FILE, &usage(1500, 260, 0.75, true));
    write(folder.path(), features::FEATURES_FILE, &FEATURES.replace("in_progress", "ready_to_test\",\"guide\":\"tests/map.md"));
    let next = poll(folder.path(), &mut seen);
    assert!(next.events.iter().any(|e| matches!(e, Event::Step { step, .. } if step == "check")));
    assert!(next.events.iter().any(|e| matches!(e, Event::Checked { round: 1, passed: true, similarity: Some(_), .. })));
    assert!(next.events.iter().any(|e| matches!(e, Event::TurnDone { input_tokens: 500, output_tokens: 60, .. })), "{:?}", next.events);
    let features: Vec<_> = next.events.iter().filter(|e| matches!(e, Event::Feature { .. })).collect();
    assert_eq!(features.len(), 1, "the unchanged Title screen is not reported again: {features:?}");

    // The board folds them as it folds a run's events.
    let mut board = Board::new();
    first.events.iter().chain(&next.events).for_each(|e| board.apply(e));
    assert_eq!(board.features().len(), 2);
}

#[test]
fn ready_and_paused_end_the_board_and_building_again_is_a_new_run() {
    let folder = project();
    let mut seen = Seen::default();
    let _ = poll(folder.path(), &mut seen);
    write(folder.path(), MANIFEST, &manifest("ready", "Ready to test", ""));
    let ready = poll(folder.path(), &mut seen);
    assert!(ready.events.iter().any(|e| matches!(e, Event::Finished { ok: true, .. })));
    assert!(!ready.restarted);
    write(folder.path(), MANIFEST, &manifest("building", "Working on the follow-up request", ""));
    assert!(poll(folder.path(), &mut seen).restarted, "a new request after the end is a new run");
    write(folder.path(), MANIFEST, &manifest("stopped", "", ""));
    assert!(poll(folder.path(), &mut seen).events.iter().any(|e| matches!(e, Event::Finished { ok: false, paused: true, .. })));
}

#[test]
fn a_broken_features_file_is_a_problem_not_a_silence() {
    let folder = project();
    write(folder.path(), features::FEATURES_FILE, "{ broken");
    let mut seen = Seen::default();
    let found = poll(folder.path(), &mut seen);
    assert!(found.problems.iter().any(|p| p.starts_with(features::FEATURES_FILE)), "{:?}", found.problems);
}

#[test]
fn the_first_look_waits_for_the_study_and_lists_what_it_found() {
    let folder = project();
    let mut seen = Seen::default();
    let _ = poll(folder.path(), &mut seen);
    assert!(!crate::first_look::due(&seen), "no study notes yet");
    write(folder.path(), STUDY_NOTES, "# What we found in Tani.apks\n\nText.\n\n## Ghidra\n");
    write(folder.path(), "capture/screens/main-menu.png", "png");
    write(folder.path(), "reference/shop_page.jpg", "jpg");
    write(folder.path(), features::FEATURES_FILE, FEATURES);
    let found = poll(folder.path(), &mut seen);
    assert!(crate::first_look::due(&seen));
    let m = found.manifest.unwrap_or_else(|| read_project(folder.path()).unwrap());
    let text = crate::first_look::text(folder.path(), &m, &seen.features);
    for want in ["Android app", "Rust + egui (eframe)", "main menu", "shop page", "Ghidra", "Title screen (ready to test)", "(from app)"] {
        assert!(text.contains(want), "{want} missing from:\n{text}");
    }
    seen.first_look_posted = true;
    assert!(!crate::first_look::due(&seen), "posted once");
}

#[test]
fn the_first_look_of_an_android_app_without_pictures_says_it_needs_a_device() {
    let folder = project();
    write(folder.path(), STUDY_NOTES, "# What we found in Tani.apks\n");
    let m = read_project(folder.path()).unwrap();
    let text = crate::first_look::text(folder.path(), &m, &[]);
    assert!(text.contains("**Original screens:** no screenshots of the original (needs a device)"), "{text}");
    assert!(crate::first_look::original_pictures(folder.path()).is_empty());
    write(folder.path(), "capture/window.png", "png");
    let text = crate::first_look::text(folder.path(), &m, &[]);
    assert!(text.contains("**Original screens (1)**") && !text.contains("needs a device"), "{text}");
    assert_eq!(crate::first_look::original_pictures(folder.path()), [folder.path().join("capture/window.png")]);
}
