//! Telling another program that a project was just made: with `CRAFT_DISCORD_REGISTER` set to a
//! folder, each new project writes `<folder>/<created_ms>-<slug>.json` there (atomically) with its
//! folder, name, target and stack. The Discord porter bot reads that folder and mirrors the
//! project in a channel (docs/discord-bot.md). Off when the variable is unset or empty. A failure
//! never stops the project: it is logged.

use super::{Manifest, TargetRef, Workspace, slug};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// The folder new projects are registered in.
pub const REGISTER_VAR: &str = "CRAFT_DISCORD_REGISTER";
pub const REGISTRATION_VERSION: u32 = 1;

/// One registration file.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Registration {
    pub version: u32,
    /// The project folder as this process sees it (a WSL path when Craft runs in WSL).
    pub folder: PathBuf,
    pub name: String,
    pub request: String,
    /// `website`, `local` or `none`.
    pub kind: &'static str,
    /// The web address or file being rebuilt; empty for none.
    pub target: String,
    /// The stack chosen so far; often empty at creation (craft.json has it later).
    pub stack: String,
    pub created_ms: u64,
}

impl Registration {
    pub fn of(root: &Path, m: &Manifest) -> Registration {
        let (kind, target) = match &m.target {
            TargetRef::Website { url } => ("website", url.clone()),
            TargetRef::Local { path } => ("local", path.display().to_string()),
            TargetRef::None => ("none", String::new()),
        };
        Registration {
            version: REGISTRATION_VERSION,
            folder: super::absolute(root),
            name: m.name.clone(),
            request: m.request.clone(),
            kind,
            target,
            stack: m.stack.clone(),
            created_ms: m.created_ms,
        }
    }

    /// The file name: unique per project and sortable by creation time.
    pub fn file_name(&self) -> String {
        let leaf = self.folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        format!("{}-{}.json", self.created_ms, slug(&leaf))
    }
}

/// Writes the registration into `dir`. Returns the file written.
pub fn register_in(dir: &Path, ws: &Workspace) -> Result<PathBuf, String> {
    let reg = Registration::of(&ws.root, &ws.manifest);
    std::fs::create_dir_all(dir).map_err(|e| format!("couldn't create {}: {e}", dir.display()))?;
    let path = dir.join(reg.file_name());
    let bytes = serde_json::to_vec_pretty(&reg).map_err(|e| e.to_string())?;
    crate::project::write_atomic(&path, &bytes).map_err(|e| e.message)?;
    Ok(path)
}

/// Registers a just-made project when `CRAFT_DISCORD_REGISTER` names a folder.
pub(super) fn announce(ws: &Workspace) {
    let Some(dir) = std::env::var_os(REGISTER_VAR).filter(|v| !v.is_empty()) else {
        return;
    };
    match register_in(Path::new(&dir), ws) {
        Ok(path) => log::info!("registered {} in {}", ws.root.display(), path.display()),
        Err(e) => log::warn!("couldn't register {} for the Discord bot ({REGISTER_VAR}): {e}", ws.root.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_registration_names_the_folder_target_and_creation_time() {
        let d = tempfile::tempdir().unwrap();
        let ws = Workspace::create(&d.path().join("projects"), "Tani Game", "Port it", TargetRef::Website { url: "https://a.com".into() }).unwrap();
        let reg_dir = d.path().join("register");
        let path = register_in(&reg_dir, &ws).unwrap();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(name, format!("{}-tani-game.json", ws.manifest.created_ms));
        let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(v["kind"], "website");
        assert_eq!(v["target"], "https://a.com");
        assert_eq!(v["name"], "Tani Game");
        assert_eq!(v["folder"].as_str().map(PathBuf::from), Some(ws.root.clone()));
        assert_eq!(v["created_ms"], ws.manifest.created_ms);
    }
}
