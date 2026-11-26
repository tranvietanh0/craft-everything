//! Tester cards: a `ready_to_test` feature with a guide gets a card in the port's channel (the
//! guide, the full file attached with the pictures it links, the similarity, the tester role
//! mentioned, Pass and Fail). Only members with the tester or porter role may press them. Pass
//! records `passed`; Fail opens a form and records `broken` with what the tester wrote, leaves it as
//! a note for the builder and posts it in the Log thread. Either way the card and the board show the verdict and Plane is told.

use super::Bot;
use super::embeds;
use super::live::{self, Port};
use super::screens::{self, Upload};
use super::sync::SyncJob;
use chrono::Utc;
use craft_anything_discord::board::Board;
use craft_anything_discord::events::Event;
use craft_anything_discord::guide::{self, GuideFile};
use craft_anything_discord::review::{self, STEPS_LIMIT, Verdict, WHAT_LIMIT};
use craft_anything_discord::screens::{MOST_PER_MESSAGE, UPLOAD_LIMIT};
use craft_anything_discord::state::{Card, CloseError};
use craft_anything_discord::tester::{self, CardAction, Press};
use craft_anything_discord::text::clip;
use serenity::all::{
    ActionRowComponent, ButtonStyle, ComponentInteraction, Context, CreateActionRow, CreateAllowedMentions, CreateAttachment, CreateButton, CreateEmbed,
    CreateInputText, CreateInteractionResponse, CreateInteractionResponseFollowup, CreateMessage, CreateModal, EditMessage, Http, InputTextStyle, Member,
    Message, MessageId, ModalInteraction, RoleId, User,
};
use std::sync::Arc;

const FIELD_LIMIT: usize = 1_024;
const MESSAGE_LIMIT: usize = 2_000;
const WHAT_ID: &str = "what";
const STEPS_ID: &str = "steps";
const RESULT_FIELD: &str = "Result";

/// A `feature` event from the run: Plane hears of every change; `broken` starts a new testing
/// round; `ready_to_test` with a guide gets a card unless one already covers this round.
pub async fn on_feature(http: &Http, bot: &Bot, port: &Port, name: &str, state: &str, guide: Option<&str>, similarity: Option<f64>) {
    port.queue(SyncJob::Feature { name: name.to_owned(), state: state.to_owned(), note: None });
    {
        let mut live = port.live.lock().await;
        live.state.record_feature(name, state, Utc::now());
        if state == "broken" {
            live.state.on_broken(name);
        }
        live.save(port);
    }
    if state != "ready_to_test" {
        return;
    }
    match guide {
        Some(guide) => post_card(http, bot, port, name, guide, similarity).await,
        None => log::warn!("{name} in {} is ready to test without a guide; no card is posted", port.channel),
    }
}

async fn post_card(http: &Http, bot: &Bot, port: &Port, feature: &str, guide: &str, similarity: Option<f64>) {
    // Held while posting, so a second report or a press can't see a half-made card.
    let mut live = port.live.lock().await;
    let (action, previous) = live.state.on_ready(feature);
    match (action, previous) {
        (CardAction::Skip, _) => return,
        (CardAction::PostFresh { disable_previous: true }, Some(old)) => {
            let _ = live.state.close_card(old.id, None);
            live.save(port);
            close_message(http, port, &old, None, "↩️ Replaced by a newer card").await;
        }
        _ => {}
    }
    let (project, guide_name) = (port.project.clone(), guide.to_owned());
    let read = tokio::task::spawn_blocking(move || guide::read_guide(&project, &guide_name)).await;
    let read = read.map_err(|e| e.to_string()).and_then(|r| r.map_err(|e| e.to_string()));
    let pictures = linked_pictures(port, guide, &read).await;
    let card = live.state.reserve_card();
    let image = pictures.first().map(|p| p.name.as_str());
    let message = card_message(bot, &live.state.label, feature, guide, similarity, &read, card, image).add_files(pictures.iter().map(Upload::attachment));
    match port.channel.send_message(http, message).await {
        Ok(sent) => {
            live.state.add_card(feature, sent.id.get());
            live.save(port);
        }
        Err(e) => log::warn!("couldn't post the tester card for {feature} in {}: {e}", port.channel),
    }
}

fn buttons(card: u64, disabled: bool) -> Vec<CreateActionRow> {
    vec![CreateActionRow::Buttons(vec![
        CreateButton::new(Press::Pass(card).custom_id()).style(ButtonStyle::Success).label("Pass").disabled(disabled),
        CreateButton::new(Press::Fail(card).custom_id()).style(ButtonStyle::Danger).label("Fail").disabled(disabled),
    ])]
}

