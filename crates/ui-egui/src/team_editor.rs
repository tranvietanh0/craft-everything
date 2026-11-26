//! The team editor: a window listing the project's roles, with a form for the one picked. It
//! changes nothing itself; Save, Remove and Reset hand a [`TeamAction`] back, which the app runs
//! as a `project_team_*` tool, and what the tool saved comes back as the roster.

use crate::i18n::{tr, trf};
use crate::theme::{self, Tokens};
use crate::widgets::{self, Button, ButtonKind};
use craft_anything_engine::team::{Hook, MAX_ROLES, Role};
use egui::{RichText, Ui};

/// What the editor asks the app to save, through the `project_team_*` tools.
#[derive(Debug, Clone, PartialEq)]
pub enum TeamAction {
    /// `project_team_set_role`.
    SetRole { role: Role, replaces: Option<String> },
    /// `project_team_remove_role`.
    RemoveRole(String),
    /// `project_team_reset`.
    Reset,
}

/// Hook events offered in the list; one saved with another name is kept and shown too.
const EVENTS: [&str; 3] = ["PreToolUse", "PostToolUse", "Stop"];

/// One role as the form holds it while it's edited.
#[derive(Debug, Clone, PartialEq)]
struct Draft {
    /// The id it's saved under; none for a role not saved yet.
    saved: Option<String>,
    id: String,
    name: String,
    description: String,
    prompt: String,
    model: String,
    /// Comma-separated; empty for the default set.
    tools: String,
    skills: Vec<String>,
    hooks: Vec<Hook>,
}

impl Draft {
    fn of(role: &Role) -> Draft {
        Draft {
            saved: Some(role.id.clone()),
            id: role.id.clone(),
            name: role.name.clone(),
            description: role.description.clone(),
            prompt: role.prompt.clone(),
            model: role.model.clone().unwrap_or_default(),
            tools: role.tools.as_ref().map(|t| t.join(", ")).unwrap_or_default(),
            skills: role.skills.clone(),
            hooks: role.hooks.clone(),
        }
    }

    fn role(&self) -> Role {
        let tools: Vec<String> = self.tools.split(',').map(str::trim).filter(|t| !t.is_empty()).map(str::to_string).collect();
        Role {
            id: self.id.trim().to_string(),
            name: self.name.trim().to_string(),
            description: self.description.trim().to_string(),
            prompt: self.prompt.clone(),
            model: Some(self.model.trim().to_string()).filter(|m| !m.is_empty()),
            tools: (!tools.is_empty()).then_some(tools),
            skills: self.skills.clone(),
            hooks: self.hooks.clone(),
        }
    }

    /// Changed since it was last saved (`base` is the saved team).
    fn unsaved(&self, base: &[Role]) -> bool {
        match &self.saved {
            None => true,
            Some(id) => base.iter().find(|r| &r.id == id).is_none_or(|r| *r != self.role()),
        }
    }
}

/// The editor's state while its window is open.
pub struct TeamEditor {
    /// The team as last saved, which the drafts are compared with.
    base: Vec<Role>,
    drafts: Vec<Draft>,
    picked: usize,
    /// Skill folder names found when the editor opened.
    known_skills: Vec<String>,
    skill_text: String,
    /// Asking to confirm a reset.
    confirm_reset: bool,
    /// Move the keyboard focus to the name field on the next frame.
    focus_name: bool,
}

impl TeamEditor {
    pub fn new(roster: &[Role], known_skills: Vec<String>) -> Self {
        TeamEditor {
            base: roster.to_vec(),
            drafts: roster.iter().map(Draft::of).collect(),
            picked: 0,
            known_skills,
            skill_text: String::new(),
            confirm_reset: false,
            focus_name: true,
        }
    }

    /// Take the team as now saved. Roles with unsaved edits keep them; the rest show what's saved.
    fn sync(&mut self, roster: &[Role]) {
        if roster == self.base.as_slice() {
            return;
        }
        let picked_id = self.drafts.get(self.picked).map(|d| d.id.clone());
        let old = std::mem::replace(&mut self.base, roster.to_vec());
        let mut next: Vec<Draft> = roster
            .iter()
            .map(|role| {
                let kept = self.drafts.iter().find(|d| d.saved.as_deref() == Some(role.id.as_str()) && d.unsaved(&old) && d.role() != *role);
                kept.cloned().unwrap_or_else(|| Draft::of(role))
            })
            .collect();
        next.extend(self.drafts.iter().filter(|d| d.saved.is_none() && !roster.iter().any(|r| r.id == d.id.trim())).cloned());
        self.drafts = next;
        self.picked = picked_id.and_then(|id| self.drafts.iter().position(|d| d.id == id)).unwrap_or(0);
    }
}

