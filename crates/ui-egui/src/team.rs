//! The project's Team tab: who works on the build. Craft Anything starts the builder (Claude
//! Code), the builder starts subagents (and they theirs), and each calls tools; the Discord bot
//! follows the project by reading its files. Read from Claude Code's transcripts by a worker
//! thread every 1.5 s while the tab is shown, so drawing never waits on a file.
//!
//! The project's porting team is drawn as an org chart (`team_chart`): the builder over one card
//! per role, lit while a subagent works as that role. The team is edited in `team_editor`.

use crate::i18n::{tr, trf};
use crate::team_chart::{self, Card, CardView};
use crate::team_editor::{self, TeamAction, TeamEditor};
use crate::theme::{self, Tokens};
use crate::{icons, widgets};
use craft_anything_ai::team::agent_files::{self, FileOwner};
use craft_anything_ai::team::discord::{BotLink, bot_link};
use craft_anything_ai::team::{AgentNode, AgentState, Session, Team, TeamWatch, ToolGroup, by_role, sessions, skill_names, transcript_dirs};
use craft_anything_engine::team::Role;
use craft_anything_platform::sync::Mutex;
use egui::{RichText, Ui};
use std::collections::BTreeMap;
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

struct Shared {
    viewed: Option<Instant>,
    /// The session the person picked; `None` follows the newest.
    chosen: Option<String>,
    dirs: Vec<PathBuf>,
    sessions: Vec<Session>,
    team: View,
    bot: Option<BotLink>,
    /// The subagent files in the project's `.claude/agents`, and whose each is.
    files: Result<BTreeMap<String, FileOwner>, String>,
    running: bool,
}

impl Default for Shared {
    fn default() -> Self {
        Shared {
            viewed: None,
            chosen: None,
            dirs: Vec::new(),
            sessions: Vec::new(),
            team: View::default(),
            bot: None,
            files: Ok(BTreeMap::new()),
            running: false,
        }
    }
}

/// The Team tab of one project.
pub struct TeamPanel {
    root: PathBuf,
    shared: Arc<Mutex<Shared>>,
    selected: Card,
    editor: Option<TeamEditor>,
    /// The job saving a change to the team, while it runs.
    saving: Option<u64>,
}

impl TeamPanel {
    pub fn new(root: PathBuf) -> Self {
        TeamPanel { root, shared: Arc::new(Mutex::new(Shared::default())), selected: Card::Builder, editor: None, saving: None }
    }

    /// A change to the team is being saved by job `job`.
    pub fn saving(&mut self, job: u64) {
        self.saving = Some(job);
    }

    /// Job `job` finished; if it was the save, the editor's buttons work again.
    pub fn saved(&mut self, job: u64) {
        if self.saving == Some(job) {
            self.saving = None;
        }
    }

    /// Open the team editor on `roles` (the "Edit team" button).
    pub fn edit(&mut self, roles: &[Role]) {
        self.editor = Some(TeamEditor::new(roles, skill_names(&self.root)));
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
        let files = agent_files::owners(root).map_err(|e| e.message);
        {
            let mut s = shared.lock().unwrap_or_else(PoisonError::into_inner);
            s.dirs = dirs.clone();
            s.sessions = list;
            s.team = team;
            s.bot = Some(bot);
            s.files = files;
        }
        ctx.request_repaint();
        std::thread::sleep(POLL);
    }
}

/// The project's team as saved, and the mode the app is in.
pub struct Roster<'a> {
    pub roles: &'a [Role],
    /// No team is saved: the standard one applies.
    pub is_default: bool,
    pub clean_room: bool,
}

