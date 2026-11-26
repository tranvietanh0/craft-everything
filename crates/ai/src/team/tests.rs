//! Fixtures in the shape Claude Code 2.1 writes (keys as found in real sessions; contents made up).

use super::discord::{BotLink, bot_link};
use super::*;
use serde_json::json;
use std::path::Path;

fn ts(sec: u32) -> String {
    format!("2026-11-24T20:{:02}:{:02}.000Z", sec / 60, sec % 60)
}

fn call(sec: u32, id: &str, name: &str, input: serde_json::Value, side: bool) -> String {
    json!({"type": "assistant", "isSidechain": side, "timestamp": ts(sec), "message": {"id": format!("m{id}"), "model": "claude-opus-5-5",
        "content": [{"type": "tool_use", "id": id, "name": name, "input": input}], "stop_reason": "tool_use",
        "usage": {"input_tokens": 1, "cache_read_input_tokens": 10, "output_tokens": 5}}})
    .to_string()
}

fn result(sec: u32, id: &str, extra: serde_json::Value, side: bool) -> String {
    json!({"type": "user", "isSidechain": side, "timestamp": ts(sec), "toolUseResult": extra,
        "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": id, "content": [{"type": "text", "text": "Found the menu code.\nmore"}]}]}})
    .to_string()
}

fn spawn(sec: u32, id: &str, kind: &str, side: bool) -> String {
    call(sec, id, "Agent", json!({"subagent_type": kind, "description": format!("{kind} work"), "prompt": "…"}), side)
}

/// A session: the builder calls craft tools and a skill, runs one subagent to the end, and starts
/// a port runner in the background that starts a nested Explore of its own (still running).
fn write_session(dir: &Path, now_sec: u32) -> Session {
    let main = [
        r#"{"type":"permission-mode","permissionMode":"default","sessionId":"s1"}"#.to_string(),
        call(1, "t1", "mcp__craft-anything__core_brief_write", json!({}), false),
        result(2, "t1", json!("ok"), false),
        call(3, "t2", "Skill", json!({"skill": "gk-apk-port"}), false),
        result(4, "t2", json!({}), false),
        spawn(5, "a1", "Explore", false),
        result(65, "a1", json!({"status": "completed", "agentId": "x1", "totalDurationMs": 60000, "totalTokens": 900, "totalToolUseCount": 3}), false),
        spawn(70, "a2", "gk-apk-port-runner", false),
        result(71, "a2", json!({"status": "async_launched", "isAsync": true, "agentId": "x2", "resolvedModel": "claude-sonnet-5-5"}), false),
        call(72, "t3", "mcp__craft-anything__core_brief_write", json!({}), false),
    ];
    let subs = dir.join("s1").join("subagents");
    std::fs::create_dir_all(&subs).unwrap();
    std::fs::write(dir.join("s1.jsonl"), main.join("\n") + "\n").unwrap();
    let sub = |id: &str, meta: serde_json::Value, lines: &[String]| {
        std::fs::write(subs.join(format!("agent-{id}.meta.json")), meta.to_string()).unwrap();
        std::fs::write(subs.join(format!("agent-{id}.jsonl")), lines.join("\n") + "\n").unwrap();
    };
    sub(
        "x1",
        json!({"agentType": "Explore", "toolUseId": "a1", "finished": true}),
        &[call(10, "u1", "Read", json!({}), true), result(11, "u1", json!({}), true)],
    );
    sub(
        "x2",
        json!({"agentType": "gk-apk-port-runner", "toolUseId": "a2", "finished": false}),
        &[
            call(72, "v1", "Bash", json!({}), true),
            result(73, "v1", json!({}), true),
            spawn(74, "a3", "Explore", true),
            call(now_sec, "v2", "Bash", json!({}), true),
        ],
    );
    sub("x3", json!({"agentType": "Explore", "toolUseId": "a3", "finished": false}), &[call(now_sec, "w1", "Grep", json!({}), true)]);
    Session { id: "s1".into(), path: dir.join("s1.jsonl"), modified: std::time::SystemTime::now() }
}

fn team_at(dir: &Path, now_sec: u32) -> Team {
    let session = write_session(dir, now_sec);
    let mut w = TeamWatch::new(&session);
    w.poll().unwrap();
    w.team(timestamp(&ts(now_sec)).unwrap())
}