/// The editor's window, while `editor` is open. `saving`: a change is being saved, so the buttons
/// that save wait for it.
pub fn show(ctx: &egui::Context, editor: &mut Option<TeamEditor>, roster: &[Role], is_default: bool, saving: bool) -> Option<TeamAction> {
    let ed = editor.as_mut()?;
    ed.sync(roster);
    let t = Tokens::get(ctx);
    let mut open = true;
    let mut action = None;
    egui::Window::new(RichText::new(tr("Edit team")).font(theme::semibold(theme::BODY)))
        .id(egui::Id::new("team-editor"))
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_size([760.0, 560.0])
        .show(ctx, |ui| {
            ui.label(
                RichText::new(tr(if is_default {
                    "The standard team. Saving a change keeps the whole team in this project's craft.json."
                } else {
                    "This project's own team, kept in its craft.json."
                }))
                .color(t.text_dim)
                .size(theme::CAPTION),
            );
            ui.add_space(theme::space::SM);
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(180.0);
                    role_list(ui, &t, ed, saving, &mut action);
                });
                ui.separator();
                ui.vertical(|ui| {
                    egui::ScrollArea::vertical().id_salt("team-editor-form").auto_shrink([false, false]).show(ui, |ui| {
                        if ed.drafts.is_empty() {
                            ui.label(RichText::new(tr("No roles. Add one, or reset to the standard team.")).color(t.text_dim));
                        } else {
                            form(ui, &t, ed, saving, &mut action);
                        }
                    });
                });
            });
        });
    if !open {
        *editor = None;
    }
    action
}

fn role_list(ui: &mut Ui, t: &Tokens, ed: &mut TeamEditor, saving: bool, action: &mut Option<TeamAction>) {
    widgets::caption(ui, "Roles");
    for i in 0..ed.drafts.len() {
        let d = &ed.drafts[i];
        let unsaved = d.unsaved(&ed.base);
        let label = if unsaved { trf("{name} (not saved)", &[("name", &d.name)]) } else { d.name.clone() };
        let r = ui.selectable_label(ed.picked == i, label);
        if r.clicked() {
            ed.picked = i;
            ed.focus_name = true;
        }
    }
    ui.add_space(theme::space::SM);
    let room = ed.drafts.len() < MAX_ROLES;
    if ui.add(Button::new(ButtonKind::Secondary, "Add role").icon(Some("plus")).enabled(room)).clicked() {
        let n = (1..).find(|n| !ed.drafts.iter().any(|d| d.id == format!("role-{n}"))).unwrap_or(1);
        ed.drafts.push(Draft {
            saved: None,
            id: format!("role-{n}"),
            name: tr("New role"),
            description: String::new(),
            prompt: String::new(),
            model: String::new(),
            tools: String::new(),
            skills: Vec::new(),
            hooks: Vec::new(),
        });
        ed.picked = ed.drafts.len() - 1;
        ed.focus_name = true;
    }
    if !room {
        ui.label(RichText::new(trf("A team has at most {n} roles.", &[("n", &MAX_ROLES.to_string())])).color(t.text_faint).size(theme::CAPTION));
    }
    ui.add_space(theme::space::MD);
    if ed.confirm_reset {
        ui.label(RichText::new(tr("Go back to the standard five roles? Your changes to the team are lost.")).color(t.warning).size(theme::CAPTION));
        ui.horizontal_wrapped(|ui| {
            if ui.add(Button::new(ButtonKind::Danger, "Reset").enabled(!saving)).clicked() {
                *action = Some(TeamAction::Reset);
                ed.confirm_reset = false;
            }
            if widgets::ghost(ui, None, "Cancel").clicked() {
                ed.confirm_reset = false;
            }
        });
    } else if ui.add(Button::new(ButtonKind::Ghost, "Reset to the standard team").icon(Some("refresh-cw")).enabled(!saving)).clicked() {
        ed.confirm_reset = true;
    }
}

