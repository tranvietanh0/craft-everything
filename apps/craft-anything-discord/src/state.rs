//! What the bot keeps about one port in `PORTS_ROOT/<slug>/bot.json`: where it posts, the tester
//! cards it has posted, the Plane and GitHub ids, its jobs ([`crate::jobs`]), its activity
//! ([`crate::activity`]) and its last board. Kept on disk so Pass and Fail still work, the board
//! is drawn again, and Plane and GitHub stay in step, after the bot restarts.

use crate::activity::Activity;
use crate::board::Board;
use crate::jobs::JobRecord;
use crate::plane::PlaneIds;
use crate::repo::RepoInfo;
use crate::tester::{CardAction, CardStatus, card_action};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The file name inside `PORTS_ROOT/<slug>/`.
pub const STATE_FILE: &str = "bot.json";

/// One posted tester card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Card {
    /// Unique within the port; the buttons carry it.
    pub id: u64,
    pub feature: String,
    /// The Discord message the card is.
    pub message: u64,
    pub status: CardStatus,
    /// The feature was reported broken after this card was posted.
    #[serde(default)]
    pub broken_since: bool,
    /// Who pressed Pass or Fail (a Discord user ID).
    #[serde(default)]
    pub reviewer: Option<u64>,
}

/// Why a card can't be closed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CloseError {
    #[error("this card is no longer known to the bot")]
    Unknown,
    #[error("this card was already reviewed")]
    AlreadyClosed { reviewer: Option<u64> },
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PortState {
    pub slug: String,
    pub label: String,
    /// Discord IDs: the port's channel, its Log thread, the pinned board and the pinned profile.
    pub channel: u64,
    pub thread: u64,
    pub board_message: u64,
    pub profile_message: u64,
    pub cards: Vec<Card>,
    pub next_card: u64,
    pub plane: Option<PlaneIds>,
    pub repo: Option<RepoInfo>,
    /// Every run, oldest first.
    pub jobs: Vec<JobRecord>,
    pub activity: Vec<Activity>,
    /// The board as the last run left it, drawn again after a restart.
    pub board: Option<Board>,
}

impl PortState {
    pub fn path(port_dir: &Path) -> PathBuf {
        port_dir.join(STATE_FILE)
    }

