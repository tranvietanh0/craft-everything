//! The project's Team tab: who works on the build. Craft Anything starts the builder (Claude
//! Code), the builder starts subagents (and they theirs), and each calls tools; the Discord bot
//! follows the project by reading its files. Read from Claude Code's transcripts by a worker
//! thread every 1.5 s while the tab is shown, so drawing never waits on a file.

use crate::i18n::{tr, trf};
use crate::theme::{self, Tokens};
use crate::{icons, widgets};
use craft_anything_ai::team::discord::{BotLink, bot_link};
use craft_anything_ai::team::{AgentNode, AgentState, Session, Team, TeamWatch, ToolGroup, sessions, transcript_dirs};
use craft_anything_platform::sync::Mutex;
use egui::{RichText, Ui};
use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError};
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_millis(1500);
/// The worker stops when the tab hasn't been drawn for this long.
const UNSEEN: Duration = Duration::from_secs(5);

/// What the tab can show.
#[derive(Default, Clone)]
enum View {
    /// Nothing read yet.
    #[default]
    Reading,
    /// No conversation of Claude Code's in this folder.
    NoSession,
    Team(Box<Team>),
    /// Why nothing can be shown.
    Failed(String),
}

#[derive(Default)]
struct Shared {
    viewed: Option<Instant>,
    /// The session the person picked; `None` follows the newest.
    chosen: Option<String>,
    dirs: Vec<PathBuf>,
    sessions: Vec<Session>,
    team: View,
    bot: Option<BotLink>,
    running: bool,
}

/// The Team tab of one project.
pub struct TeamPanel {
    root: PathBuf,
    shared: Arc<Mutex<Shared>>,
}

impl TeamPanel {
    pub fn new(root: PathBuf) -> Self {
        TeamPanel { root, shared: Arc::new(Mutex::new(Shared::default())) }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Notes the tab was shown and starts the worker when it isn't running.
    fn keep_alive(&self, ctx: &egui::Context) {
        let mut s = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
        s.viewed = Some(Instant::now());
        if s.running {
            return;
        }
        s.running = true;
        let (shared, root, ctx) = (self.shared.clone(), self.root.clone(), ctx.clone());
        let started = std::thread::Builder::new().name("team-watch".into()).spawn(move || watch(&shared, &root, &ctx));
        if let Err(e) = started {
            s.running = false;
            s.team = View::Failed(trf("Couldn't start reading the team: {e}", &[("e", &e.to_string())]));
        }
    }
}

/// The worker: lists the sessions, follows the chosen one and the bot's files until the tab is
/// no longer shown.
fn watch(shared: &Mutex<Shared>, root: &Path, ctx: &egui::Context) {
    let dirs = transcript_dirs(root);
    let register = std::env::var_os(craft_anything_engine::workspace::register::REGISTER_VAR).filter(|v| !v.is_empty()).map(PathBuf::from);
    let mut current: Option<TeamWatch> = None;
    let mut current_id = String::new();
    loop {
        let chosen = {
            let mut s = shared.lock().unwrap_or_else(PoisonError::into_inner);
            if s.viewed.is_none_or(|v| v.elapsed() > UNSEEN) {
                s.running = false;
                return;
            }
            s.chosen.clone()
        };
        let (list, team) = match sessions(&dirs) {
            Err(e) => (Vec::new(), View::Failed(e)),
            Ok(list) => {
                let pick = chosen.as_ref().and_then(|c| list.iter().find(|s| &s.id == c)).or(list.first()).cloned();
                let team = match pick {
                    None => View::NoSession,
                    Some(p) => {
                        if current_id != p.id {
                            current = None;
                            current_id = p.id.clone();
                        }
                        let w = current.get_or_insert_with(|| TeamWatch::new(&p));
                        match w.poll() {
                            Ok(()) => View::Team(Box::new(w.team(craft_anything_ai::team::now()))),
                            Err(e) => View::Failed(e),
                        }
                    }
                };
                (list, team)
            }
        };
        let bot = bot_link(root, register.as_deref());
        {
            let mut s = shared.lock().unwrap_or_else(PoisonError::into_inner);
            s.dirs = dirs.clone();
            s.sessions = list;
            s.team = team;
            s.bot = Some(bot);
        }
        ctx.request_repaint();
        std::thread::sleep(POLL);
    }
}

pub fn show(panel: &TeamPanel, ui: &mut Ui) {
    panel.keep_alive(ui.ctx());
    let t = Tokens::get(ui.ctx());
    let (sessions, team, bot, dirs, chosen) = {
        let s = panel.shared.lock().unwrap_or_else(PoisonError::into_inner);
        (s.sessions.clone(), s.team.clone(), s.bot.clone(), s.dirs.clone(), s.chosen.clone())
    };
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.label(RichText::new(tr("Who works on this build, and the tools each one called.")).color(t.text_dim));
        ui.add_space(theme::space::SM);
        if !sessions.is_empty() {
            session_picker(panel, ui, &sessions, chosen.as_deref());
            ui.add_space(theme::space::SM);
        }
        match &team {
            View::Reading => widgets::working(ui, &tr("Reading the builder's conversation…"), None),
            View::Failed(e) => {
                widgets::empty_state(ui, "triangle-alert", "The team can't be read", "");
                ui.label(RichText::new(e).color(t.danger));
            }
            View::NoSession => {
                let looked: Vec<String> = dirs.iter().map(|d| d.display().to_string()).collect();
                widgets::empty_state(ui, "bot", "No builder conversation yet", "It shows here once a build with Claude Code starts.");
                ui.label(RichText::new(trf("Looked in: {dirs}", &[("dirs", &looked.join(", "))])).color(t.text_faint).size(theme::CAPTION));
            }
            View::Team(team) => {
                widgets::card(&t).inner_margin(theme::pad::TIGHT).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let header = trf(
                        "Craft Anything (orchestrator): {n} agent(s), {live} working",
                        &[("n", &team.builder.count().to_string()), ("live", &team.builder.live().to_string())],
                    );
                    egui::CollapsingHeader::new(RichText::new(header).font(theme::semibold(theme::BODY)))
                        .id_salt(("team-root", &team.session))
                        .default_open(true)
                        .icon(widgets::disclosure_icon)
                        .show(ui, |ui| agent(ui, &t, &team.builder, &team.session, 0));
                });
                for p in &team.problems {
                    ui.label(RichText::new(p).color(t.warning).size(theme::CAPTION));
                }
            }
        }
        ui.add_space(theme::space::MD);
        if let Some(bot) = &bot {
            discord(ui, &t, bot);
        }
    });
}

