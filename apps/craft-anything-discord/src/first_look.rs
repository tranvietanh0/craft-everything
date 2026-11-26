//! A mirrored project's first report: a light look at what is being rebuilt, from what craft's study
//! already wrote. No new AI call: the target, kind and stack from `craft.json`, the sections of the
//! study's notes (`capture/*.md`), the pictures of the original (`capture/`, `reference/`) as the
//! screens it shows, and the features the builder reported. Posted once, when the study's notes
//! exist ([`crate::mirror::STUDY_NOTES`]).

use crate::mirror::{STUDY_NOTES, Seen};
use craft_anything_engine::workspace::Manifest;
use craft_anything_engine::workspace::features::Feature;
use std::path::Path;

/// Most items listed per section.
const MOST: usize = 12;
const PICTURES: [&str; 4] = ["png", "jpg", "jpeg", "webp"];
/// Where the study and the person leave pictures of the original.
const PICTURE_DIRS: [&str; 2] = ["capture", "reference"];
/// Pictures are looked for this deep under each folder.
const PICTURE_DEPTH: usize = 3;

/// The first look is due once the study wrote its notes and it was not posted yet.
pub fn due(seen: &Seen) -> bool {
    !seen.first_look_posted && seen.files.contains_key(STUDY_NOTES)
}

/// The kind of target, in words.
fn kind_label(m: &Manifest) -> String {
    use craft_anything_engine::workspace::TargetRef;
    match &m.target {
        TargetRef::Website { url } => format!("Website · {url}"),
        TargetRef::Local { path } => {
            let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            let kind = match ext.as_str() {
                "apk" | "apks" | "xapk" | "aab" => "Android app",
                "exe" | "msi" => "Windows program",
                "ipa" => "iOS app",
                "" => "Folder or program",
                _ => "Local file",
            };
            format!("{kind} · {}", m.target.label())
        }
        TargetRef::None => "From the request only".to_owned(),
    }
}

/// The section headings of the study's notes (`capture/*.md`), the top title first.
fn study_sections(folder: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(folder.join("capture")) else { return Vec::new() };
    let mut notes: Vec<_> = entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "md")).collect();
    notes.sort();
    notes
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .flat_map(|text| text.lines().filter_map(|l| l.strip_prefix("## ").or_else(|| l.strip_prefix("# "))).map(|h| h.trim().to_owned()).collect::<Vec<_>>())
        .filter(|h| !h.is_empty())
        .take(MOST)
        .collect()
}

/// The pictures of the original, by name: each one is a screen or page the study saw.
fn screens(folder: &Path) -> (usize, Vec<String>) {
    let mut names = Vec::new();
    for dir in PICTURE_DIRS {
        collect_pictures(&folder.join(dir), PICTURE_DEPTH, &mut names);
    }
    names.sort();
    names.dedup();
    let count = names.len();
    names.truncate(MOST);
    (count, names)
}

fn collect_pictures(dir: &Path, depth: usize, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for path in entries.flatten().map(|e| e.path()) {
        if path.is_dir() && depth > 0 {
            collect_pictures(&path, depth - 1, out);
        } else if path.extension().is_some_and(|x| PICTURES.contains(&x.to_string_lossy().to_ascii_lowercase().as_str()))
            && let Some(stem) = path.file_stem()
        {
            out.push(stem.to_string_lossy().replace(['-', '_'], " "));
        }
    }
}

fn bullets(items: &[String]) -> String {
    items.iter().map(|i| format!("• {i}")).collect::<Vec<_>>().join("\n")
}

/// The first look's text (plain markdown; the caller clips it to Discord's limit).
pub fn text(folder: &Path, m: &Manifest, features: &[Feature]) -> String {
    let stack = if m.stack.trim().is_empty() { "not chosen yet".to_owned() } else { m.stack.clone() };
    let mut out = format!("🔎 **First look: {}** (from app)\n**Target:** {}\n**Stack:** {stack}", m.name, kind_label(m));
    if let Some(builder) = &m.builder {
        out.push_str(&format!("\n**Builder:** {builder}"));
    }
    if !m.request.trim().is_empty() {
        out.push_str(&format!("\n**Request:** {}", m.request.trim()));
    }
    let (count, names) = screens(folder);
    if count > 0 {
        out.push_str(&format!("\n\n**Screens and pages seen ({count})**\n{}", bullets(&names)));
        if count > names.len() {
            out.push_str(&format!("\n… and {} more", count - names.len()));
        }
    }
    let sections = study_sections(folder);
    if !sections.is_empty() {
        out.push_str(&format!("\n\n**What the study covered**\n{}", bullets(&sections)));
    }
    let listed: Vec<String> = features.iter().take(MOST).map(|f| format!("{} ({})", f.name, f.state.id().replace('_', " "))).collect();
    if listed.is_empty() {
        out.push_str("\n\n**Features:** none reported yet; they show on the board, with a tester card each, as the builder reports them.");
    } else {
        out.push_str(&format!("\n\n**Features and flows ({})**\n{}", features.len(), bullets(&listed)));
    }
    out
}