/// The tab. Returns a change to the team the person asked for in the editor, for the app to save
/// through the `project_team_*` tools.
pub fn show(panel: &mut TeamPanel, ui: &mut Ui, roster: &Roster) -> Option<TeamAction> {
    panel.keep_alive(ui.ctx());
    let t = Tokens::get(ui.ctx());
    let (sessions, view, bot, dirs, chosen, files) = {
        let s = panel.shared.lock().unwrap_or_else(PoisonError::into_inner);
        (s.sessions.clone(), s.team.clone(), s.bot.clone(), s.dirs.clone(), s.chosen.clone(), s.files.clone())
    };
    let team = match &view {
        View::Team(team) => Some(team.as_ref()),
        _ => None,
    };
    let ids: Vec<&str> = roster.roles.iter().map(|r| r.id.as_str()).collect();
    let matched = team.map(|team| by_role(team, &ids)).unwrap_or_default();
    // A card that's gone (a role removed, no helpers in this conversation) leaves the builder selected.
    let gone = match &panel.selected {
        Card::Role(id) => !ids.contains(&id.as_str()),
        Card::Others => matched.others.is_empty(),
        Card::Builder => false,
    };
    if gone {
        panel.selected = Card::Builder;
    }
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.label(RichText::new(tr("Who works on this build, and the tools each one called.")).color(t.text_dim));
        ui.add_space(theme::space::XS);
        mode_note(ui, &t, roster.clean_room);
        ui.add_space(theme::space::SM);
        ui.horizontal(|ui| {
            if !sessions.is_empty() {
                session_picker(panel, ui, &sessions, chosen.as_deref());
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::secondary(ui, Some("settings"), "Edit team").clicked() {
                    panel.edit(roster.roles);
                }
            });
        });
        ui.add_space(theme::space::SM);
        match &view {
            View::Reading => widgets::working(ui, &tr("Reading the builder's conversation…"), None),
            View::Failed(e) => {
                ui.label(RichText::new(tr("The builder's conversation can't be read.")).color(t.danger));
                ui.label(RichText::new(e).color(t.danger).size(theme::CAPTION));
            }
            View::NoSession => {
                let looked: Vec<String> = dirs.iter().map(|d| d.display().to_string()).collect();
                ui.label(RichText::new(tr("No builder conversation yet. It shows here once a build with Claude Code starts.")).color(t.text_dim));
                ui.label(RichText::new(trf("Looked in: {dirs}", &[("dirs", &looked.join(", "))])).color(t.text_faint).size(theme::CAPTION));
            }
            View::Team(_) => {}
        }
        ui.add_space(theme::space::SM);
        let builder = team_chart::builder_card(&t, team);
        let mut cards: Vec<CardView> = roster
            .roles
            .iter()
            .map(|role| {
                let nodes = matched.by_role.get(&role.id).map(Vec::as_slice).unwrap_or_default();
                team_chart::role_card(&t, role, nodes, &team_chart::file_line(roster.clean_room, &files, &role.id))
            })
            .collect();
        if !matched.others.is_empty() {
            cards.push(team_chart::others_card(&t, &matched.others));
        }
        if let Some(card) = team_chart::chart(ui, &t, &builder, &cards, &panel.selected) {
            panel.selected = card;
        }
        ui.add_space(theme::space::MD);
        selected_details(ui, &t, &panel.selected, team, roster, &matched, &files);
        if let Some(team) = team {
            for p in &team.problems {
                ui.label(RichText::new(p).color(t.warning).size(theme::CAPTION));
            }
        }
        ui.add_space(theme::space::MD);
        if let Some(bot) = &bot {
            discord(ui, &t, bot);
        }
    });
    team_editor::show(ui.ctx(), &mut panel.editor, roster.roles, roster.is_default, panel.saving.is_some())
}

/// Where the team applies, said once at the top: only Deep mode gives the builder its team.
fn mode_note(ui: &mut Ui, t: &Tokens, clean_room: bool) {
    let (icon, colour, text) = if clean_room {
        (
            "shield",
            t.warning,
            "The team applies only in Deep mode. In Clean room the builder works alone: it has no Agent tool, and Craft Anything writes no subagent files.",
        )
    } else {
        (
            "info",
            t.text_faint,
            "When a Claude Code build starts on this computer in Deep mode, each role is written to .claude/agents in the project and the builder hands work to them. Claude Code in herdr reads the same folder. Other coding agents, and Claude Code over SSH, don't get the team.",
        )
    };
    ui.horizontal_top(|ui| {
        icons::show(ui, icon, theme::icon::SM, colour);
        ui.add(egui::Label::new(RichText::new(tr(text)).color(colour).size(theme::CAPTION)).wrap());
    });
}