#[test]
fn spawns_nest_under_whoever_started_them() {
    let dir = tempfile::tempdir().unwrap();
    let team = team_at(dir.path(), 80);
    let b = &team.builder;
    let kinds: Vec<&str> = b.children.iter().map(|c| c.kind.as_str()).collect();
    assert_eq!(kinds, ["Explore", "gk-apk-port-runner"], "the builder's two starts, in order");
    let runner = &b.children[1];
    assert_eq!(runner.children.len(), 1, "the runner's own Explore sits under the runner, not the builder");
    assert_eq!(b.count(), 4);
    assert!(team.problems.is_empty(), "{:?}", team.problems);
}

#[test]
fn a_background_subagent_is_running_until_its_notification() {
    let dir = tempfile::tempdir().unwrap();
    let team = team_at(dir.path(), 80);
    let (explore, runner) = (&team.builder.children[0], &team.builder.children[1]);
    assert_eq!(explore.state, AgentState::Done);
    assert!(explore.summary.as_deref().is_some_and(|s| s.starts_with("Found")));
    assert!(runner.state.is_live() && runner.children[0].state.is_live(), "{:?}", runner.state);
    assert!(team.builder.live() >= 3, "builder, runner and its Explore are all working");
    assert!(runner.duration_ms.unwrap_or(0) > 0, "a running agent's time so far");

    // Its end, as Claude Code queues it for the builder.
    let note = json!({"type": "queue-operation", "operation": "enqueue", "timestamp": ts(90),
        "content": "<task-notification>\n<task-id>x2</task-id>\n<tool-use-id>a2</tool-use-id>\n<status>completed</status>\n<summary>Ported the menu</summary>\n<usage><subagent_tokens>5000</subagent_tokens><tool_uses>9</tool_uses><duration_ms>20000</duration_ms></usage>\n</task-notification>"});
    let path = dir.path().join("s1.jsonl");
    let mut w = TeamWatch::new(&Session { id: "s1".into(), path: path.clone(), modified: std::time::SystemTime::now() });
    w.poll().unwrap();
    let before = w.team(timestamp(&ts(90)).unwrap());
    assert!(before.builder.children[1].state.is_live());
    let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
    std::io::Write::write_all(&mut f, format!("{note}\n").as_bytes()).unwrap();
    w.poll().unwrap();
    let after = w.team(timestamp(&ts(90)).unwrap());
    let runner = &after.builder.children[1];
    assert_eq!(runner.state, AgentState::Done, "only the appended line was read, and it ended the runner");
    assert!(runner.tokens.unwrap_or(0) > before.builder.children[1].tokens.unwrap_or(0));
}

#[test]
fn tools_are_counted_and_grouped_per_agent() {
    let dir = tempfile::tempdir().unwrap();
    let team = team_at(dir.path(), 80);
    let b = &team.builder;
    let brief = b.tools.iter().find(|t| t.name == "core_brief_write").unwrap();
    assert_eq!((brief.group, brief.count), (ToolGroup::Craft, 2));
    assert!(b.tools.iter().any(|t| t.group == ToolGroup::Skill && t.name.contains("gk-apk-port")));
    assert!(b.tools[0].count >= b.tools.last().unwrap().count, "most-called first");
    assert!(!b.tools.iter().any(|t| t.name == "Bash"), "a subagent's calls aren't the builder's");
    assert_eq!(b.state, AgentState::InTool("core_brief_write".into()), "its last call has no result yet");
}

#[test]
fn an_agent_with_no_entry_for_a_while_is_quiet_not_working() {
    let dir = tempfile::tempdir().unwrap();
    let team = team_at(dir.path(), 80);
    let session = Session { id: "s1".into(), path: dir.path().join("s1.jsonl"), modified: std::time::SystemTime::now() };
    let mut w = TeamWatch::new(&session);
    w.poll().unwrap();
    let later = w.team(timestamp("2026-11-24T21:30:00.000Z").unwrap());
    assert!(team.builder.state.is_live());
    assert_eq!(later.builder.state, AgentState::Quiet);
}