/// A one-line field with a label above it whose words name the field.
fn line_field(ui: &mut Ui, t: &Tokens, label: &str, hint: &str, value: &mut String) -> egui::Response {
    widgets::caption(ui, label);
    let r = ui.add(egui::TextEdit::singleline(value).hint_text(tr(hint)).desired_width(f32::INFINITY).frame(widgets::text_field_frame_compact(t)));
    widgets::named(r, label)
}

fn text_field(ui: &mut Ui, t: &Tokens, label: &str, rows: usize, code: bool, value: &mut String) {
    widgets::caption(ui, label);
    let mut edit = egui::TextEdit::multiline(value).desired_rows(rows).desired_width(f32::INFINITY).frame(widgets::text_field_frame_compact(t));
    if code {
        edit = edit.font(theme::mono(theme::CODE));
    }
    widgets::named(ui.add(edit), label);
}

fn form(ui: &mut Ui, t: &Tokens, ed: &mut TeamEditor, saving: bool, action: &mut Option<TeamAction>) {
    let picked = ed.picked.min(ed.drafts.len() - 1);
    let focus = std::mem::take(&mut ed.focus_name);
    let known = ed.known_skills.clone();
    let d = &mut ed.drafts[picked];
    let name = line_field(ui, t, "Name", "What people call the role", &mut d.name);
    if focus {
        name.request_focus();
    }
    line_field(ui, t, "Id", "lower-case-with-hyphens", &mut d.id);
    ui.label(
        RichText::new(tr("The subagent's name. The builder starts the role by it, and the Team tab matches it.")).color(t.text_faint).size(theme::CAPTION),
    );
    text_field(ui, t, "When to use it", 2, false, &mut d.description);
    text_field(ui, t, "Instructions", 8, true, &mut d.prompt);
    line_field(ui, t, "Model", "Empty: the builder's own (or sonnet, opus, haiku, inherit)", &mut d.model);
    line_field(ui, t, "Tools", "Empty: the builder's tools. Or a list: Read, Grep, Bash", &mut d.tools);
    ui.add_space(theme::space::SM);
    skills(ui, t, d, &known, &mut ed.skill_text);
    ui.add_space(theme::space::SM);
    hooks(ui, t, d);
    ui.add_space(theme::space::MD);
    let role = d.role();
    let problem = role.validate().err().map(|e| e.message);
    if let Some(p) = &problem {
        ui.label(RichText::new(p).color(t.danger));
    }
    let base = ed.base.clone();
    let unsaved = ed.drafts[picked].unsaved(&base);
    let saved = ed.drafts[picked].saved.clone();
    ui.horizontal_wrapped(|ui| {
        let can_save = unsaved && problem.is_none() && !saving;
        if ui.add(Button::new(ButtonKind::Primary, "Save role").enabled(can_save)).clicked() {
            let replaces = saved.clone().filter(|s| *s != role.id);
            *action = Some(TeamAction::SetRole { role: role.clone(), replaces });
        }
        let remove = Button::new(ButtonKind::Secondary, if saved.is_some() { "Remove role" } else { "Discard" }).icon(Some("trash-2"));
        if ui.add(remove.enabled(!saving)).clicked() {
            match &saved {
                Some(id) => *action = Some(TeamAction::RemoveRole(id.clone())),
                None => {
                    ed.drafts.remove(picked);
                    ed.picked = picked.saturating_sub(1);
                }
            }
        }
        if saving {
            ui.label(RichText::new(tr("Saving…")).color(t.text_dim));
        }
    });
}