    /// `Ok(None)` when the port has no state file yet; a file that can't be read is an error.
    pub fn load(port_dir: &Path) -> Result<Option<PortState>, String> {
        let path = Self::path(port_dir);
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("couldn't read {}: {e}", path.display())),
        };
        serde_json::from_slice(&bytes).map(Some).map_err(|e| format!("{} is not valid: {e}", path.display()))
    }

    pub fn save(&self, port_dir: &Path) -> Result<(), String> {
        let path = Self::path(port_dir);
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| format!("couldn't encode the port state: {e}"))?;
        craft_anything_platform::fs::write_atomic(&path, &bytes).map_err(|e| format!("couldn't save {}: {}", path.display(), e.message))
    }

    /// The port folder and state of the port whose channel is `channel`, looked up on disk (after a
    /// restart the bot knows no port in memory). Folders without a readable state are skipped.
    pub fn find_by_channel(ports_root: &Path, channel: u64) -> Option<(PathBuf, PortState)> {
        let entries = std::fs::read_dir(ports_root).ok()?;
        entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()).find_map(|dir| match PortState::load(&dir) {
            Ok(Some(state)) if state.channel == channel => Some((dir, state)),
            _ => None,
        })
    }

    /// Every port's state under `ports_root`. A folder without `bot.json` is not a port; one whose
    /// `bot.json` can't be read is logged and left out.
    pub fn load_all(ports_root: &Path) -> Vec<PortState> {
        let entries = match std::fs::read_dir(ports_root) {
            Ok(entries) => entries,
            Err(e) => {
                log::warn!("couldn't list {}: {e}", ports_root.display());
                return Vec::new();
            }
        };
        let mut states: Vec<PortState> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .filter_map(|dir| {
                PortState::load(&dir).unwrap_or_else(|e| {
                    log::warn!("{e}");
                    None
                })
            })
            .collect();
        states.sort_by(|a, b| a.slug.cmp(&b.slug));
        states
    }

    fn latest_card(&self, feature: &str) -> Option<&Card> {
        self.cards.iter().rev().find(|c| c.feature == feature)
    }

    /// What to do now that `feature` reports `ready_to_test`, and the card it is compared with.
    pub fn on_ready(&self, feature: &str) -> (CardAction, Option<Card>) {
        let latest = self.latest_card(feature);
        (card_action(latest.map(|c| c.status), latest.is_some_and(|c| c.broken_since)), latest.cloned())
    }

    /// The feature was reported broken: the next `ready_to_test` is a new round.
    pub fn on_broken(&mut self, feature: &str) {
        if let Some(card) = self.cards.iter_mut().rev().find(|c| c.feature == feature) {
            card.broken_since = true;
        }
    }

    /// Records a newly posted card and returns its id.
    pub fn add_card(&mut self, feature: &str, message: u64) -> u64 {
        self.next_card = self.next_card.saturating_add(1);
        let id = self.next_card;
        self.cards.push(Card { id, feature: feature.to_owned(), message, status: CardStatus::Open, broken_since: false, reviewer: None });
        id
    }

    /// The card's id for its message, once it is posted (the buttons need the id first).
    pub fn reserve_card(&mut self) -> u64 {
        self.next_card.saturating_add(1)
    }

    pub fn card(&self, id: u64) -> Option<&Card> {
        self.cards.iter().find(|c| c.id == id)
    }

    /// Closes a card for one reviewer; a second press finds it closed. Returns the card as it was.
    pub fn close_card(&mut self, id: u64, reviewer: Option<u64>) -> Result<Card, CloseError> {
        let card = self.cards.iter_mut().find(|c| c.id == id).ok_or(CloseError::Unknown)?;
        if card.status == CardStatus::Closed {
            return Err(CloseError::AlreadyClosed { reviewer: card.reviewer });
        }
        let before = card.clone();
        card.status = CardStatus::Closed;
        card.reviewer = reviewer;
        Ok(before)
    }

    /// Opens a card again after its verdict couldn't be recorded.
    pub fn reopen_card(&mut self, id: u64) {
        if let Some(card) = self.cards.iter_mut().find(|c| c.id == id) {
            card.status = CardStatus::Open;
            card.reviewer = None;
        }
    }

    /// A Fail was recorded: the card is closed and the feature's next ready report is a new round.
    pub fn failed(&mut self, id: u64) {
        if let Some(card) = self.cards.iter_mut().find(|c| c.id == id) {
            card.broken_since = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_card_per_round_and_a_break_starts_a_new_round() {
        let mut state = PortState::default();
        assert_eq!(state.on_ready("Map").0, CardAction::Post);
        let first = state.add_card("Map", 100);
        assert_eq!(state.on_ready("Map").0, CardAction::Skip, "the live card covers a repeat report");
        state.on_broken("Map");
        let (action, previous) = state.on_ready("Map");
        assert_eq!(action, CardAction::PostFresh { disable_previous: true });
        assert_eq!(previous.map(|c| c.id), Some(first), "the live card is the one to disable");
        assert_eq!(state.on_ready("Shop").0, CardAction::Post, "features have their own cards");
    }

    #[test]
    fn a_failed_card_is_closed_and_the_next_ready_report_gets_a_fresh_card() {
        let mut state = PortState::default();
        let id = state.add_card("Map", 100);
        assert!(state.close_card(id, Some(9)).is_ok());
        state.failed(id);
        assert_eq!(state.on_ready("Map").0, CardAction::PostFresh { disable_previous: false });
    }

    #[test]
    fn a_card_is_reviewed_once() {
        let mut state = PortState::default();
        let id = state.add_card("Map", 100);
        assert!(state.close_card(id, Some(9)).is_ok());
        assert_eq!(state.close_card(id, Some(10)), Err(CloseError::AlreadyClosed { reviewer: Some(9) }));
        state.reopen_card(id);
        assert!(state.close_card(id, Some(10)).is_ok(), "a card reopened after a failed record can be reviewed");
        assert_eq!(state.close_card(99, None), Err(CloseError::Unknown));
    }

    #[test]
    fn the_reserved_id_is_the_one_the_card_gets() {
        let mut state = PortState::default();
        let reserved = state.reserve_card();
        assert_eq!(state.add_card("Map", 1), reserved);
        assert_ne!(state.reserve_card(), reserved);
    }

    #[test]
    fn a_saved_state_is_found_again_by_its_channel() {
        let root = tempfile::tempdir().unwrap();
        let port = root.path().join("snake");
        std::fs::create_dir_all(&port).unwrap();
        std::fs::create_dir_all(root.path().join("no-state")).unwrap();
        let mut state = PortState { slug: "snake".into(), channel: 42, ..PortState::default() };
        state.add_card("Map", 7);
        state.save(&port).unwrap();
        let (dir, found) = PortState::find_by_channel(root.path(), 42).unwrap();
        assert_eq!((dir, found), (port, state));
        assert!(PortState::find_by_channel(root.path(), 43).is_none());
    }
}