/// `image`: the first picture the guide links, attached to the same message, shown in the card.
#[expect(clippy::too_many_arguments, reason = "the card's parts, all shown on it")]
fn card_message(
    bot: &Bot,
    label: &str,
    feature: &str,
    guide: &str,
    similarity: Option<f64>,
    read: &Result<GuideFile, String>,
    card: u64,
    image: Option<&str>,
) -> CreateMessage {
    let testers = RoleId::new(bot.config.tester_role_id);
    let mut embed = CreateEmbed::new()
        .title(tester::card_title(feature))
        .colour(embeds::TESTING)
        .footer(serenity::all::CreateEmbedFooter::new("Pass or Fail: members with the tester or porter role. Follow the guide, then say how it went."));
    embed = embed.field("Port", clip(label, FIELD_LIMIT), true);
    if let Some(s) = similarity {
        embed = embed.field("Similarity", format!("{s:.0}%"), true);
    }
    let mut message = CreateMessage::new()
        .content(format!("<@&{testers}> 🧪 **{}** is ready to test.", clip(feature, 200)))
        .allowed_mentions(CreateAllowedMentions::new().roles(vec![testers]))
        .components(buttons(card, false));
    match read {
        Ok(file) => {
            let (excerpt, cut) = tester::guide_excerpt(&file.text);
            embed = embed.description(excerpt);
            if file.too_large {
                embed =
                    embed.field("Guide", clip(&format!("{guide} is too large to attach; this is its start. It is in the project folder."), FIELD_LIMIT), false);
            } else {
                if cut {
                    embed = embed.field("Guide", "Cut short here: the full guide is attached.", false);
                }
                message = message.add_file(CreateAttachment::bytes(file.text.clone().into_bytes(), file.file_name.clone()));
            }
        }
        Err(e) => {
            embed = embed.description(clip(&format!("⚠️ The guide {guide} couldn't be read: {e}"), FIELD_LIMIT));
        }
    }
    if let Some(name) = image {
        embed = embed.image(format!("attachment://{name}"));
    }
    message.embed(embed)
}

/// Disables a card's buttons and adds its result, keeping the rest of the card.
async fn close_message(http: &Http, port: &Port, card: &Card, shown: Option<&Message>, result: &str) {
    close_message_with(http, port, card, shown, &[(RESULT_FIELD, result.to_owned())]).await;
}

async fn close_message_with(http: &Http, port: &Port, card: &Card, shown: Option<&Message>, fields: &[(&str, String)]) {
    let id = MessageId::new(card.message.max(1));
    let fetched;
    let message = match shown {
        Some(m) => m,
        None => match port.channel.message(http, id).await {
            Ok(m) => {
                fetched = m;
                &fetched
            }
            Err(e) => {
                log::warn!("couldn't read the card for {} in {}: {e}", card.feature, port.channel);
                return;
            }
        },
    };
    let mut embed = message.embeds.first().cloned().map(CreateEmbed::from).unwrap_or_else(|| CreateEmbed::new().title(tester::card_title(&card.feature)));
    for (name, value) in fields {
        embed = embed.field(*name, clip(value, FIELD_LIMIT), false);
    }
    let edit = EditMessage::new().embed(embed).components(buttons(card.id, true));
    if let Err(e) = port.channel.edit_message(http, id, edit).await {
        log::warn!("couldn't update the card for {} in {}: {e}", card.feature, port.channel);
    }
}

/// Where the answers to a press go: the button press, or the Fail form.
enum Asker<'a> {
    Button(&'a ComponentInteraction),
    Form(&'a ModalInteraction),
}

impl Asker<'_> {
    async fn respond(&self, http: &Http, response: CreateInteractionResponse) {
        let sent = match self {
            Asker::Button(i) => i.create_response(http, response).await,
            Asker::Form(i) => i.create_response(http, response).await,
        };
        if let Err(e) = sent {
            log::warn!("couldn't answer a card press: {e}");
        }
    }

    /// A private line to the person who pressed.
    async fn tell(&self, http: &Http, text: &str) {
        let followup = CreateInteractionResponseFollowup::new().content(clip(text, MESSAGE_LIMIT)).ephemeral(true);
        let sent = match self {
            Asker::Button(i) => i.create_followup(http, followup).await.map(|_| ()),
            Asker::Form(i) => i.create_followup(http, followup).await.map(|_| ()),
        };
        if let Err(e) = sent {
            log::warn!("couldn't tell the tester: {e}");
        }
    }

    /// The card message as Discord sent it with the press, if it did.
    fn card_message(&self) -> Option<&Message> {
        match self {
            Asker::Button(i) => Some(&i.message),
            Asker::Form(i) => i.message.as_deref(),
        }
    }

    async fn refuse(&self, http: &Http, text: &str) {
        let message = serenity::all::CreateInteractionResponseMessage::new().content(text).ephemeral(true);
        self.respond(http, CreateInteractionResponse::Message(message)).await;
    }
}

