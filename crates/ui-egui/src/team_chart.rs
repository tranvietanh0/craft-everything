//! The Team tab's org chart: the builder's card on top, one card per role on the project's team
//! below it, joined by lines, and a card for helpers that aren't a role. A card lights up while a
//! subagent works as its role. Cards take keyboard focus; Enter or a click selects one.

use crate::i18n::{tr, trf};
use crate::team::state_look;
use crate::theme::{self, Tokens};
use crate::{icons, widgets};
use craft_anything_ai::team::agent_files::FileOwner;
use craft_anything_ai::team::{AgentNode, Team};
use craft_anything_engine::team::Role;
use egui::{Color32, Rect, Sense, Stroke, Ui, Vec2, pos2};
use std::collections::BTreeMap;

/// A card's size and the room between cards and rows (the lines run through it).
const CARD_W: f32 = 210.0;
const CARD_H: f32 = 118.0;
const GAP: f32 = theme::space::MD;
const LINK_H: f32 = theme::space::XXL;
const LINE_W: f32 = 1.5;

/// Which card is selected.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub enum Card {
    #[default]
    Builder,
    Role(String),
    Others,
}

/// What one card shows.
pub struct CardView {
    pub card: Card,
    pub title: String,
    /// Under the title, in the code font: a role's id.
    pub tag: String,
    /// The state in words, its icon and colour (the words carry it).
    pub state: (String, &'static str, Color32),
    /// Up to three more lines: the task, the current tool, counts.
    pub lines: Vec<String>,
    /// Someone is working as this card now.
    pub live: bool,
}

/// The chart: `builder` on top, `cards` below. Returns the card picked this frame.
pub fn chart(ui: &mut Ui, t: &Tokens, builder: &CardView, cards: &[CardView], selected: &Card) -> Option<Card> {
    let width = ui.available_width().max(CARD_W);
    let cols = (((width + GAP) / (CARD_W + GAP)).floor() as usize).max(1);
    let rows = cards.len().div_ceil(cols);
    let height = CARD_H + rows as f32 * (LINK_H + CARD_H);
    let (area, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let top = Rect::from_min_size(pos2(area.center().x - CARD_W / 2.0, area.top()), Vec2::new(CARD_W, CARD_H));
    let mut rects = Vec::with_capacity(cards.len());
    for (row, chunk) in cards.chunks(cols).enumerate() {
        let row_w = chunk.len() as f32 * CARD_W + (chunk.len() as f32 - 1.0) * GAP;
        let y = area.top() + CARD_H + LINK_H + row as f32 * (CARD_H + LINK_H);
        for i in 0..chunk.len() {
            let x = area.center().x - row_w / 2.0 + i as f32 * (CARD_W + GAP);
            rects.push(Rect::from_min_size(pos2(x, y), Vec2::new(CARD_W, CARD_H)));
        }
    }
    links(ui, t, top, &rects, cards);
    let mut picked = None;
    for (view, rect) in std::iter::once((builder, top)).chain(cards.iter().zip(rects)) {
        if card(ui, t, view, rect, *selected == view.card) {
            picked = Some(view.card.clone());
        }
    }
    picked
}

/// The lines: a trunk down from the builder, a bar over each row, and a drop to each card. A line
/// to a card someone works as now is drawn in the accent colour.
fn links(ui: &Ui, t: &Tokens, top: Rect, rects: &[Rect], cards: &[CardView]) {
    let Some(last) = rects.last() else { return };
    let p = ui.painter();
    let quiet = Stroke::new(LINE_W, t.field_border);
    let trunk_x = top.center().x;
    p.line_segment([top.center_bottom(), pos2(trunk_x, last.top() - LINK_H / 2.0)], quiet);
    let mut row_start = 0;
    while row_start < rects.len() {
        let y = rects[row_start].top();
        let row_end = rects[row_start..].iter().position(|r| r.top() != y).map_or(rects.len(), |n| row_start + n);
        let bar_y = y - LINK_H / 2.0;
        let left = rects[row_start].center().x.min(trunk_x);
        let right = rects[row_end - 1].center().x.max(trunk_x);
        p.line_segment([pos2(left, bar_y), pos2(right, bar_y)], quiet);
        for (rect, view) in rects[row_start..row_end].iter().zip(&cards[row_start..row_end]) {
            let stroke = if view.live { Stroke::new(LINE_W * 1.5, t.accent) } else { quiet };
            p.line_segment([pos2(rect.center().x, bar_y), rect.center_top()], stroke);
        }
        row_start = row_end;
    }
}

/// One card. True when it was clicked, or Enter or Space was pressed on it.
fn card(ui: &mut Ui, t: &Tokens, view: &CardView, rect: Rect, selected: bool) -> bool {
    let id = ui.id().with(("team-card", &view.card));
    let resp = ui.interact(rect, id, Sense::click());
    let h = widgets::hover_t(ui, &resp);
    let edge = if selected {
        Stroke::new(2.0, t.accent_ink)
    } else if view.live {
        Stroke::new(1.5, t.accent)
    } else {
        Stroke::new(1.0, theme::mix(t.card_border, t.field_border, h))
    };
    let fill = if selected { theme::mix(t.card, t.accent_soft, 0.6) } else { theme::mix(t.card, t.hover, h) };
    ui.painter().rect(rect, t.radius_lg, fill, edge, egui::StrokeKind::Inside);
    let inner = rect.shrink(theme::space::MD);
    let p = ui.painter().with_clip_rect(rect.shrink(theme::space::XS));
    let text_w = inner.width() - theme::icon::SM - theme::space::XS;
    let fit = |text: &str, font: egui::FontId, colour: Color32, w: f32| {
        egui::WidgetText::from(egui::RichText::new(text).font(font).color(colour)).into_galley(
            ui,
            Some(egui::TextWrapMode::Truncate),
            w,
            egui::FontSelection::Default,
        )
    };
    let mut y = inner.top();
    let title = fit(&view.title, theme::semibold(theme::SMALL), t.text, text_w);
    if selected {
        icons::paint(ui, Rect::from_min_size(pos2(inner.right() - theme::icon::SM, y), Vec2::splat(theme::icon::SM)), "check", theme::icon::SM, t.accent_ink);
    }
    let title_h = title.size().y;
    p.galley(pos2(inner.left(), y), title, t.text);
    y += title_h;
    if !view.tag.is_empty() {
        let tag = fit(&view.tag, theme::mono(theme::CAPTION), t.text_faint, inner.width());
        let tag_h = tag.size().y;
        p.galley(pos2(inner.left(), y), tag, t.text_faint);
        y += tag_h;
    }
    y += theme::space::XS;
    let (state, icon, colour) = &view.state;
    icons::paint(ui, Rect::from_min_size(pos2(inner.left(), y + 1.0), Vec2::splat(theme::icon::SM)), icon, theme::icon::SM, *colour);
    let state_g = fit(state, egui::FontId::proportional(theme::CAPTION), *colour, text_w);
    let state_h = state_g.size().y;
    p.galley(pos2(inner.left() + theme::icon::SM + theme::space::XS, y), state_g, *colour);
    y += state_h;
    for line in view.lines.iter().take(3) {
        if y >= inner.bottom() {
            break;
        }
        let g = fit(line, egui::FontId::proportional(theme::CAPTION), t.text_dim, inner.width());
        let line_h = g.size().y;
        p.galley(pos2(inner.left(), y), g, t.text_dim);
        y += line_h;
    }
    widgets::focus_ring(ui, &resp, rect);
    // The name starts with the visible label; the state and the lines follow.
    let mut name = view.title.clone();
    for part in std::iter::once(state).chain(&view.lines) {
        name.push_str(", ");
        name.push_str(part);
    }
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, &name));
    let clicked = resp.clicked();
    if clicked {
        resp.request_focus();
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    clicked
}

/// The lines a card shows for the subagents working as one role (newest live one first).
pub fn agent_lines(nodes: &[AgentNode]) -> Vec<String> {
    let Some(node) = nodes.iter().rev().find(|n| n.state.is_live()).or(nodes.last()) else {
        return Vec::new();
    };
    let mut lines = Vec::new();
    if !node.description.is_empty() {
        lines.push(node.description.clone());
    }
    let mut counts = Vec::new();
    if let Some(n) = node.tool_uses {
        counts.push(trf("{n} tool call(s)", &[("n", &n.to_string())]));
    }
    if let Some(n) = node.tokens {
        counts.push(trf("{n} tokens", &[("n", &n.to_string())]));
    }
    if !counts.is_empty() {
        lines.push(counts.join(" · "));
    }
    if nodes.len() > 1 {
        lines.push(trf("Started {n} times in this conversation", &[("n", &nodes.len().to_string())]));
    }
    lines
}

/// "Not working as this role yet", for a role no subagent has worked as.
fn idle_state(t: &Tokens) -> (String, &'static str, Color32) {
    (tr("Not started in this conversation"), "minus", t.text_faint)
}

/// The builder's card, from the conversation shown (none: not started).
pub fn builder_card(t: &Tokens, team: Option<&Team>) -> CardView {
    let Some(team) = team else {
        return CardView {
            card: Card::Builder,
            title: tr("Builder (Claude Code)"),
            tag: String::new(),
            state: (tr("No conversation yet"), "bot", t.text_faint),
            lines: Vec::new(),
            live: false,
        };
    };
    let b = &team.builder;
    let mut lines = vec![trf("{n} agent(s), {live} working", &[("n", &b.count().to_string()), ("live", &b.live().to_string())])];
    lines.extend(agent_lines(std::slice::from_ref(b)).into_iter().filter(|l| *l != b.description));
    CardView { card: Card::Builder, title: tr("Builder (Claude Code)"), tag: String::new(), state: state_look(&b.state, t), lines, live: b.state.is_live() }
}

/// A role's card: lit while a subagent works as it; otherwise where its subagent file stands.
pub fn role_card(t: &Tokens, role: &Role, nodes: &[AgentNode], file: &str) -> CardView {
    let working = nodes.iter().rev().find(|n| n.state.is_live()).or(nodes.last());
    let (state, lines) = match working {
        Some(node) => (state_look(&node.state, t), agent_lines(nodes)),
        None => (idle_state(t), vec![file.to_string()]),
    };
    CardView {
        card: Card::Role(role.id.clone()),
        title: role.name.clone(),
        tag: role.id.clone(),
        state,
        lines,
        live: nodes.iter().any(|n| n.state.is_live()),
    }
}

/// The card for subagents that aren't a role.
pub fn others_card(t: &Tokens, others: &[AgentNode]) -> CardView {
    let live = others.iter().filter(|n| n.state.is_live()).count();
    let mut kinds: Vec<&str> = others.iter().map(|n| n.kind.as_str()).collect();
    kinds.dedup();
    let state = if live > 0 {
        (trf("{n} working", &[("n", &live.to_string())]), "loader", t.accent)
    } else {
        (trf("{n} subagent(s), none working", &[("n", &others.len().to_string())]), "check", t.text_dim)
    };
    CardView { card: Card::Others, title: tr("Other helpers"), tag: String::new(), state, lines: vec![kinds.join(", ")], live: live > 0 }
}

/// Where a role's subagent file stands, in words.
pub fn file_line(clean_room: bool, files: &Result<BTreeMap<String, FileOwner>, String>, id: &str) -> String {
    if clean_room {
        return tr("Not written in Clean room");
    }
    match files {
        Err(e) => trf("Can't read .claude/agents: {e}", &[("e", e)]),
        Ok(files) => match files.get(id) {
            Some(FileOwner::Craft) => tr("Subagent file written by Craft Anything"),
            Some(FileOwner::Person) => tr("Your own subagent file of this name is used; Craft Anything leaves it alone"),
            None => tr("Written when the next Deep build with Claude Code starts"),
        },
    }
}
