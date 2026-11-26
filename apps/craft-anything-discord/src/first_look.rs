//! A mirrored project's first report: a light look at what is being rebuilt, from what craft's study
//! already wrote. No new AI call: the target, kind and stack from `craft.json`, the sections of the
//! study's notes (`capture/*.md`), the pictures of the original (`capture/`, `reference/`) as the
//! screens it shows, and the features the builder reported. Posted once, when the study's notes
//! exist ([`crate::mirror::STUDY_NOTES`]).

use crate::mirror::{STUDY_NOTES, Seen};
use craft_anything_engine::workspace::Manifest;
use craft_anything_engine::workspace::features::Feature;
use std::path::{Path, PathBuf};

/// Most items listed per section.
const MOST: usize = 12;
/// Most pictures of the original attached to the first look (one message's worth).
pub const GALLERY: usize = crate::screens::MOST_PER_MESSAGE;
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

/// The pictures of the original in `capture/` and `reference/` (the study's and the person's),
/// sorted: each one is a screen or page the study saw. The first look's gallery.
pub fn original_pictures(folder: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = PICTURE_DIRS.iter().flat_map(|dir| crate::screens::pictures_in(&folder.join(dir), PICTURE_DEPTH)).collect();
    found.sort();
    found
}

/// The pictures of the original, by name.
fn screens(folder: &Path) -> (usize, Vec<String>) {
    let mut names: Vec<String> =
        original_pictures(folder).iter().filter_map(|p| p.file_stem()).map(|stem| stem.to_string_lossy().replace(['-', '_'], " ")).collect();
    names.sort();
    names.dedup();
    let count = names.len();
    names.truncate(MOST);
    (count, names)
}

/// An Android app is never started by craft (the study reads its package), so its first look
/// says why there are no pictures of it.
fn is_android(m: &Manifest) -> bool {
    use craft_anything_engine::workspace::TargetRef;
    let TargetRef::Local { path } = &m.target else { return false };
    path.extension().is_some_and(|e| matches!(e.to_string_lossy().to_ascii_lowercase().as_str(), "apk" | "apks" | "xapk" | "aab"))
}

/// Said in place of the gallery when craft made no picture of an Android original.
pub const NO_ANDROID_SCREENS: &str = "no screenshots of the original (needs a device): craft reads an Android app's package but doesn't run it, so nothing pictured its screens. Picturing them needs adb and an emulator or a phone with the app.";

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
        out.push_str(&format!("\n\n**Original screens ({count})**\n{}", bullets(&names)));
        if count > names.len() {
            out.push_str(&format!("\n… and {} more", count - names.len()));
        }
    } else if is_android(m) {
        out.push_str(&format!("\n\n**Original screens:** {NO_ANDROID_SCREENS}"));
    } else {
        out.push_str("\n\n**Original screens:** none (no pictures in capture/ or reference/).");
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
