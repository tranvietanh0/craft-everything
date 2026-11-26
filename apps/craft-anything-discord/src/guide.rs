//! Reading a feature's test guide for its card: only a file inside the project folder (a link out
//! of it is refused, as `..` and absolute paths already are by [`crate::tester::guide_path`]), and
//! at most [`ATTACH_LIMIT`] bytes, which is also the most that is attached.

use crate::tester::{GuideError, guide_path};
use std::io::Read;
use std::path::Path;

/// The largest guide attached to a card; a larger one is shown cut and not attached.
pub const ATTACH_LIMIT: u64 = 1024 * 1024;

/// A guide as read for its card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuideFile {
    /// The text (all of it, or the first [`ATTACH_LIMIT`] bytes).
    pub text: String,
    /// The file name for the attachment.
    pub file_name: String,
    /// The file was larger than [`ATTACH_LIMIT`]: `text` is its start and it is not attached.
    pub too_large: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReadError {
    #[error(transparent)]
    Path(#[from] GuideError),
    #[error("the guide {0} is not a file in the project")]
    Missing(String),
    #[error("the guide {0} leads out of the project")]
    Outside(String),
    #[error("couldn't read the guide {0}: {1}")]
    Io(String, String),
}

/// Reads `guide` (project-relative) from `project`. Blocking.
pub fn read_guide(project: &Path, guide: &str) -> Result<GuideFile, ReadError> {
    let path = guide_path(project, guide)?;
    let shown = guide.trim().to_owned();
    let io = |e: std::io::Error| ReadError::Io(shown.clone(), e.to_string());
    let real = path.canonicalize().map_err(|_| ReadError::Missing(shown.clone()))?;
    let real_project = project.canonicalize().map_err(io)?;
    if !real.starts_with(&real_project) {
        return Err(ReadError::Outside(shown));
    }
    if !real.is_file() {
        return Err(ReadError::Missing(shown));
    }
    let size = std::fs::metadata(&real).map_err(io)?.len();
    let mut bytes = Vec::new();
    std::fs::File::open(&real).map_err(io)?.take(ATTACH_LIMIT).read_to_end(&mut bytes).map_err(io)?;
    let file_name = real.file_name().map_or_else(|| "guide.md".to_owned(), |n| n.to_string_lossy().into_owned());
    Ok(GuideFile { text: String::from_utf8_lossy(&bytes).into_owned(), file_name, too_large: size > ATTACH_LIMIT })
}

/// The pictures a guide links (`![…](path)`): the screenshots the builder tied to the feature. A
/// path is taken from the guide's folder, then from the project folder; only a picture file inside
/// the project counts (links followed first). At most `most`, in the guide's order. Blocking.
pub fn linked_pictures(project: &Path, guide: &str, text: &str, most: usize) -> Vec<std::path::PathBuf> {
    let Ok(real_project) = project.canonicalize() else { return Vec::new() };
    let guide_dir = Path::new(guide.trim()).parent().unwrap_or(Path::new(""));
    let mut out: Vec<std::path::PathBuf> = Vec::new();
    for target in text.split("![").skip(1).filter_map(|rest| rest.split_once("](")).filter_map(|(_, rest)| rest.split_once(')')).map(|(t, _)| t) {
        // `![x](path "title")`: the path is the first word, maybe in `<…>`.
        let target = target.split_whitespace().next().unwrap_or("").trim_matches(['<', '>']);
        if target.is_empty() || target.contains("://") || Path::new(target).is_absolute() {
            continue;
        }
        let found = [project.join(guide_dir).join(target), project.join(target)]
            .into_iter()
            .filter_map(|p| p.canonicalize().ok())
            .find(|p| p.starts_with(&real_project) && p.is_file() && crate::screens::is_picture(p));
        if let Some(path) = found.filter(|p| !out.contains(p)) {
            out.push(path);
        }
        if out.len() >= most {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_guide_links_pictures_inside_the_project_only() {
        let project = tempfile::tempdir().unwrap();
        let p = project.path();
        std::fs::create_dir_all(p.join("tests")).unwrap();
        std::fs::create_dir_all(p.join("checks/round-1")).unwrap();
        std::fs::write(p.join("checks/round-1/map.png"), b"png").unwrap();
        std::fs::write(p.join("tests/inline.jpg"), b"jpg").unwrap();
        let text = "![map](../checks/round-1/map.png) ![same](checks/round-1/map.png \"again\")\n![](inline.jpg) ![out](../../x.png) ![web](https://e.com/a.png) ![md](map.md)";
        let real = p.canonicalize().unwrap();
        assert_eq!(linked_pictures(p, "tests/map.md", text, 10), [real.join("checks/round-1/map.png"), real.join("tests/inline.jpg")]);
        assert_eq!(linked_pictures(p, "tests/map.md", text, 1).len(), 1, "at most `most`");
        assert!(linked_pictures(p, "tests/map.md", "no pictures", 10).is_empty());
    }

    #[test]
    fn a_guide_in_the_project_is_read_whole() {
        let project = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(project.path().join("tests")).unwrap();
        std::fs::write(project.path().join("tests/map.md"), "# Map\nOpen it.").unwrap();
        let guide = read_guide(project.path(), "tests/map.md").unwrap();
        assert_eq!(guide, GuideFile { text: "# Map\nOpen it.".into(), file_name: "map.md".into(), too_large: false });
    }

    #[test]
    fn a_missing_or_escaping_guide_is_refused() {
        let project = tempfile::tempdir().unwrap();
        assert!(matches!(read_guide(project.path(), "tests/none.md"), Err(ReadError::Missing(_))));
        assert!(matches!(read_guide(project.path(), "../x.md"), Err(ReadError::Path(GuideError::Escapes))));
        std::fs::create_dir_all(project.path().join("tests")).unwrap();
        assert!(matches!(read_guide(project.path(), "tests"), Err(ReadError::Missing(_))), "a folder is not a guide");
    }

    #[cfg(unix)]
    #[test]
    fn a_link_out_of_the_project_is_refused() {
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.md"), "secret").unwrap();
        let project = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path().join("secret.md"), project.path().join("guide.md")).unwrap();
        assert!(matches!(read_guide(project.path(), "guide.md"), Err(ReadError::Outside(_))));
    }

    #[test]
    fn a_huge_guide_is_read_in_part_and_flagged() {
        let project = tempfile::tempdir().unwrap();
        let text = "a".repeat(usize::try_from(ATTACH_LIMIT).unwrap() + 10);
        std::fs::write(project.path().join("big.md"), &text).unwrap();
        let guide = read_guide(project.path(), "big.md").unwrap();
        assert!(guide.too_large && guide.text.len() as u64 == ATTACH_LIMIT);
    }
}
