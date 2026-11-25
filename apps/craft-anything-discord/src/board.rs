//! The progress board: the state a `rebuild` run leaves behind, folded from events, and rendered to
//! plain view data. No Discord types and no clock: the caller passes the elapsed time to [`Board::render`].

use crate::events::Event;
use std::cmp::Ordering;
use std::time::Duration;

const SECS_PER_MINUTE: u64 = 60;
const SECS_PER_HOUR: u64 = 3_600;

/// The four stages a run goes through, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    Study,
    Build,
    Check,
    Done,
}

impl Stage {
    pub const ALL: [Stage; 4] = [Stage::Study, Stage::Build, Stage::Check, Stage::Done];

    pub fn from_name(name: &str) -> Option<Stage> {
        match name {
            "study" => Some(Stage::Study),
            "build" => Some(Stage::Build),
            "check" => Some(Stage::Check),
            "done" => Some(Stage::Done),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Stage::Study => "Study",
            Stage::Build => "Build",
            Stage::Check => "Check",
            Stage::Done => "Done",
        }
    }
}

/// The builder's state for one feature, as the `feature` event reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeatureState {
    Planned,
    InProgress,
    ReadyToTest,
    Passed,
    Broken,
}

impl FeatureState {
    /// `None` for a state this build does not know; the caller then ignores that event.
    pub fn from_name(name: &str) -> Option<FeatureState> {
        match name {
            "planned" => Some(FeatureState::Planned),
            "in_progress" => Some(FeatureState::InProgress),
            "ready_to_test" => Some(FeatureState::ReadyToTest),
            "passed" => Some(FeatureState::Passed),
            "broken" => Some(FeatureState::Broken),
            _ => None,
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            FeatureState::Planned => "⏳",
            FeatureState::InProgress => "🔨",
            FeatureState::ReadyToTest => "🧪",
            FeatureState::Passed => "✅",
            FeatureState::Broken => "❌",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Outcome {
    ok: bool,
    paused: bool,
    message: String,
}

#[derive(Debug, Clone, PartialEq)]
struct CheckResult {
    check: String,
    passed: bool,
    similarity: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
struct Usage {
    turns: u64,
    tokens_known: bool,
    input_tokens: u64,
    output_tokens: u64,
    cost_usd: Option<f64>,
    cost_is_estimate: bool,
}

impl Default for Usage {
    fn default() -> Self {
        Usage { turns: 0, tokens_known: true, input_tokens: 0, output_tokens: 0, cost_usd: None, cost_is_estimate: false }
    }
}

impl Usage {
    /// Sums the turns: the contract leaves totals to the consumer.
    fn add_turn(&mut self, input: u64, output: u64, tokens_unknown: bool, cost: Option<f64>, estimate: bool) {
        self.turns = self.turns.saturating_add(1);
        self.tokens_known &= !tokens_unknown;
        self.input_tokens = self.input_tokens.saturating_add(input);
        self.output_tokens = self.output_tokens.saturating_add(output);
        if let Some(c) = cost {
            self.cost_usd = Some(self.cost_usd.unwrap_or(0.0) + c);
        }
        self.cost_is_estimate |= estimate;
    }

    fn describe(&self) -> String {
        if self.turns == 0 {
            return "no agent turns yet".to_owned();
        }
        let tokens =
            if self.tokens_known { format!("{} tokens", self.input_tokens.saturating_add(self.output_tokens)) } else { "token count unknown".to_owned() };
        let cost = match self.cost_usd {
            Some(c) if self.cost_is_estimate => format!(" · ${c:.4} (estimate)"),
            Some(c) => format!(" · ${c:.4}"),
            None => String::new(),
        };
        format!("{tokens} over {} turns{cost}", self.turns)
    }
}

/// Folds events into the board state. Events it does not show (agent text, tool calls) are ignored
/// here; [`crate::log::log_line`] sends them to the Log thread.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Board {
    stage: Option<Stage>,
    status: Option<String>,
    error: Option<String>,
    usage: Usage,
    last_check: Option<CheckResult>,
    features: Vec<(String, FeatureState)>,
    preview: Option<String>,
    outcome: Option<Outcome>,
}

impl Board {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn apply(&mut self, event: &Event) {
        match event {
            Event::Step { step, text, .. } => {
                if let Some(stage) = Stage::from_name(step) {
                    self.stage = Some(stage);
                    self.status = Some(text.clone());
                    self.error = None;
                }
            }
            Event::Status { text } => {
                self.status = Some(text.clone());
                self.error = None;
            }
            Event::Error { message, .. } => self.error = Some(message.clone()),
            Event::TurnDone { input_tokens, output_tokens, tokens_unknown, cost_usd, cost_is_estimate, .. } => {
                self.usage.add_turn(*input_tokens, *output_tokens, *tokens_unknown, *cost_usd, *cost_is_estimate);
            }
            Event::Checked { check, passed, similarity, .. } => {
                self.last_check = Some(CheckResult { check: check.clone(), passed: *passed, similarity: *similarity });
            }
            Event::Feature { name, state, .. } => {
                if let Some(state) = FeatureState::from_name(state) {
                    self.set_feature(name, state);
                }
            }
            Event::Preview { url } => self.preview = Some(url.clone()),
            Event::Finished { ok, paused, message } => {
                self.outcome = Some(Outcome { ok: *ok, paused: *paused, message: message.clone() });
            }
            _ => {}
        }
    }

    fn set_feature(&mut self, name: &str, state: FeatureState) {
        match self.features.iter_mut().find(|(n, _)| n == name) {
            Some(entry) => entry.1 = state,
            None => self.features.push((name.to_owned(), state)),
        }
    }

    /// The marker for one stage. Earlier stages are done; the current one is running, or failed
    /// or paused once the run has an outcome; later ones are pending. A passed run shows all done.
    fn step_marker(&self, stage: Stage) -> &'static str {
        if self.outcome.as_ref().is_some_and(|o| o.ok) {
            return "✅";
        }
        let Some(current) = self.stage else {
            return "⏳";
        };
        match stage.cmp(&current) {
            Ordering::Less => "✅",
            Ordering::Greater => "⏳",
            Ordering::Equal => match &self.outcome {
                None => "🔄",
                Some(o) if o.paused => "⏸️",
                Some(_) => "❌",
            },
        }
    }

    fn activity(&self) -> String {
        match (&self.error, &self.status) {
            (Some(e), _) => format!("⚠️ {e}"),
            (None, Some(s)) => s.clone(),
            (None, None) => "Waiting to start".to_owned(),
        }
    }

    pub fn render(&self, elapsed: Duration) -> BoardView {
        BoardView {
            steps: Stage::ALL.iter().map(|&s| StepRow { label: s.label(), marker: self.step_marker(s) }).collect(),
            activity: self.activity(),
            elapsed: format_elapsed(elapsed),
            usage: self.usage.describe(),
            last_check: self.last_check.as_ref().map(describe_check),
            features: self.features.iter().map(|(name, state)| format!("{} {name}", state.icon())).collect(),
            preview: self.preview.clone(),
            outcome: self.outcome.as_ref().map(describe_outcome),
        }
    }
}

fn describe_check(c: &CheckResult) -> String {
    let verdict = if c.passed { "passed" } else { "failed" };
    let similarity = c.similarity.map_or_else(|| "n/a".to_owned(), |s| format!("{s:.0}%"));
    format!("{} {}: {verdict}, similarity {similarity}", if c.passed { "✅" } else { "❌" }, c.check)
}

fn describe_outcome(o: &Outcome) -> String {
    let icon = match (o.ok, o.paused) {
        (true, _) => "✅",
        (false, true) => "⏸️",
        (false, false) => "❌",
    };
    format!("{icon} {}", o.message)
}

/// `m:ss`, or `h:mm:ss` from one hour on.
pub fn format_elapsed(elapsed: Duration) -> String {
    let total = elapsed.as_secs();
    let (hours, minutes, seconds) = (total / SECS_PER_HOUR, total % SECS_PER_HOUR / SECS_PER_MINUTE, total % SECS_PER_MINUTE);
    if hours > 0 { format!("{hours}:{minutes:02}:{seconds:02}") } else { format!("{minutes}:{seconds:02}") }
}

/// Plain view data for one board embed; the I/O layer maps it to Discord fields.
#[derive(Debug, Clone, PartialEq)]
pub struct BoardView {
    pub steps: Vec<StepRow>,
    pub activity: String,
    pub elapsed: String,
    pub usage: String,
    pub last_check: Option<String>,
    pub features: Vec<String>,
    pub preview: Option<String>,
    pub outcome: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepRow {
    pub label: &'static str,
    pub marker: &'static str,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{Event, Line, parse_line};

    fn feed(board: &mut Board, lines: &[&str]) {
        for line in lines {
            if let Ok(Some(Line::Event(e))) = parse_line(line) {
                board.apply(&e);
            }
        }
    }

    fn markers(view: &BoardView) -> Vec<&'static str> {
        view.steps.iter().map(|s| s.marker).collect()
    }

    const STUDY: &str = r#"{"type":"step","step":"study","label":"Study","text":"Reading the app"}"#;
    const BUILD: &str = r#"{"type":"step","step":"build","label":"Build","text":"Building"}"#;

    #[test]
    fn a_stage_is_done_once_its_successor_starts() {
        let mut board = Board::new();
        feed(&mut board, &[STUDY]);
        assert_eq!(markers(&board.render(Duration::ZERO)), ["🔄", "⏳", "⏳", "⏳"]);
        feed(&mut board, &[BUILD]);
        assert_eq!(markers(&board.render(Duration::ZERO)), ["✅", "🔄", "⏳", "⏳"]);
    }

    #[test]
    fn a_passed_run_shows_every_stage_done() {
        let mut board = Board::new();
        feed(&mut board, &[STUDY, BUILD, r#"{"type":"finished","ok":true,"message":"Ready"}"#]);
        assert_eq!(markers(&board.render(Duration::ZERO)), ["✅", "✅", "✅", "✅"]);
    }

    #[test]
    fn a_failed_run_marks_the_current_stage_failed() {
        let mut board = Board::new();
        feed(&mut board, &[STUDY, BUILD, r#"{"type":"finished","ok":false,"message":"Build broke"}"#]);
        let view = board.render(Duration::ZERO);
        assert_eq!(markers(&view), ["✅", "❌", "⏳", "⏳"]);
        assert_eq!(view.outcome.as_deref(), Some("❌ Build broke"));
    }

    #[test]
    fn feature_states_are_shown_and_updated_in_place() {
        let mut board = Board::new();
        feed(
            &mut board,
            &[
                r#"{"type":"feature","name":"Login","state":"planned","guide":null,"note":null,"similarity":null}"#,
                r#"{"type":"feature","name":"Shop","state":"in_progress","guide":null,"note":null,"similarity":null}"#,
                r#"{"type":"feature","name":"Login","state":"passed","guide":null,"note":null,"similarity":91.0}"#,
            ],
        );
        assert_eq!(board.render(Duration::ZERO).features, ["✅ Login", "🔨 Shop"]);
    }

    #[test]
    fn a_feature_with_an_unknown_state_is_ignored() {
        let mut board = Board::new();
        feed(&mut board, &[r#"{"type":"feature","name":"X","state":"someday","guide":null,"note":null,"similarity":null}"#]);
        assert!(board.render(Duration::ZERO).features.is_empty());
    }

    #[test]
    fn usage_sums_turns_and_marks_estimates() {
        let mut board = Board::new();
        feed(
            &mut board,
            &[
                r#"{"type":"turn_done","backend":"Claude Code","input_tokens":100,"output_tokens":20,"tokens_unknown":false,"cost_usd":0.25,"cost_is_estimate":true}"#,
                r#"{"type":"turn_done","backend":"Claude Code","input_tokens":50,"output_tokens":5,"tokens_unknown":false,"cost_usd":null,"cost_is_estimate":false}"#,
            ],
        );
        assert_eq!(board.render(Duration::ZERO).usage, "175 tokens over 2 turns · $0.2500 (estimate)");
    }

    #[test]
    fn no_turns_says_so() {
        assert_eq!(Board::new().render(Duration::ZERO).usage, "no agent turns yet");
    }

    #[test]
    fn an_error_replaces_the_activity_until_the_next_step() {
        let mut board = Board::new();
        feed(&mut board, &[STUDY, r#"{"type":"error","message":"tool crashed","remediation":null,"log":null}"#]);
        assert_eq!(board.render(Duration::ZERO).activity, "⚠️ tool crashed");
        feed(&mut board, &[BUILD]);
        assert_eq!(board.render(Duration::ZERO).activity, "Building");
    }

    #[test]
    fn the_last_check_is_shown_with_its_similarity() {
        let mut board = Board::new();
        feed(&mut board, &[r#"{"type":"checked","round":1,"check":"build","passed":false,"similarity":null,"note":null,"artifacts":[]}"#]);
        assert_eq!(board.render(Duration::ZERO).last_check.as_deref(), Some("❌ build: failed, similarity n/a"));
    }

    #[test]
    fn elapsed_is_minutes_then_hours() {
        assert_eq!(format_elapsed(Duration::from_secs(187)), "3:07");
        assert_eq!(format_elapsed(Duration::from_secs(3_725)), "1:02:05");
    }

    #[test]
    fn unknown_events_leave_the_board_unchanged() {
        let mut board = Board::new();
        feed(&mut board, &[STUDY]);
        let before = board.clone();
        board.apply(&Event::Other);
        assert_eq!(board, before);
    }
}