fn session_picker(panel: &TeamPanel, ui: &mut Ui, sessions: &[Session], chosen: Option<&str>) {
    let shown = chosen.and_then(|c| sessions.iter().find(|s| s.id == c)).or(sessions.first());
    let label = |s: &Session, i: usize| {
        let ago = s.modified.elapsed().map(|d| d.as_secs() / 60).unwrap_or(0);
        let short: String = s.id.chars().take(8).collect();
        if i == 0 {
            trf("{id} (latest, {m} min ago)", &[("id", &short), ("m", &ago.to_string())])
        } else {
            trf("{id} ({m} min ago)", &[("id", &short), ("m", &ago.to_string())])
        }
    };
    ui.horizontal(|ui| {
        widgets::caption(ui, "Conversation");
        let current = shown.map(|s| label(s, sessions.iter().position(|x| x.id == s.id).unwrap_or(0))).unwrap_or_default();
        widgets::dropdown("team-session").selected_text(current).show_ui(ui, |ui| {
            for (i, s) in sessions.iter().enumerate() {
                if widgets::menu_item(ui, shown.is_some_and(|x| x.id == s.id), label(s, i)).clicked() {
                    let mut sh = panel.shared.lock().unwrap_or_else(PoisonError::into_inner);
                    sh.chosen = if i == 0 { None } else { Some(s.id.clone()) };
                }
            }
        });
    });
}

/// What a state is called, its icon and colour (the words carry it; the colour only helps).
fn state_look(state: &AgentState, t: &Tokens) -> (String, &'static str, egui::Color32) {
    match state {
        AgentState::Running => (tr("Working"), "loader", t.accent),
        AgentState::InTool(tool) => (trf("In {tool} (running, or waiting for approval in herdr)", &[("tool", tool)]), "wrench", t.accent),
        AgentState::Idle => (tr("Waiting for the next message"), "moon", t.text_dim),
        AgentState::Quiet => (tr("No activity for 5 minutes"), "circle-help", t.warning),
        AgentState::Done => (tr("Done"), "check", t.success),
        AgentState::Failed => (tr("Failed"), "triangle-alert", t.danger),
        AgentState::Stopped => (tr("Stopped"), "circle-stop", t.warning),
    }
}

fn duration(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 {
        format!("{}h {:02}m", s / 3600, (s % 3600) / 60)
    } else if s >= 60 {
        format!("{}m {:02}s", s / 60, s % 60)
    } else {
        format!("{s}s")
    }
}

fn agent(ui: &mut Ui, t: &Tokens, node: &AgentNode, path: &str, depth: usize) {
    let (state, icon, colour) = state_look(&node.state, t);
    let name = if depth == 0 { tr("Builder (Claude Code)") } else { node.kind.clone() };
    // A subagent's name and its task stay visible while the row is folded.
    let task: String = node.description.chars().take(60).collect();
    let mut head = if depth > 0 && !task.is_empty() { format!("{name} — {task} · {state}") } else { format!("{name} · {state}") };
    if let Some(ms) = node.duration_ms {
        head.push_str(&format!(" · {}", duration(ms)));
    }
    ui.horizontal(|ui| {
        icons::show(ui, if depth == 0 { "terminal" } else { "bot" }, theme::icon::SM, t.icon);
        icons::show(ui, icon, theme::icon::SM, colour);
        let id = format!("{path}/{}{}", node.kind, node.started.map(|s| s.timestamp_millis()).unwrap_or_default());
        egui::CollapsingHeader::new(RichText::new(head).color(t.text))
            .id_salt(("team-agent", id.clone()))
            .default_open(depth == 0 || node.state.is_live())
            .icon(widgets::disclosure_icon)
            .show(ui, |ui| {
                ui.vertical(|ui| details(ui, t, node, &id, depth));
            });
    });
}

