//! Tester cards: the pure rules behind a `ready_to_test` feature's card and its Pass/Fail buttons.
//! Nothing here touches Discord, Plane, GitHub or the disk; the Discord side draws the card from these.

use std::path::{Component, Path, PathBuf};

use crate::text::clip;

/// Discord's limit for an embed description, in characters.
pub const GUIDE_LIMIT: usize = 4_096;
/// Discord's limit for an embed title, in characters.
const TITLE_LIMIT: usize = 256;
/// Plane identifiers are at most this many characters.
const PLANE_IDENTIFIER_MAX: usize = 5;

/// Whether a member with these role IDs may press Pass or Fail.
pub fn may_review(member_roles: &[u64], porter_role: u64, tester_role: u64) -> bool {
    member_roles.iter().any(|role| *role == porter_role || *role == tester_role)
}

/// The title of a feature's tester card.
pub fn card_title(feature: &str) -> String {
    clip(&format!("🧪 Ready to test: {feature}"), TITLE_LIMIT)
}

/// Where a feature's live card stands, as the bot last left it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardStatus {
    /// Pass and Fail are still live.
    Open,
    /// Someone pressed Pass or Fail; the buttons are disabled.
    Closed,
}

/// What the bot does when a feature reports `ready_to_test`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardAction {
    /// No card exists for this feature yet: post one.
    Post,
    /// A card already covers this round: post nothing.
    Skip,
    /// The feature was broken after its last card and is ready again: post a fresh card. The old
    /// card's buttons are disabled first when they are still live.
    PostFresh { disable_previous: bool },
}

/// One card per testing round. `broken_since` is true when the feature was reported `broken`
/// after the card it is compared with was posted.
pub fn card_action(previous: Option<CardStatus>, broken_since: bool) -> CardAction {
    match previous {
        None => CardAction::Post,
        Some(_) if !broken_since => CardAction::Skip,
        Some(CardStatus::Open) => CardAction::PostFresh { disable_previous: true },
        Some(CardStatus::Closed) => CardAction::PostFresh { disable_previous: false },
    }
}

/// Why a guide path is refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GuideError {
    #[error("the guide path is empty")]
    Empty,
    #[error("the guide must be a relative path inside the project folder")]
    Escapes,
}

/// Joins a guide path onto the project folder, refusing anything that is absolute or climbs out
/// of the folder. This is lexical only: the caller must also check the resolved file before reading.
pub fn guide_path(project: &Path, guide: &str) -> Result<PathBuf, GuideError> {
    let rel = guide.trim();
    if rel.is_empty() {
        return Err(GuideError::Empty);
    }
    let rel_path = Path::new(rel);
    let stays_inside = rel_path.components().all(|part| matches!(part, Component::Normal(_) | Component::CurDir));
    if rel_path.is_absolute() || !stays_inside {
        return Err(GuideError::Escapes);
    }
    Ok(project.join(rel_path))
}

/// The guide text as the card shows it, cut to the embed limit, and whether it was cut. A cut
/// guide points the reader at the attached full file.
pub fn guide_excerpt(markdown: &str) -> (String, bool) {
    (clip(markdown, GUIDE_LIMIT), markdown.chars().count() > GUIDE_LIMIT)
}

/// How a feature state is shown in Plane: the state name, and whether it is filed as a bug.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlaneMapping {
    pub state: &'static str,
    pub bug: bool,
}

/// The Plane state for a feature's wire name. `None` for a name the engine does not use.
pub fn plane_mapping(feature_state: &str) -> Option<PlaneMapping> {
    let (state, bug) = match feature_state {
        "planned" => ("Backlog", false),
        "in_progress" => ("In Progress", false),
        "ready_to_test" => ("Ready to test", false),
        "passed" => ("Done", false),
        "broken" => ("Todo", true),
        _ => return None,
    };
    Some(PlaneMapping { state, bug })
}

/// Why no Plane project identifier can be made.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IdentifierError {
    #[error("the app name has no letters to build a Plane identifier from")]
    NoLetters,
    #[error("every Plane identifier for this app is taken")]
    Exhausted,
}