/// The role's skills: each with a button to take it off, a list of the skill folders found, and a
/// field for any other name.
fn skills(ui: &mut Ui, t: &Tokens, d: &mut Draft, known: &[String], text: &mut String) {
    widgets::caption(ui, "Skills");
    ui.label(
        RichText::new(tr("Loaded into the role when it starts, from a skill folder of that name in your Claude Code skills or this project's .claude/skills."))
            .color(t.text_faint)
            .size(theme::CAPTION),
    );
    let mut gone = None;
    ui.horizontal_wrapped(|ui| {
        for (i, skill) in d.skills.iter().enumerate() {
            ui.label(RichText::new(skill).font(theme::mono(theme::CODE)).color(t.text));
            let r = ui.add(Button::new(ButtonKind::Ghost, "").icon(Some("x")).size(theme::ControlSize::Small));
            if widgets::named(r, &trf("Remove skill {name}", &[("name", skill)])).clicked() {
                gone = Some(i);
            }
        }
        if d.skills.is_empty() {
            ui.label(RichText::new(tr("None")).color(t.text_faint));
        }
    });
    if let Some(i) = gone {
        d.skills.remove(i);
    }
    ui.horizontal_wrapped(|ui| {
        let offered: Vec<&String> = known.iter().filter(|k| !d.skills.contains(k)).collect();
        let pick = widgets::dropdown("team-skill-pick").selected_text(tr("Add a skill")).show_ui(ui, |ui| {
            let mut chosen = None;
            for k in &offered {
                if widgets::menu_item(ui, false, k.as_str()).clicked() {
                    chosen = Some((*k).clone());
                }
            }
            if offered.is_empty() {
                ui.label(RichText::new(tr("No other skill folders found")).color(t.text_faint));
            }
            chosen
        });
        if let Some(Some(k)) = pick.inner {
            d.skills.push(k);
        }
        let r = ui.add(
            egui::TextEdit::singleline(text).hint_text(tr("Another skill's folder name")).desired_width(180.0).frame(widgets::text_field_frame_compact(t)),
        );
        widgets::named(r, "Another skill's folder name");
        let name = text.trim().to_string();
        let ok = !name.is_empty() && !d.skills.contains(&name);
        if ui.add(Button::new(ButtonKind::Secondary, "Add").enabled(ok).size(theme::ControlSize::Small)).clicked() {
            d.skills.push(name);
            text.clear();
        }
    });
}

/// The role's hooks, one row each: event, which tool calls, the command.
fn hooks(ui: &mut Ui, t: &Tokens, d: &mut Draft) {
    widgets::caption(ui, "Hooks");
    ui.label(
        RichText::new(tr(
            "Claude Code runs a role's hooks only in a folder you trusted in Claude Code. A headless build doesn't count as trusted, so there they may be skipped.",
        ))
        .color(t.text_faint)
        .size(theme::CAPTION),
    );
    let mut gone = None;
    for (i, hook) in d.hooks.iter_mut().enumerate() {
        let mut events: Vec<String> = EVENTS.map(String::from).to_vec();
        if !EVENTS.contains(&hook.event.as_str()) {
            events.push(hook.event.clone());
        }
        ui.horizontal(|ui| {
            let event = widgets::dropdown(("team-hook-event", i)).selected_text(hook.event.clone()).width(130.0).show_ui(ui, |ui| {
                for e in &events {
                    if widgets::menu_item(ui, hook.event == *e, e.as_str()).clicked() {
                        hook.event.clone_from(e);
                    }
                }
            });
            widgets::named(event.response, &trf("Hook {n} event", &[("n", &(i + 1).to_string())]));
            let m = ui.add(
                egui::TextEdit::singleline(&mut hook.matcher)
                    .hint_text(tr("Tools: Bash, Edit|Write"))
                    .desired_width(130.0)
                    .frame(widgets::text_field_frame_compact(t)),
            );
            widgets::named(m, &trf("Hook {n} tools", &[("n", &(i + 1).to_string())]));
            let c = ui.add(
                egui::TextEdit::singleline(&mut hook.command)
                    .hint_text(tr("Command"))
                    .font(theme::mono(theme::CODE))
                    .desired_width((ui.available_width() - 40.0).max(120.0))
                    .frame(widgets::text_field_frame_compact(t)),
            );
            widgets::named(c, &trf("Hook {n} command", &[("n", &(i + 1).to_string())]));
            let r = ui.add(Button::new(ButtonKind::Ghost, "").icon(Some("trash-2")).size(theme::ControlSize::Small));
            if widgets::named(r, &trf("Remove hook {n}", &[("n", &(i + 1).to_string())])).clicked() {
                gone = Some(i);
            }
        });
    }
    if let Some(i) = gone {
        d.hooks.remove(i);
    }
    if widgets::ghost(ui, Some("plus"), "Add hook").clicked() {
        d.hooks.push(Hook { event: EVENTS[0].into(), matcher: String::new(), command: String::new() });
    }
}
