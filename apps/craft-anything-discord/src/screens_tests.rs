use super::*;
use craft_anything_engine::tools::web_common::{Image, encode_png};

fn write(root: &Path, rel: &str, bytes: &[u8]) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn check(round: u32, id: &str, passed: bool, artifacts: &[&str]) -> CheckResult {
    serde_json::from_value(serde_json::json!({
        "round": round, "check": id, "passed": passed, "similarity": null, "note": "",
        "artifacts": artifacts, "at_ms": 0
    }))
    .unwrap()
}

fn shot(rel: &str) -> Shot {
    Shot { path: PathBuf::from("/p").join(rel), rel: rel.to_owned(), modified_ms: 1, len: 1 }
}

#[test]
fn finds_pictures_in_round_folders_only() {
    let project = tempfile::tempdir().unwrap();
    let p = project.path();
    write(p, "checks/round-0/screenshot.png", b"a");
    write(p, "checks/round-2/routes/about/screenshot-390.png", b"b");
    write(p, "checks/round-10/window.PNG", b"c");
    write(p, "checks/round-2/capture.json", b"{}");
    write(p, "checks/asks/screen.png", b"not a round");
    write(p, "capture/window.png", b"the original");
    let rels: Vec<String> = find(p).into_iter().map(|s| s.rel).collect();
    assert_eq!(rels, ["checks/round-0/screenshot.png", "checks/round-2/routes/about/screenshot-390.png", "checks/round-10/window.PNG"]);
    assert!(find(&p.join("nothing")).is_empty(), "a project without checks/ has none");
}

#[test]
fn batches_hold_at_most_ten_and_the_size_limit() {
    let sizes = vec![1u64; 23];
    let groups = batches(&sizes, MOST_PER_MESSAGE, UPLOAD_LIMIT);
    assert_eq!(groups.iter().map(Vec::len).collect::<Vec<_>>(), [10, 10, 3]);
    assert_eq!(groups.concat(), (0..23).collect::<Vec<_>>(), "every item once, in order");
    // By bytes: 5 + 4 fits in 10, the next 4 does not; one item over the limit goes alone.
    assert_eq!(batches(&[5, 4, 4, 11, 1], 10, 10), [vec![0, 1], vec![2], vec![3], vec![4]]);
    assert!(batches(&[], 10, 10).is_empty());
}

#[test]
fn a_picture_over_the_limit_is_scaled_down_or_skipped() {
    let dir = tempfile::tempdir().unwrap();
    // Noise compresses badly, so the PNG is far over a small limit.
    let (w, h) = (256u32, 256u32);
    let rgba: Vec<u8> = (0..w * h * 4).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
    let png = encode_png(&Image { width: w, height: h, rgba }).unwrap();
    let path = dir.path().join("big.png");
    std::fs::write(&path, &png).unwrap();
    let limit = png.len() as u64 / 3;
    let (bytes, scaled) = upload_bytes(&path, limit).unwrap();
    assert!(scaled && bytes.len() as u64 <= limit, "scaled to {} bytes for a limit of {limit}", bytes.len());
    assert_eq!(upload_bytes(&path, png.len() as u64).unwrap(), (png.clone(), false), "a picture that fits is sent as it is");
    let err = upload_bytes(&path, 16).unwrap_err();
    assert!(err.contains("over Discord's limit"), "{err}");
    std::fs::write(dir.path().join("bad.png"), vec![0u8; 64]).unwrap();
    assert!(upload_bytes(&dir.path().join("bad.png"), 8).unwrap_err().contains("couldn't be read"));
}

#[test]
fn posted_pictures_are_not_posted_again_after_a_restart() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("app/project");
    write(&project, "checks/round-1/screenshot.png", b"one");
    write(&project, "checks/round-1/window.png", b"two");
    let mut posted = Posted::load(root.path()).unwrap();
    let first = unposted(find(&project), &posted);
    assert_eq!(first.len(), 2);
    posted.mark(&first[0]);
    posted.save(root.path()).unwrap();
    // A restart: the store is read again from disk.
    let again = Posted::load(root.path()).unwrap();
    let left = unposted(find(&project), &again);
    assert_eq!(left.iter().map(|s| s.rel.as_str()).collect::<Vec<_>>(), ["checks/round-1/window.png"]);
    // The same path written again (a newer modification time) is a new picture.
    let mut rewritten = first[0].clone();
    rewritten.modified_ms += 1000;
    assert!(!again.contains(&rewritten));
}

#[test]
fn an_unreadable_store_is_an_error_not_empty() {
    let root = tempfile::tempdir().unwrap();
    write(root.path(), &format!("{BOT_DIR}/{SHOTS_FILE}"), b"not json");
    assert!(Posted::load(root.path()).unwrap_err().contains("is not valid"));
}

#[test]
fn captions_name_the_round_the_screen_and_the_verdict() {
    let checks = [check(1, "build", true, &[]), check(1, "app-screenshot", true, &["checks/round-0/screenshot.png"])];
    let s = shot("checks/round-0/screenshot.png");
    assert_eq!(caption(&s, made_by(&s.rel, &checks)), "Round 1 · app screen · ✅ app-screenshot passed");
    let failed = [check(2, "web-visual", false, &["checks/round-2/routes/about/screenshot-390.png"])];
    let s = shot("checks/round-2/routes/about/screenshot-390.png");
    assert_eq!(caption(&s, made_by(&s.rel, &failed)), "Round 2 · about · 390 px wide · ❌ web-visual failed");
    assert_eq!(caption(&shot("checks/round-0/window.png"), None), "Builder's own check · app started normally · no verdict yet");
    assert_eq!(caption(&shot("checks/round-3/main_menu.png"), None), "Round 3 · main menu · no verdict yet");
}

#[test]
fn the_latest_check_that_lists_a_picture_gives_its_verdict() {
    let checks = [check(1, "app-screenshot", false, &["checks/round-0/screenshot.png"]), check(2, "app-screenshot", true, &["checks/round-0/screenshot.png"])];
    assert_eq!(made_by("checks/round-0/screenshot.png", &checks).map(|c| c.round), Some(2));
    assert!(made_by("checks/round-0/other.png", &checks).is_none());
}

#[test]
fn upload_names_are_unique_per_round() {
    assert_eq!(shot("checks/round-1/routes/about/screenshot-390.png").upload_name(), "round-1-routes-about-screenshot-390.png");
    assert_ne!(shot("checks/round-1/screenshot.png").upload_name(), shot("checks/round-2/screenshot.png").upload_name());
}