/// What the selected card is about: the builder's own tree, a role and the subagents that worked
/// as it, or the other helpers.
fn selected_details(
    ui: &mut Ui,
    t: &Tokens,
    selected: &Card,
    team: Option<&Team>,
    roster: &Roster,
    matched: &craft_anything_ai::team::RoleMatch,
    files: &Result<BTreeMap<String, FileOwner>, String>,
) {
    widgets::card(t).inner_margin(theme::pad::TIGHT).show(ui, |ui| {
        ui.set_width(ui.available_width());
        match selected {
            Card::Builder => match team {
                Some(team) => {
                    let header = trf(
                        "Craft Anything (orchestrator): {n} agent(s), {live} working",
                        &[("n", &team.builder.count().to_string()), ("live", &team.builder.live().to_string())],
                    );
                    egui::CollapsingHeader::new(RichText::new(header).font(theme::semibold(theme::BODY)))
                        .id_salt(("team-root", &team.session))
                        .default_open(true)
                        .icon(widgets::disclosure_icon)
                        .show(ui, |ui| agent(ui, t, &team.builder, &team.session, 0));
                }
                None => {
                    ui.label(RichText::new(tr("The builder hasn't started in this project yet.")).color(t.text_dim));
                }
            },
            Card::Role(id) => {
                let Some(role) = roster.roles.iter().find(|r| &r.id == id) else { return };
                role_details(ui, t, role, &team_chart::file_line(roster.clean_room, files, id));
                let nodes = matched.by_role.get(id).map(Vec::as_slice).unwrap_or_default();
                if nodes.is_empty() {
                    ui.label(RichText::new(tr("No subagent has worked as this role in this conversation yet.")).color(t.text_dim));
                }
                let path = team.map(|t| t.session.as_str()).unwrap_or_default();
                for node in nodes {
                    agent(ui, t, node, path, 1);
                }
            }
            Card::Others => {
                ui.label(RichText::new(tr("Other helpers")).font(theme::semibold(theme::BODY)));
                ui.label(
                    RichText::new(tr("Subagents that aren't a role on the team: Claude Code's own helpers, or a role renamed or removed since."))
                        .color(t.text_dim)
                        .size(theme::CAPTION),
                );
                let path = team.map(|t| t.session.as_str()).unwrap_or_default();
                for node in &matched.others {
                    agent(ui, t, node, path, 1);
                }
            }
        }
    });
}

/// A role as saved: what it's for and how it's set up.
fn role_details(ui: &mut Ui, t: &Tokens, role: &Role, file: &str) {
    ui.label(RichText::new(&role.name).font(theme::semibold(theme::BODY)));
    ui.label(RichText::new(&role.id).font(theme::mono(theme::CODE)).color(t.text_faint));
    ui.label(RichText::new(&role.description).color(t.text));
    let mut facts = vec![
        trf("model {m}", &[("m", role.model.as_deref().unwrap_or(&tr("the builder's")))]),
        match &role.tools {
            Some(tools) => trf("tools {list}", &[("list", &tools.join(", "))]),
            None => tr("the builder's tools"),
        },
    ];
    if !role.skills.is_empty() {
        facts.push(trf("skills {list}", &[("list", &role.skills.join(", "))]));
    }
    if !role.hooks.is_empty() {
        facts.push(trf("{n} hook(s), run only in a folder trusted in Claude Code", &[("n", &role.hooks.len().to_string())]));
    }
    ui.label(RichText::new(facts.join(" · ")).color(t.text_dim).size(theme::CAPTION));
    ui.label(RichText::new(file).color(t.text_dim).size(theme::CAPTION));
    ui.add_space(theme::space::SM);
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
pub(crate) fn state_look(state: &AgentState, t: &Tokens) -> (String, &'static str, egui::Color32) {
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

/// The `project_team_*` tools the editor's buttons run.
pub(crate) const TEAM_TOOLS: [&str; 3] = ["project_team_set_role", "project_team_remove_role", "project_team_reset"];

impl crate::CraftApp {
    /// Save a change to `root`'s team through its tool, as an MCP agent or the CLI would.
    pub(crate) fn change_team(&mut self, root: &Path, action: TeamAction) {
        let (tool, params, title) = match action {
            TeamAction::SetRole { role, replaces } => {
                (TEAM_TOOLS[0], serde_json::json!({"project": root, "role": role, "replaces": replaces}), tr("Save a team role"))
            }
            TeamAction::RemoveRole(id) => (TEAM_TOOLS[1], serde_json::json!({"project": root, "id": id}), tr("Remove a team role")),
            TeamAction::Reset => (TEAM_TOOLS[2], serde_json::json!({"project": root}), tr("Reset the team")),
        };
        let job = self.act(tool, params, title);
        if let Some(panel) = self.team.as_mut() {
            panel.saving(job);
        }
    }

    /// A team tool finished: show what it saved, or why not.
    pub(crate) fn team_changed(&mut self, job: u64, tool: &str, result: Result<serde_json::Value, craft_anything_engine::evidence::ToolError>) {
        if let Some(panel) = self.team.as_mut() {
            panel.saved(job);
        }
        match result {
            Ok(v) => {
                // What the tool saved (the person's other changes on disk come along).
                if let Some(ws) = self.workspace.as_mut().filter(|w| v["project"].as_str().is_some_and(|p| w.root == Path::new(p))) {
                    let _ = ws.reload();
                }
                let done = match tool {
                    "project_team_remove_role" => "Role removed. The next Deep build uses the team as it is now.",
                    "project_team_reset" => "The team is the standard one again.",
                    _ => "Role saved. The next Deep build uses the team as it is now.",
                };
                self.toast(tr(done), false);
            }
            Err(e) => self.toast(format!("{} {}", tr("Couldn't change the team:"), e.for_people()), true),
        }
    }
}
