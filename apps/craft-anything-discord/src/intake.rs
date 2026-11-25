//! Getting the app in: which file types and sizes are accepted, where the file goes, and the
//! blocking download (an attachment's or a link's URL) into `PORTS_ROOT/<slug>/input/`.
//! The app's name and package come from [`crate::identify`].

use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Android package files `/port` accepts, lower case, without the dot.
pub const ACCEPTED_EXTENSIONS: [&str; 4] = ["apk", "xapk", "apks", "aab"];
const BYTES_PER_MB: u64 = 1024 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const FIRST_BYTE_TIMEOUT: Duration = Duration::from_secs(60);
const BODY_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const MAX_REDIRECTS: u32 = 5;
const USER_AGENT: &str = "CraftAnything-discord";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IntakeError {
    #[error("give either an `apk` attachment or a `link`")]
    NoSource,
    #[error("give an `apk` attachment or a `link`, not both")]
    BothSources,
    #[error("{name} is not an Android package: send a .apk, .xapk, .apks or .aab file")]
    UnsupportedType { name: String },
    #[error("the file is {} MB; the limit is {} MB", mb(*.bytes), mb(*.cap))]
    TooLarge { bytes: u64, cap: u64 },
    #[error("the link must be an http:// or https:// address")]
    BadLink,
    #[error("the download failed: {0}")]
    Download(String),
    #[error("couldn't save the file: {0}")]
    Io(String),
}

fn mb(bytes: u64) -> u64 {
    bytes.div_ceil(BYTES_PER_MB)
}

/// What `/port` was given to fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A Discord attachment: its CDN URL, file name and size as Discord reports them.
    Attachment { url: String, file_name: String, size: u64 },
    /// A link the person pasted.
    Link { url: String },
}

impl Source {
    /// Exactly one of the two options must be set. A link is checked for its scheme; an
    /// attachment for its type and declared size, so a wrong file is refused before any download.
    pub fn choose(attachment: Option<(String, String, u64)>, link: Option<&str>, cap: u64) -> Result<Source, IntakeError> {
        let link = link.map(str::trim).filter(|l| !l.is_empty());
        match (attachment, link) {
            (Some(_), Some(_)) => Err(IntakeError::BothSources),
            (None, None) => Err(IntakeError::NoSource),
            (Some((url, file_name, size)), None) => {
                package_extension(&file_name).ok_or_else(|| IntakeError::UnsupportedType { name: file_name.clone() })?;
                check_size(size, cap)?;
                Ok(Source::Attachment { url, file_name, size })
            }
            (None, Some(url)) => {
                let lower = url.to_ascii_lowercase();
                if !(lower.starts_with("https://") || lower.starts_with("http://")) || url.chars().any(char::is_whitespace) {
                    return Err(IntakeError::BadLink);
                }
                Ok(Source::Link { url: url.to_owned() })
            }
        }
    }

    pub fn url(&self) -> &str {
        match self {
            Source::Attachment { url, .. } | Source::Link { url } => url,
        }
    }

    /// The file name to start from: the attachment's, or the link path's last part.
    pub fn suggested_name(&self) -> Option<String> {
        match self {
            Source::Attachment { file_name, .. } => Some(file_name.clone()),
            Source::Link { url } => file_name_from_url(url),
        }
    }
}

/// The accepted extension of `file_name` (case-insensitive), or `None` for any other file.
pub fn package_extension(file_name: &str) -> Option<&'static str> {
    let (_, ext) = file_name.rsplit_once('.')?;
    ACCEPTED_EXTENSIONS.iter().copied().find(|a| a.eq_ignore_ascii_case(ext))
}

/// A size at or under the cap passes.
pub fn check_size(bytes: u64, cap: u64) -> Result<(), IntakeError> {
    if bytes > cap { Err(IntakeError::TooLarge { bytes, cap }) } else { Ok(()) }
}

/// The last path segment of a URL, without its query or fragment; `None` when the path ends in `/`.
pub fn file_name_from_url(url: &str) -> Option<String> {
    let no_query = url.split(['?', '#']).next().unwrap_or(url);
    let after_scheme = no_query.split_once("://").map_or(no_query, |(_, rest)| rest);
    let (_, path) = after_scheme.split_once('/')?;
    path.rsplit('/').next().filter(|s| !s.is_empty()).map(str::to_owned)
}

/// The `filename` of a `Content-Disposition` header, quoted or not.
pub fn file_name_from_disposition(header: &str) -> Option<String> {
    header.split(';').map(str::trim).find_map(|part| {
        let (key, value) = part.split_once('=')?;
        if !key.trim().eq_ignore_ascii_case("filename") {
            return None;
        }
        let value = value.trim().trim_matches('"').trim();
        (!value.is_empty()).then(|| value.to_owned())
    })
}

/// A file name that is safe on disk: no folders, only `[A-Za-z0-9._-]`, no leading dot.
pub fn safe_file_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let cleaned: String = base.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '_' }).collect();
    let trimmed = cleaned.trim_start_matches('.');
    if trimmed.is_empty() { "app.apk".to_owned() } else { trimmed.to_owned() }
}

/// `PORTS_ROOT/<slug>/input`, where the downloaded package is kept.
pub fn input_dir(ports_root: &Path, slug: &str) -> PathBuf {
    ports_root.join(slug).join("input")
}

/// `PORTS_ROOT/<slug>/project`, the rebuild's project folder.
pub fn project_dir(ports_root: &Path, slug: &str) -> PathBuf {
    ports_root.join(slug).join("project")
}