fn may_review(bot: &Bot, member: Option<&Member>) -> bool {
    let roles: Vec<u64> = member.map(|m| m.roles.iter().map(|r| r.get()).collect()).unwrap_or_default();
    tester::may_review(&roles, bot.config.porter_role_id, bot.config.tester_role_id)
}

fn tester_name(user: &User, member: Option<&Member>) -> String {
    member.map_or_else(|| user.display_name().to_owned(), |m| m.display_name().to_owned())
}

const NOT_ALLOWED: &str = "Only members with the tester or porter role can press Pass or Fail.";
const UNKNOWN: &str = "The bot doesn't know this card any more.";

/// Pass or Fail was pressed.
pub async fn on_press(ctx: &Context, press: &ComponentInteraction, which: Press, bot: &Arc<Bot>) {
    let asker = Asker::Button(press);
    if !may_review(bot, press.member.as_ref()) {
        asker.refuse(&ctx.http, NOT_ALLOWED).await;
        return;
    }
    let Some(port) = live::find(&ctx.http, bot, press.channel_id).await else {
        asker.refuse(&ctx.http, UNKNOWN).await;
        return;
    };
    let card = match which {
        Press::Pass(card) | Press::Fail(card) => card,
        Press::FailForm(_) => return,
    };
    if let Some(reason) = closed_reason(&port, card).await {
        asker.refuse(&ctx.http, &reason).await;
        return;
    }
    match which {
        Press::Fail(card) => asker.respond(&ctx.http, CreateInteractionResponse::Modal(fail_form(card))).await,
        _ => {
            asker.respond(&ctx.http, CreateInteractionResponse::Acknowledge).await;
            let name = tester_name(&press.user, press.member.as_ref());
            conclude(&ctx.http, bot, &port, card, (&press.user, &name), Verdict::Pass, &asker).await;
        }
    }
}

/// Why a card can't be reviewed now, or `None` when it can.
async fn closed_reason(port: &Port, card: u64) -> Option<String> {
    let live = port.live.lock().await;
    match live.state.card(card) {
        None => Some(UNKNOWN.to_owned()),
        Some(c) if c.status == tester::CardStatus::Closed => Some(already(c.reviewer)),
        Some(_) => None,
    }
}

fn already(reviewer: Option<u64>) -> String {
    match reviewer {
        Some(who) => format!("This card was already reviewed by <@{who}>."),
        None => "This card is closed: a newer card replaced it.".to_owned(),
    }
}

fn fail_form(card: u64) -> CreateModal {
    let what = CreateInputText::new(InputTextStyle::Paragraph, "What went wrong?", WHAT_ID).required(true).max_length(WHAT_LIMIT);
    let steps = CreateInputText::new(InputTextStyle::Paragraph, "Steps to reproduce", STEPS_ID).required(false).max_length(STEPS_LIMIT);
    CreateModal::new(Press::FailForm(card).custom_id(), "Fail: what went wrong?")
        .components(vec![CreateActionRow::InputText(what), CreateActionRow::InputText(steps)])
}

fn form_value(form: &ModalInteraction, id: &str) -> Option<String> {
    form.data.components.iter().flat_map(|row| row.components.iter()).find_map(|c| match c {
        ActionRowComponent::InputText(input) if input.custom_id == id => input.value.clone(),
        _ => None,
    })
}

/// The Fail form was sent.
pub async fn on_form(ctx: &Context, form: &ModalInteraction, card: u64, bot: &Arc<Bot>) {
    let asker = Asker::Form(form);
    if !may_review(bot, form.member.as_ref()) {
        asker.refuse(&ctx.http, NOT_ALLOWED).await;
        return;
    }
    let Some(port) = live::find(&ctx.http, bot, form.channel_id).await else {
        asker.refuse(&ctx.http, UNKNOWN).await;
        return;
    };
    let Some(verdict) = Verdict::fail(&form_value(form, WHAT_ID).unwrap_or_default(), form_value(form, STEPS_ID).as_deref()) else {
        asker.refuse(&ctx.http, "Say what went wrong, then press Fail again.").await;
        return;
    };
    asker.respond(&ctx.http, CreateInteractionResponse::Acknowledge).await;
    let name = tester_name(&form.user, form.member.as_ref());
    conclude(&ctx.http, bot, &port, card, (&form.user, &name), verdict, &asker).await;
}

