//! Opt-in live check of Claude Code in herdr: one message in a chat, its events printed as JSON
//! lines. Uses the herdr server running on this computer (or on Windows, from WSL) and the signed-in
//! Claude Code's allowance. Build craft-anything-cli first. Never run in CI.
//!
//! `cargo run -p craft-anything-ai --example herdr_smoke -- <project folder> "<message>"`

use craft_anything_ai::cli_agents::{CliSpec, herdr};
use craft_anything_ai::{AgentEvent, BackendConfig, BackendKind, BackendOptions, Conversation};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(project), Some(message)) = (args.next(), args.next()) else {
        eprintln!("usage: herdr_smoke <project folder> <message>");
        std::process::exit(2);
    };
    let config =
        BackendConfig { name: herdr::DISPLAY.into(), kind: BackendKind::AgentCli, model: herdr::ID.into(), base_url: None, ssh_host: None, enabled: true };
    let mut spec = CliSpec::new(config, BackendOptions::default(), false, Some(PathBuf::from(project)));
    spec.deep = true;
    let mut c = herdr::HerdrConversation::new(spec);
    c.send(&message);
    let deadline = Instant::now() + Duration::from_secs(300);
    let mut ended = false;
    while !ended && Instant::now() < deadline {
        for event in c.poll() {
            ended |= matches!(event, AgentEvent::TurnDone { .. });
            println!("{}", serde_json::to_string(&event).unwrap_or_default());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    println!("{{\"session\":{:?},\"ended\":{ended}}}", c.session_id());
    if !ended {
        c.stop();
    }
    // Dropping it ends Claude Code with /exit; the pane and workspace stay.
    drop(c);
}
