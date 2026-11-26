use super::*;
use crate::workspace::{MANIFEST, Manifest, TargetRef, Workspace};

fn custom(id: &str) -> Role {
    Role { hooks: vec![], ..role(id, "Custom", "Does a custom job.", "Do the custom job.") }
}

#[test]
fn the_default_roster_is_five_valid_distinct_roles() {
    let roster = default_roster();
    assert_eq!(roster.len(), 5);
    assert!(roster.iter().all(|r| r.validate().is_ok()));
    let mut ids: Vec<_> = roster.iter().map(|r| r.id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), roster.len());
}

#[test]
fn a_project_without_a_saved_team_gets_the_default_and_writes_none() {
    let d = tempfile::tempdir().unwrap();
    let ws = Workspace::create(d.path(), "Game", "A game", TargetRef::None).unwrap();
    let saved = std::fs::read_to_string(ws.root.join(MANIFEST)).unwrap();
    assert!(!saved.contains("\"team\""), "the default roster is not written: {saved}");
    assert_eq!(Workspace::open(&ws.root).unwrap().manifest.roster(), default_roster());
}

#[test]
fn a_team_round_trips_through_craft_json_with_hooks_and_skills() {
    let mut team = Role { skills: vec!["pdf-tools".into()], model: Some("sonnet".into()), ..custom("reviewer") };
    team.hooks = vec![Hook { event: "PostToolUse".into(), matcher: "Edit|Write".into(), command: "echo done".into() }];
    let back: Manifest = serde_json::from_str(&serde_json::to_string(&with_team(Some(vec![team.clone()]))).unwrap()).unwrap();
    assert_eq!(back.team, Some(vec![team]));
}

/// A project's manifest with `team` set through the person's own path ([`Workspace::set_person`]).
fn with_team(team: Option<Vec<Role>>) -> Manifest {
    let d = tempfile::tempdir().unwrap();
    let mut ws = Workspace::create(d.path(), "Game", "A game", TargetRef::None).unwrap();
    ws.set_person(|m| m.team = team).unwrap();
    ws.manifest
}

#[test]
fn an_empty_saved_team_is_kept_as_empty_not_the_default() {
    let back: Manifest = serde_json::from_str(&serde_json::to_string(&with_team(Some(vec![]))).unwrap()).unwrap();
    assert!(back.roster().is_empty());
}

#[test]
fn ids_are_file_safe_slugs() {
    for ok in ["analyst", "asset-porter", "r2d2", "a"] {
        assert!(valid_id(ok), "{ok} should be valid");
    }
    for bad in ["", "Analyst", "-a", "a-", "a--b", "1a", "a b", "a/b", "a.b", "../x"] {
        assert!(!valid_id(bad), "{bad:?} should be refused");
    }
    assert!(!valid_id(&"a".repeat(MAX_ID_LEN + 1)));
}

#[test]
fn a_role_with_a_bad_field_is_refused_before_it_is_kept() {
    let mut r = custom("ok");
    r.name = "  ".into();
    assert!(r.validate().is_err());
    r = custom("ok");
    r.prompt = "x\u{7}".into();
    assert!(r.validate().is_err(), "a control character in the prompt is refused");
    r = custom("ok");
    r.model = Some("sonnet; rm".into());
    assert!(r.validate().is_err());
    r = custom("ok");
    r.hooks = vec![Hook { event: "Pre Tool".into(), matcher: String::new(), command: "x".into() }];
    assert!(r.validate().is_err());
}

#[test]
fn set_role_adds_then_replaces_by_id_and_keeps_the_order() {
    let mut roster = vec![custom("a"), custom("b")];
    set_role(&mut roster, custom("c"), None).unwrap();
    assert_eq!(roster.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["a", "b", "c"]);
    let renamed = Role { name: "Renamed".into(), ..custom("b") };
    set_role(&mut roster, renamed.clone(), None).unwrap();
    assert_eq!(roster[1], renamed);
    assert_eq!(roster.len(), 3);
}

#[test]
fn a_role_can_change_its_id_but_not_take_another_roles() {
    let mut roster = vec![custom("a"), custom("b")];
    set_role(&mut roster, custom("z"), Some("a")).unwrap();
    assert_eq!(roster.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["z", "b"]);
    let clash = set_role(&mut roster, custom("b"), Some("z")).unwrap_err();
    assert!(clash.message.contains("already"), "{}", clash.message);
    assert_eq!(roster[0].id, "z", "a refused change leaves the roster as it was");
    assert!(set_role(&mut roster, custom("y"), Some("nope")).is_err());
}

#[test]
fn a_team_has_a_limit_on_its_roles() {
    let mut roster: Vec<Role> = (0..MAX_ROLES).map(|i| custom(&format!("r{i}"))).collect();
    assert!(set_role(&mut roster, custom("extra"), None).is_err());
    assert_eq!(roster.len(), MAX_ROLES);
    assert!(set_role(&mut roster, custom("r0"), None).is_ok(), "replacing one still fits");
}

#[test]
fn remove_role_takes_the_role_off_and_refuses_an_unknown_id() {
    let mut roster = default_roster();
    remove_role(&mut roster, "tester").unwrap();
    assert_eq!(roster.len(), 4);
    assert!(roster.iter().all(|r| r.id != "tester"));
    assert_eq!(remove_role(&mut roster, "tester").unwrap_err().code, ErrorCode::NotFound);
}