/// Downloads `source` into `dir` (created if missing) and returns the saved file. The name comes
/// from the source, else from the response's `Content-Disposition`; it must have an accepted
/// extension. More than `cap` bytes is refused and the partial file removed. Blocking.
pub fn download(source: &Source, dir: &Path, cap: u64) -> Result<PathBuf, IntakeError> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .user_agent(USER_AGENT)
        .max_redirects(MAX_REDIRECTS)
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_recv_response(Some(FIRST_BYTE_TIMEOUT))
        .timeout_recv_body(Some(BODY_TIMEOUT))
        .build()
        .new_agent();
    let response = agent.get(source.url()).call().map_err(|e| IntakeError::Download(e.to_string()))?;
    let status = response.status();
    if !status.is_success() {
        return Err(IntakeError::Download(format!("the server answered {status}")));
    }
    let disposition = response.headers().get("content-disposition").and_then(|v| v.to_str().ok()).and_then(file_name_from_disposition);
    let name = source
        .suggested_name()
        .filter(|n| package_extension(n).is_some())
        .or(disposition)
        .ok_or_else(|| IntakeError::UnsupportedType { name: source.suggested_name().unwrap_or_else(|| "the linked file".to_owned()) })?;
    package_extension(&name).ok_or_else(|| IntakeError::UnsupportedType { name: name.clone() })?;
    let body = response.into_body();
    if let Some(declared) = body.content_length() {
        check_size(declared, cap)?;
    }
    std::fs::create_dir_all(dir).map_err(|e| IntakeError::Io(e.to_string()))?;
    let path = dir.join(safe_file_name(&name));
    let partial = path.with_extension("part");
    let written = save_capped(body.into_reader(), &partial, cap);
    let finished = written.and_then(|bytes| check_size(bytes, cap)).and_then(|()| std::fs::rename(&partial, &path).map_err(|e| IntakeError::Io(e.to_string())));
    if let Err(e) = finished {
        let _ = std::fs::remove_file(&partial);
        return Err(e);
    }
    Ok(path)
}

/// Copies at most `cap + 1` bytes, so an over-cap body is noticed without reading all of it.
fn save_capped(reader: impl Read, to: &Path, cap: u64) -> Result<u64, IntakeError> {
    let io_error = |e: io::Error| IntakeError::Io(e.to_string());
    let mut file = File::create(to).map_err(io_error)?;
    let copied = io::copy(&mut reader.take(cap.saturating_add(1)), &mut file).map_err(|e| IntakeError::Download(e.to_string()))?;
    file.flush().map_err(io_error)?;
    Ok(copied)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAP: u64 = 10 * BYTES_PER_MB;

    fn attachment(name: &str, size: u64) -> Option<(String, String, u64)> {
        Some(("https://cdn.example/a".to_owned(), name.to_owned(), size))
    }

    #[test]
    fn only_android_package_extensions_are_accepted() {
        assert!(package_extension("Game.APK").is_some());
        assert!(package_extension("bundle.xapk").is_some() && package_extension("split.apks").is_some());
        assert!(package_extension("store.aab").is_some());
        assert!(package_extension("game.zip").is_none());
        assert!(package_extension("apk").is_none(), "a bare word is not an extension");
    }

    #[test]
    fn exactly_one_source_is_required() {
        assert_eq!(Source::choose(None, Some("  "), CAP), Err(IntakeError::NoSource));
        assert_eq!(Source::choose(attachment("a.apk", 1), Some("https://x/a.apk"), CAP), Err(IntakeError::BothSources));
    }

    #[test]
    fn an_attachment_is_checked_before_download() {
        assert!(matches!(Source::choose(attachment("notes.txt", 1), None, CAP), Err(IntakeError::UnsupportedType { .. })));
        assert!(matches!(Source::choose(attachment("big.apk", CAP + 1), None, CAP), Err(IntakeError::TooLarge { .. })));
        assert!(Source::choose(attachment("ok.apk", CAP), None, CAP).is_ok(), "the cap itself is allowed");
    }

    #[test]
    fn a_link_needs_a_web_scheme() {
        assert_eq!(Source::choose(None, Some("file:///etc/passwd"), CAP), Err(IntakeError::BadLink));
        assert_eq!(Source::choose(None, Some("https://x/a b.apk"), CAP), Err(IntakeError::BadLink));
        assert!(Source::choose(None, Some("HTTPS://example.com/app.apk"), CAP).is_ok());
    }

    #[test]
    fn the_link_name_drops_query_and_fragment() {
        assert_eq!(file_name_from_url("https://example.com/dl/My%20App.apk?token=1#x").as_deref(), Some("My%20App.apk"));
        assert_eq!(file_name_from_url("https://example.com/dl/"), None);
        assert_eq!(file_name_from_url("https://example.com"), None);
    }

    #[test]
    fn content_disposition_gives_the_name() {
        assert_eq!(file_name_from_disposition("attachment; filename=\"game.xapk\"").as_deref(), Some("game.xapk"));
        assert_eq!(file_name_from_disposition("inline").as_deref(), None);
    }

    #[test]
    fn saved_names_cannot_leave_the_folder() {
        let name = safe_file_name("../../etc/My App?.apk");
        assert!(!name.contains('/') && !name.contains(".."));
        assert!(name.ends_with(".apk"));
        assert!(!safe_file_name("..hidden.apk").starts_with('.'));
    }

    #[test]
    fn an_over_cap_body_is_noticed() {
        let dir = std::env::temp_dir().join(format!("craft-discord-intake-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("x.part");
        let copied = save_capped(&[0_u8; 64][..], &file, 16).unwrap();
        assert!(check_size(copied, 16).is_err());
        let copied = save_capped(&[0_u8; 16][..], &file, 16).unwrap();
        assert!(check_size(copied, 16).is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