/// Closes the card for this tester, records the verdict, then shows it everywhere. When it can't
/// be recorded, the card opens again and the tester is told why.
async fn conclude(http: &Http, bot: &Arc<Bot>, port: &Port, card_id: u64, who: (&User, &str), verdict: Verdict, asker: &Asker<'_>) {
    let (user, name) = who;
    let card = {
        let mut live = port.live.lock().await;
        match live.state.close_card(card_id, Some(user.id.get())) {
            Ok(card) => {
                live.save(port);
                card
            }
            Err(CloseError::AlreadyClosed { reviewer }) => {
                drop(live);
                asker.tell(http, &already(reviewer)).await;
                return;
            }
            Err(CloseError::Unknown) => {
                drop(live);
                asker.tell(http, UNKNOWN).await;
                return;
            }
        }
    };
    let (services, project, feature, tester, recorded_verdict) =
        (Arc::clone(&bot.services), port.project.clone(), card.feature.clone(), name.to_owned(), verdict.clone());
    let recorded = tokio::task::spawn_blocking(move || review::record(services.recorder.as_ref(), &project, &feature, &tester, &recorded_verdict)).await;
    let recorded = recorded.map_err(|e| e.to_string()).and_then(|r| r);
    let recorded = match recorded {
        Ok(r) => r,
        Err(e) => {
            log::warn!("couldn't record {} for {} in {}: {e}", verdict.state(), card.feature, port.channel);
            let mut live = port.live.lock().await;
            live.state.reopen_card(card_id);
            live.save(port);
            drop(live);
            asker.tell(http, &format!("❌ Couldn't record it: {e}\nThe card is open again; try once more.")).await;
            return;
        }
    };
    let state = verdict.state();
    let mut fields = vec![(
        RESULT_FIELD,
        match &verdict {
            Verdict::Pass => format!("✅ Passed by <@{}>", user.id),
            Verdict::Fail { .. } => format!("❌ Failed by <@{}>", user.id),
        },
    )];
    if let Verdict::Fail { what, steps } = &verdict {
        fields.push(("What went wrong", what.clone()));
        if let Some(steps) = steps {
            fields.push(("Steps to reproduce", steps.clone()));
        }
    }
    {
        let mut live = port.live.lock().await;
        if matches!(verdict, Verdict::Fail { .. }) {
            live.state.failed(card_id);
        }
        live.state.record_review(&card.feature, verdict == Verdict::Pass, Utc::now());
        let shown = Event::Feature { name: card.feature.clone(), state: state.to_owned(), guide: None, note: None, similarity: None };
        live.state.board.get_or_insert_with(Board::new).apply(&shown);
        live.save(port);
    }
    port.draw(http).await;
    close_message_with(http, port, &card, asker.card_message(), &fields).await;
    let line = CreateMessage::new().content(clip(&verdict.log_line(&card.feature, name), MESSAGE_LIMIT)).allowed_mentions(CreateAllowedMentions::new());
    if let Err(e) = port.thread.send_message(http, line).await {
        log::warn!("couldn't post the verdict in the Log thread of {}: {e}", port.channel);
    }
    port.queue(SyncJob::Feature { name: card.feature.clone(), state: state.to_owned(), note: Some(verdict.record_note(name)) });
    let mut told = format!("Recorded: **{}** {}.", card.feature, if state == "passed" { "passed" } else { "is broken" });
    if let Some(e) = recorded.note_error {
        log::warn!("couldn't leave the builder a note for {} in {}: {e}", card.feature, port.channel);
        told.push_str(&format!(" The note for the builder couldn't be left ({e}); the failure is still recorded with the feature."));
    }
    asker.tell(http, &told).await;
}

/// The pictures the guide links ([`guide::linked_pictures`]), read to go on the card with the
/// guide: up to nine, within Discord's limit for the message with the guide attached.
async fn linked_pictures(port: &Port, guide: &str, read: &Result<GuideFile, String>) -> Vec<Upload> {
    let Ok(file) = read else { return Vec::new() };
    let guide_bytes = if file.too_large { 0 } else { file.text.len() as u64 };
    let (project, guide, text) = (port.project.clone(), guide.to_owned(), file.text.clone());
    let most = MOST_PER_MESSAGE - 1;
    let limit = UPLOAD_LIMIT.saturating_sub(guide_bytes);
    tokio::task::spawn_blocking(move || screens::gallery(guide::linked_pictures(&project, &guide, &text, most), most, limit)).await.unwrap_or_default()
}