/// A Plane project identifier: up to five uppercase letters from the slug, made unique against
/// `taken` with a numeric suffix (`RETRO`, then `RETR2`, `RETR3`, ...).
pub fn plane_identifier(slug: &str, taken: &[String]) -> Result<String, IdentifierError> {
    let letters: String = slug.chars().filter(char::is_ascii_alphabetic).take(PLANE_IDENTIFIER_MAX).collect::<String>().to_ascii_uppercase();
    if letters.is_empty() {
        return Err(IdentifierError::NoLetters);
    }
    if !taken.contains(&letters) {
        return Ok(letters);
    }
    for number in 2..=99 {
        let suffix = number.to_string();
        let keep = letters.len().min(PLANE_IDENTIFIER_MAX - suffix.len());
        let candidate = format!("{}{suffix}", &letters[..keep]);
        if !taken.contains(&candidate) {
            return Ok(candidate);
        }
    }
    Err(IdentifierError::Exhausted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_porter_or_tester_members_may_review() {
        assert!(may_review(&[1, 500], 500, 600), "a porter may review");
        assert!(may_review(&[600], 500, 600), "a tester may review");
        assert!(!may_review(&[1, 2], 500, 600), "anyone else is refused");
        assert!(!may_review(&[], 500, 600), "no roles is refused");
    }

    #[test]
    fn the_card_title_carries_the_feature_name_and_fits() {
        assert_eq!(card_title("Map"), "🧪 Ready to test: Map");
        assert!(card_title(&"x".repeat(400)).chars().count() <= TITLE_LIMIT);
    }

    #[test]
    fn a_first_ready_report_posts_a_card() {
        assert_eq!(card_action(None, false), CardAction::Post);
    }

    #[test]
    fn a_repeat_ready_report_with_a_live_card_posts_nothing() {
        assert_eq!(card_action(Some(CardStatus::Open), false), CardAction::Skip);
    }

    #[test]
    fn a_ready_report_after_a_break_posts_fresh_and_disables_a_live_card() {
        assert_eq!(card_action(Some(CardStatus::Open), true), CardAction::PostFresh { disable_previous: true });
    }

    #[test]
    fn a_ready_report_after_a_failed_card_posts_fresh_without_touching_the_old_one() {
        assert_eq!(card_action(Some(CardStatus::Closed), true), CardAction::PostFresh { disable_previous: false });
    }

    #[test]
    fn a_closed_card_with_no_break_stays_closed() {
        assert_eq!(card_action(Some(CardStatus::Closed), false), CardAction::Skip);
    }

    #[test]
    fn a_guide_inside_the_project_resolves_under_it() {
        let project = Path::new("/srv/ports/app/project");
        assert_eq!(guide_path(project, "tests/map.md"), Ok(project.join("tests/map.md")));
        assert_eq!(guide_path(project, " ./guide.md "), Ok(project.join("guide.md")));
    }

    #[test]
    fn a_guide_that_leaves_the_project_is_refused() {
        let project = Path::new("/srv/ports/app/project");
        for bad in ["../secret.md", "tests/../../x.md", "/etc/passwd", "a/../../b.md"] {
            assert_eq!(guide_path(project, bad), Err(GuideError::Escapes), "{bad}");
        }
        assert_eq!(guide_path(project, "  "), Err(GuideError::Empty));
    }

    #[test]
    fn a_short_guide_is_shown_whole_and_a_long_one_is_cut() {
        assert_eq!(guide_excerpt("## Goal"), ("## Goal".to_owned(), false));
        let long = "é".repeat(GUIDE_LIMIT + 10);
        let (cut, truncated) = guide_excerpt(&long);
        assert!(truncated && cut.chars().count() <= GUIDE_LIMIT && cut.ends_with('…'));
    }

    #[test]
    fn every_engine_state_has_a_plane_state_and_unknown_names_do_not() {
        assert_eq!(plane_mapping("planned"), Some(PlaneMapping { state: "Backlog", bug: false }));
        assert_eq!(plane_mapping("in_progress"), Some(PlaneMapping { state: "In Progress", bug: false }));
        assert_eq!(plane_mapping("ready_to_test"), Some(PlaneMapping { state: "Ready to test", bug: false }));
        assert_eq!(plane_mapping("passed"), Some(PlaneMapping { state: "Done", bug: false }));
        assert_eq!(plane_mapping("broken"), Some(PlaneMapping { state: "Todo", bug: true }));
        assert_eq!(plane_mapping("shipped"), None);
    }

    #[test]
    fn a_plane_identifier_is_five_uppercase_letters_from_the_slug() {
        assert_eq!(plane_identifier("retro-racer-deluxe", &[]), Ok("RETRO".to_owned()));
        assert_eq!(plane_identifier("ab", &[]), Ok("AB".to_owned()));
    }

    #[test]
    fn a_taken_plane_identifier_gets_a_numeric_suffix_within_five_characters() {
        let taken = vec!["RETRO".to_owned(), "RETR2".to_owned()];
        assert_eq!(plane_identifier("retro-racer", &taken), Ok("RETR3".to_owned()));
        assert_eq!(plane_identifier("retro", &["RETRO".to_owned()]), Ok("RETR2".to_owned()));
    }

    #[test]
    fn a_slug_without_letters_has_no_plane_identifier() {
        assert_eq!(plane_identifier("1234-$$", &[]), Err(IdentifierError::NoLetters));
    }

    #[test]
    fn every_suffix_taken_is_reported_not_looped_forever() {
        let base = "ABCDE";
        let taken: Vec<String> = std::iter::once(base.to_owned())
            .chain((2..=99).map(|n: u32| {
                let suffix = n.to_string();
                format!("{}{suffix}", &base[..base.len().min(PLANE_IDENTIFIER_MAX - suffix.len())])
            }))
            .collect();
        assert_eq!(plane_identifier("abcdef", &taken), Err(IdentifierError::Exhausted));
    }
}
