//! Embeds: the pinned app profile and the progress board.

use craft_anything_discord::board::BoardView;
use craft_anything_discord::identify::AppIdentity;
use craft_anything_discord::run::Target;
use craft_anything_discord::text::clip;
use serenity::all::{Colour, CreateEmbed, UserId};

/// Discord's limit for one embed field's value, in characters.
const FIELD_LIMIT: usize = 1_024;
/// Discord's limit for an embed description, in characters.
const DESCRIPTION_LIMIT: usize = 4_096;
const BYTES_PER_MB: f64 = 1024.0 * 1024.0;
const RUNNING: Colour = Colour::BLURPLE;
const PASSED: Colour = Colour::DARK_GREEN;
const FAILED: Colour = Colour::RED;

pub fn profile_embed(app: &AppIdentity, target: Target, requester: UserId, file_name: &str, bytes: u64) -> CreateEmbed {
    let name = if app.label_from_file { format!("{} (name from file)", app.label) } else { app.label.clone() };
    // Display only: the size in MB with one decimal.
    let size = format!("{:.1} MB", bytes as f64 / BYTES_PER_MB);
    CreateEmbed::new()
        .title(clip(&format!("App profile: {name}"), 256))
        .colour(RUNNING)
        .field("Name", clip(&name, FIELD_LIMIT), true)
        .field("Package", clip(app.package.as_deref().unwrap_or("unknown"), FIELD_LIMIT), true)
        .field("Target", target.label(), true)
        .field("Requested by", format!("<@{requester}>"), true)
        .field("File", clip(&format!("{file_name} ({size})"), FIELD_LIMIT), true)
}

pub fn board_embed(label: &str, view: &BoardView) -> CreateEmbed {
    let steps: Vec<String> = view.steps.iter().map(|s| format!("{} {}", s.marker, s.label)).collect();
    let description = clip(&format!("{}\n\n{}", steps.join("  ·  "), view.activity), DESCRIPTION_LIMIT);
    let colour = match view.outcome.as_deref() {
        None => RUNNING,
        Some(o) if o.starts_with('✅') => PASSED,
        Some(_) => FAILED,
    };
    let mut embed = CreateEmbed::new()
        .title(clip(&format!("Progress: {label}"), 256))
        .description(description)
        .colour(colour)
        .field("Elapsed", view.elapsed.clone(), true)
        .field("Usage", clip(&view.usage, FIELD_LIMIT), true);
    if let Some(check) = &view.last_check {
        embed = embed.field("Last check", clip(check, FIELD_LIMIT), false);
    }
    if !view.features.is_empty() {
        embed = embed.field("Features", clip(&view.features.join("\n"), FIELD_LIMIT), false);
    }
    if let Some(preview) = &view.preview {
        embed = embed.field("Preview", clip(preview, FIELD_LIMIT), false);
    }
    if let Some(outcome) = &view.outcome {
        embed = embed.field("Result", clip(outcome, FIELD_LIMIT), false);
    }
    embed
}