#[test]
fn the_bot_link_comes_from_the_bots_own_files() {
    let root = tempfile::tempdir().unwrap();
    let ports = root.path().join("ports");
    let register = ports.join(".bot").join("register");
    std::fs::create_dir_all(&register).unwrap();
    // One of the bot's own ports.
    let port = ports.join("tani").join("project");
    std::fs::create_dir_all(&port).unwrap();
    std::fs::write(ports.join("tani").join("bot.json"), json!({"slug": "tani", "label": "Tani", "jobs": [{}, {}], "cards": [{}]}).to_string()).unwrap();
    assert!(matches!(bot_link(&port, None), BotLink::Port { runs: 2, cards: 1, .. }));
    // A project made in the app and mirrored.
    let app = root.path().join("app");
    std::fs::write(
        ports.join(".bot").join("mirrors.json"),
        json!({"projects": [{"folder": app, "slug": "app", "channel": 1, "seen": {"status": "building", "checks": 2, "features": [{}], "first_look_posted": true}}]}).to_string(),
    )
    .unwrap();
    assert!(matches!(bot_link(&app, Some(&register)), BotLink::Mirror { checks: 2, features: 1, first_look_posted: true, .. }));
    // Not set up, unknown, and a file that can't be read.
    assert!(matches!(bot_link(&root.path().join("other"), None), BotLink::None { .. }));
    assert!(matches!(bot_link(&root.path().join("other"), Some(&register)), BotLink::None { .. }));
    std::fs::write(ports.join(".bot").join("mirrors.json"), "{not json").unwrap();
    assert!(matches!(bot_link(&app, Some(&register)), BotLink::Unreadable { .. }), "never shown as not followed");
}

#[test]
fn subagents_light_up_the_role_whose_id_is_their_type_at_any_depth() {
    let dir = tempfile::tempdir().unwrap();
    let team = team_at(dir.path(), 80);
    let m = by_role(&team, &["gk-apk-port-runner", "tester"]);
    assert_eq!(m.by_role.get("gk-apk-port-runner").map(Vec::len), Some(1));
    assert!(!m.by_role.contains_key("tester"), "a role nobody worked as has no subagents");
    let others: Vec<&str> = m.others.iter().map(|n| n.kind.as_str()).collect();
    assert_eq!(others, ["Explore", "Explore"], "the builder's Explore and the runner's nested one are other helpers");
}

fn roster_role(id: &str) -> craft_anything_engine::team::Role {
    let mut role = craft_anything_engine::team::default_roster().remove(0);
    role.id = id.into();
    role
}

#[test]
fn the_team_is_written_as_subagents_and_only_craft_files_are_replaced_or_removed() {
    use agent_files::{FileOwner, owners, write};
    let dir = tempfile::tempdir().unwrap();
    let agents = agent_files::agents_dir(dir.path());
    std::fs::create_dir_all(&agents).unwrap();
    let mine = "---\nname: analyst\ndescription: my own\n---\nMine.\n";
    std::fs::write(agents.join("analyst.md"), mine).unwrap();
    std::fs::write(agents.join("notes.md"), "my notes").unwrap();

    let first = write(dir.path(), &[roster_role("analyst"), roster_role("tester"), roster_role("porter")]).unwrap();
    assert_eq!(first.kept, ["analyst"], "the person's file for a role id is left to them");
    assert_eq!(std::fs::read_to_string(agents.join("analyst.md")).unwrap(), mine);
    let tester = std::fs::read_to_string(agents.join("tester.md")).unwrap();
    assert!(tester.starts_with("---\n") && tester.contains("name: \"tester\""), "{tester}");

    let second = write(dir.path(), &[roster_role("analyst"), roster_role("tester")]).unwrap();
    assert_eq!(second.removed, ["porter"], "a role that left the team loses its file");
    let now = owners(dir.path()).unwrap();
    assert!(!now.contains_key("porter"));
    let seen = (now.get("analyst"), now.get("notes"), now.get("tester"));
    assert_eq!(seen, (Some(&FileOwner::Person), Some(&FileOwner::Person), Some(&FileOwner::Craft)));
}

#[test]
fn a_role_file_carries_its_skills_and_hooks_as_frontmatter_claude_code_reads() {
    use craft_anything_engine::team::Hook;
    let mut role = roster_role("tester");
    role.description = "Says \"hi\": then stops".into();
    role.tools = Some(vec!["Read".into(), "Bash".into()]);
    role.skills = vec!["my-skill".into()];
    role.hooks = vec![
        Hook { event: "PreToolUse".into(), matcher: "Bash".into(), command: "./check.sh".into() },
        Hook { event: "Stop".into(), matcher: String::new(), command: "echo done".into() },
    ];
    let text = agent_files::render(&role);
    let front = text.split("---\n").nth(1).unwrap();
    for wanted in [
        r#"description: "Says \"hi\": then stops""#,
        r#"tools: ["Read", "Bash"]"#,
        r#"skills: ["my-skill"]"#,
        "  PreToolUse:\n    - matcher: \"Bash\"\n      hooks:\n        - type: command\n          command: \"./check.sh\"",
        "  Stop:\n    - hooks:\n        - type: command\n          command: \"echo done\"",
    ] {
        assert!(front.contains(wanted), "{wanted} missing from\n{front}");
    }
}
