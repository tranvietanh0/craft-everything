//! Where things are for the Claude Code herdr runs: the same folder seen from Linux (WSL) and from
//! Windows, the folder Claude Code keeps a session's transcript in, and the names herdr is given.

use std::path::{Path, PathBuf};

/// Where Craft Anything runs relative to the Claude Code that herdr starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Host {
    /// The same system: paths are passed unchanged.
    Native,
    /// Craft Anything runs in WSL (distro named) and herdr and Claude Code run on Windows.
    Wsl { distro: String },
}

impl Host {
    /// This process's host: WSL when `WSL_DISTRO_NAME` is set on a Unix build.
    pub fn current() -> Host {
        match std::env::var("WSL_DISTRO_NAME").ok().filter(|d| cfg!(unix) && !d.trim().is_empty()) {
            Some(distro) => Host::Wsl { distro: distro.trim().to_string() },
            None => Host::Native,
        }
    }

    /// `path` as Claude Code (on the agent's side) reads it.
    pub fn to_agent(&self, path: &Path) -> String {
        let text = path.display().to_string();
        match self {
            Host::Native => text,
            Host::Wsl { distro } => linux_to_windows(&text, distro),
        }
    }

    /// A path Claude Code (on the agent's side) named, as this process reads it.
    pub fn from_agent(&self, path: &str) -> PathBuf {
        match self {
            Host::Native => PathBuf::from(path),
            Host::Wsl { .. } => PathBuf::from(windows_to_linux(path).unwrap_or_else(|| path.to_string())),
        }
    }
}

/// A Linux path as Windows reads it: `/mnt/c/x/y` is `C:\x\y`; any other absolute path is inside
/// the distro (`\\wsl.localhost\<distro>\x`). A path already in Windows form keeps it.
pub fn linux_to_windows(path: &str, distro: &str) -> String {
    if is_windows_path(path) {
        return path.replace('/', "\\");
    }
    if let Some(rest) = path.strip_prefix("/mnt/") {
        let mut parts = rest.splitn(2, '/');
        let drive = parts.next().unwrap_or_default();
        if drive.len() == 1 && drive.chars().all(|c| c.is_ascii_alphabetic()) {
            let tail = parts.next().unwrap_or_default().trim_end_matches('/').replace('/', "\\");
            return format!("{}:\\{tail}", drive.to_ascii_uppercase());
        }
    }
    let inside = path.trim_end_matches('/').replace('/', "\\");
    format!("\\\\wsl.localhost\\{distro}{inside}")
}

/// A Windows path as WSL reads it: `C:\x` is `/mnt/c/x`; `\\wsl.localhost\<distro>\x` (or
/// `\\wsl$\…`) is `/x`. `None` for anything else (a relative path, another share).
pub fn windows_to_linux(path: &str) -> Option<String> {
    let p = path.replace('\\', "/");
    for share in ["//wsl.localhost/", "//wsl$/"] {
        if let Some(rest) = p.strip_prefix(share) {
            let inside = rest.split_once('/').map_or("", |(_, tail)| tail);
            return Some(format!("/{inside}"));
        }
    }
    if !is_windows_path(&p) {
        return None;
    }
    let drive = p.chars().next()?.to_ascii_lowercase();
    let tail = p.get(2..).unwrap_or_default().trim_start_matches('/').trim_end_matches('/');
    Some(if tail.is_empty() { format!("/mnt/{drive}") } else { format!("/mnt/{drive}/{tail}") })
}

fn is_windows_path(path: &str) -> bool {
    let b = path.as_bytes();
    b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b.len() == 2 || b[2] == b'\\' || b[2] == b'/')
}

/// The folder name Claude Code keeps a working folder's sessions under (`projects/<name>`): every
/// character that isn't an ASCII letter or digit becomes `-` (`C:\Users\me` is `C--Users-me`).
/// Very long names are shortened by Claude Code itself; the transcript is then found by its id.
pub fn transcript_dir_name(cwd: &str) -> String {
    // Claude Code counts UTF-16 units, so a character outside the BMP gives two dashes.
    cwd.chars().flat_map(|c| if c.is_ascii_alphanumeric() { vec![c] } else { vec!['-'; c.len_utf16()] }).collect()
}

/// The herdr agent name for a project folder: `craft-<folder>-<hash>`, within herdr's
/// `[a-z][a-z0-9_-]{0,31}`. `kind` tells a builder's agent from a question's (`b`, `q`, `c`).
pub fn agent_name(project: &Path, kind: char) -> String {
    let slug = slug(project);
    let hash = short_hash(&project.display().to_string());
    // "craft-" + slug + "-" + kind + hash(4) fits 32.
    let room = 32 - "craft-".len() - 1 - 1 - hash.len();
    let slug: String = slug.chars().take(room).collect();
    format!("craft-{}-{kind}{hash}", slug.trim_end_matches('-'))
}

/// The workspace label for a project folder: its name, as the person knows it.
pub fn workspace_label(project: &Path) -> String {
    let name = project.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if name.trim().is_empty() { "craft".to_string() } else { format!("craft {name}") }
}

fn slug(project: &Path) -> String {
    let name = project.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let mut out = String::new();
    for c in name.chars() {
        let c = if c.is_ascii_alphanumeric() { c } else { '-' };
        if !(c == '-' && out.ends_with('-')) {
            out.push(c);
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() { "project".to_string() } else { out }
}

/// Four hex digits from `text` (FNV-1a), so two projects with the same folder name differ.
fn short_hash(text: &str) -> String {
    let mut h: u32 = 0x811c_9dc5;
    for b in text.bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    format!("{:04x}", h & 0xffff)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_maps_to_windows_and_back_to_the_same_folder() {
        for linux in ["/mnt/c/Projects/My Game", "/home/me/ports/app", "/mnt/d"] {
            let win = linux_to_windows(linux, "Ubuntu-24.04");
            assert!(win.contains('\\') && !win.contains('/'), "{win}");
            assert_eq!(windows_to_linux(&win).as_deref(), Some(linux), "{linux} -> {win}");
        }
        assert!(linux_to_windows("/mnt/c/Projects", "U").starts_with("C:\\"), "a drive mount is that drive");
        assert!(linux_to_windows("/home/me", "U").starts_with("\\\\wsl.localhost\\U\\"), "the distro's own files go through its share");
        assert_eq!(windows_to_linux("relative\\path"), None, "a relative path isn't guessed at");
    }

    #[test]
    fn the_transcript_folder_name_matches_claude_codes() {
        // Names Claude Code itself made on this kind of computer.
        assert_eq!(transcript_dir_name("C:\\Users\\me"), "C--Users-me");
        assert_eq!(transcript_dir_name("C:\\Projects\\MyProject\\AOV-ArenaOfVal"), "C--Projects-MyProject-AOV-ArenaOfVal");
        assert!(transcript_dir_name("C:\\a b\\ü").chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
    }

    #[test]
    fn agent_names_fit_herdr_and_tell_projects_apart() {
        let long = Path::new("/home/me/A Very Long Project Folder Name That Goes On And On");
        for p in [long, Path::new("/x/Ünïcode"), Path::new("/")] {
            let name = agent_name(p, 'b');
            assert!(name.len() <= 32 && name.starts_with(|c: char| c.is_ascii_lowercase()), "{name}");
            assert!(name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_'), "{name}");
        }
        assert_ne!(agent_name(Path::new("/a/game"), 'b'), agent_name(Path::new("/b/game"), 'b'), "same folder name, other project");
        assert_ne!(agent_name(Path::new("/a/game"), 'b'), agent_name(Path::new("/a/game"), 'q'), "a question beside the builder");
    }
}
