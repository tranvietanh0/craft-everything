//! The app's name and package, read by the Craft CLI's own Android tools:
//! `CRAFT_CLI android_package_show <file> --json`, and, when the manifest's label is a resource
//! reference (`@0x7f…`), `android_resources_read` to look that string up. When the CLI can't read
//! the file (an `.aab` or `.xapk`, a broken APK), the name comes from the file and says so.

use craft_anything_platform::process::Process;
use serde_json::{Value, json};
use std::path::Path;
use std::time::Duration;

const TOOL_TIMEOUT: Duration = Duration::from_secs(120);
/// Resource entries asked for per page while looking up a label reference.
const RESOURCE_PAGE: usize = 1_000;
/// Pages read at most: a string table larger than this is not searched to the end.
const MAX_RESOURCE_PAGES: usize = 20;

/// Who the app is, as shown on its profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppIdentity {
    pub label: String,
    pub package: Option<String>,
    /// True when the label is the file name because the package couldn't be read.
    pub label_from_file: bool,
}

/// The manifest label as written: text, a resource reference, or nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawLabel {
    Text(String),
    Reference(String),
    Missing,
}

/// The package and raw label from `android_package_show`'s JSON (`result.package`,
/// `result.manifest.application.label`).
pub fn read_package_show(output: &Value) -> (Option<String>, RawLabel) {
    let result = output.get("result").unwrap_or(output);
    let package = result.get("package").and_then(Value::as_str).map(str::trim).filter(|p| !p.is_empty()).map(str::to_owned);
    let label = match result.pointer("/manifest/application/label").and_then(Value::as_str).map(str::trim) {
        None | Some("") => RawLabel::Missing,
        Some(l) if l.starts_with('@') => RawLabel::Reference(l.trim_start_matches('@').to_owned()),
        Some(l) => RawLabel::Text(l.to_owned()),
    };
    (package, label)
}

/// The string for resource `id` (`0x7f010000`) in one `android_resources_read` page: the default
/// locale's value first, else any locale's.
pub fn find_label(page: &Value, id: &str) -> Option<String> {
    let entries = page.pointer("/result/entries").or_else(|| page.get("entries"))?.as_array()?;
    let matching = || entries.iter().filter(|e| e.get("id").and_then(Value::as_str).is_some_and(|i| i.eq_ignore_ascii_case(id)));
    let value = |e: &Value| e.get("value").and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty() && !v.starts_with('@')).map(str::to_owned);
    matching().filter(|e| e.get("locale").and_then(Value::as_str).is_some_and(str::is_empty)).find_map(value).or_else(|| matching().find_map(value))
}

/// A readable name from the file: its stem, with `_` and `-` as spaces.
pub fn name_from_file(path: &Path) -> String {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let name = stem.replace(['_', '-'], " ");
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() { "app".to_owned() } else { name }
}

/// Reads the app's identity with the Craft CLI. Never fails: a file it can't read falls back to
/// the file name. Blocking (runs the CLI).
pub fn identify(craft_cli: &Path, file: &Path) -> AppIdentity {
    let from_file = || AppIdentity { label: name_from_file(file), package: None, label_from_file: true };
    let shown = match run_tool(craft_cli, "android_package_show", &[file.as_os_str().to_string_lossy().as_ref(), "--json"]) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("android_package_show could not read {}: {e}", file.display());
            return from_file();
        }
    };
    let (package, label) = read_package_show(&shown);
    let label = match label {
        RawLabel::Text(text) => Some(text),
        RawLabel::Reference(id) => resolve_reference(craft_cli, file, &id),
        RawLabel::Missing => None,
    };
    match label {
        Some(label) => AppIdentity { label, package, label_from_file: false },
        None => AppIdentity { package, ..from_file() },
    }
}

fn resolve_reference(craft_cli: &Path, file: &Path, id: &str) -> Option<String> {
    for page in 0..MAX_RESOURCE_PAGES {
        let params = json!({"path": file, "with_strings": false, "type_name": "string", "offset": page * RESOURCE_PAGE, "limit": RESOURCE_PAGE});
        let out = match run_tool(craft_cli, "android_resources_read", &["--json", &params.to_string()]) {
            Ok(v) => v,
            Err(e) => {
                log::warn!("android_resources_read could not read {}: {e}", file.display());
                return None;
            }
        };
        if let Some(label) = find_label(&out, id) {
            return Some(label);
        }
        let shown = out.pointer("/result/entries").and_then(Value::as_array).map_or(0, Vec::len);
        let total = out.pointer("/result/entries_matching").and_then(Value::as_u64).unwrap_or(0);
        if shown < RESOURCE_PAGE || u64::try_from((page + 1) * RESOURCE_PAGE).unwrap_or(u64::MAX) >= total {
            return None;
        }
    }
    None
}

fn run_tool(craft_cli: &Path, tool: &str, args: &[&str]) -> Result<Value, String> {
    let out = Process::new(craft_cli).arg(tool).args(args).timeout(TOOL_TIMEOUT).run_checked().map_err(|e| e.message)?;
    let value: Value = serde_json::from_slice(&out.stdout).map_err(|e| format!("{tool} printed no JSON: {e}"))?;
    if let Some(error) = value.get("error") {
        return Err(error.get("message").and_then(Value::as_str).unwrap_or("the tool failed").to_owned());
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_text_label_and_package_are_read() {
        let v = json!({"result": {"package": "com.example.snake", "manifest": {"application": {"label": "Snake"}}}});
        assert_eq!(read_package_show(&v), (Some("com.example.snake".to_owned()), RawLabel::Text("Snake".to_owned())));
    }

    #[test]
    fn a_resource_label_is_a_reference() {
        let v = json!({"result": {"package": "com.example.snake", "manifest": {"application": {"label": "@0x7f010000"}}}});
        assert_eq!(read_package_show(&v).1, RawLabel::Reference("0x7f010000".to_owned()));
        assert_eq!(read_package_show(&json!({"result": {}})), (None, RawLabel::Missing));
    }

    #[test]
    fn the_default_locale_wins_a_reference_lookup() {
        let page = json!({"result": {"entries": [
            {"id": "0x7f010000", "locale": "fr", "value": "Serpent"},
            {"id": "0x7f010000", "locale": "", "value": "Snake"},
            {"id": "0x7f010001", "locale": "", "value": "Other"},
        ]}});
        assert_eq!(find_label(&page, "0x7F010000").as_deref(), Some("Snake"));
        assert_eq!(find_label(&page, "0x7f0100ff"), None);
    }

    #[test]
    fn the_file_name_fallback_is_readable() {
        assert_eq!(name_from_file(Path::new("/ports/x/input/super_snake-2.apk")), "super snake 2");
        assert_eq!(name_from_file(Path::new("/ports/x/input/.apk")), "app");
    }
}