fn details(ui: &mut Ui, t: &Tokens, node: &AgentNode, id: &str, depth: usize) {
    let dim = |ui: &mut Ui, text: String| ui.label(RichText::new(text).color(t.text_dim).size(theme::CAPTION));
    if !node.description.is_empty() {
        ui.label(RichText::new(&node.description).color(t.text));
    }
    let mut facts = Vec::new();
    if !node.model.is_empty() {
        facts.push(trf("model {m}", &[("m", &node.model)]));
    }
    if let Some(s) = node.started {
        facts.push(trf("started {t} UTC", &[("t", &s.format("%H:%M:%S").to_string())]));
    }
    if let Some(n) = node.tokens {
        facts.push(trf("{n} tokens", &[("n", &n.to_string())]));
    }
    if let Some(n) = node.tool_uses {
        facts.push(trf("{n} tool call(s)", &[("n", &n.to_string())]));
    }
    dim(ui, facts.join(" · "));
    if let Some(s) = &node.summary {
        dim(ui, trf("Answered: {s}", &[("s", s)]));
    }
    if !node.tools.is_empty() {
        let groups = [
            (ToolGroup::Craft, "puzzle", tr("Craft tools")),
            (ToolGroup::Skill, "wand-sparkles", tr("Skills")),
            (ToolGroup::Agent, "workflow", tr("Subagents started")),
            (ToolGroup::Mcp, "cable", tr("Other MCP tools")),
            (ToolGroup::Builtin, "wrench", tr("Claude Code tools")),
        ];
        for (group, icon, title) in groups {
            let calls: Vec<String> = node.tools.iter().filter(|c| c.group == group).map(|c| format!("{} ×{}", c.name, c.count)).collect();
            if calls.is_empty() {
                continue;
            }
            ui.horizontal_wrapped(|ui| {
                icons::show(ui, icon, theme::icon::XS, t.text_faint);
                ui.label(RichText::new(format!("{title}:")).color(t.text_dim).size(theme::CAPTION));
                ui.label(RichText::new(calls.join(", ")).font(theme::mono(theme::CODE)).color(t.text));
            });
        }
        if !node.recent.is_empty() {
            dim(ui, trf("Latest: {calls}", &[("calls", &node.recent.join(" → "))]));
        }
    }
    for child in &node.children {
        agent(ui, t, child, id, depth + 1);
    }
}

fn discord(ui: &mut Ui, t: &Tokens, bot: &BotLink) {
    widgets::card(t).inner_margin(theme::pad::TIGHT).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            icons::show(ui, "message-square", theme::icon::SM, t.icon);
            ui.label(RichText::new(tr("Discord bot")).font(theme::semibold(theme::BODY)));
        });
        ui.label(
            RichText::new(tr("The bot reads this project's files and posts what changed. No agent posts to Discord.")).color(t.text_faint).size(theme::CAPTION),
        );
        let (text, colour) = match bot {
            BotLink::Port { slug, label, runs, cards } => (
                trf(
                    "A port the bot made ({label}, channel port-{slug}): {runs} run(s), {cards} tester card(s).",
                    &[("label", label), ("slug", slug), ("runs", &runs.to_string()), ("cards", &cards.to_string())],
                ),
                t.success,
            ),
            BotLink::Mirror { slug, status, features, checks, first_look_posted } => (
                trf(
                    "Mirrored in port-{slug}: stage {status}, {features} feature(s) and {checks} check(s) reported, first look {look}.",
                    &[
                        ("slug", slug),
                        ("status", status.as_deref().unwrap_or("?")),
                        ("features", &features.to_string()),
                        ("checks", &checks.to_string()),
                        ("look", &if *first_look_posted { tr("posted") } else { tr("not posted yet") }),
                    ],
                ),
                t.success,
            ),
            BotLink::Waiting { file } => (trf("Offered to the bot ({file}); it hasn't picked it up yet.", &[("file", &file.display().to_string())]), t.warning),
            BotLink::None { why } => (trf("Not followed by the bot. {why}", &[("why", why)]), t.text_dim),
            BotLink::Unreadable { why } => (trf("Can't tell: {why}", &[("why", why)]), t.danger),
        };
        ui.add_space(theme::space::XS);
        ui.add(egui::Label::new(RichText::new(text).color(colour)).wrap());
    });
}
