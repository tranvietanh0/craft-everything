//! Tokens and cost, summed from `turn_done` events. The contract leaves totals to the consumer, so a
//! job keeps its own [`Usage`] and a port's or a day's total is the sum of the parts. herdr prices
//! its turns at list prices, an estimate: an estimated total is shown as `~$`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Usage {
    pub turns: u64,
    /// False once any turn said its token count is unknown.
    pub tokens_known: bool,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// `None` until a turn reports a cost.
    pub cost_usd: Option<f64>,
    /// True once any priced turn was an estimate.
    pub cost_is_estimate: bool,
}

impl Default for Usage {
    fn default() -> Self {
        Usage { turns: 0, tokens_known: true, input_tokens: 0, output_tokens: 0, cost_usd: None, cost_is_estimate: false }
    }
}

impl Usage {
    /// One `turn_done` event.
    pub fn add_turn(&mut self, input: u64, output: u64, tokens_unknown: bool, cost: Option<f64>, estimate: bool) {
        self.add(&Usage { turns: 1, tokens_known: !tokens_unknown, input_tokens: input, output_tokens: output, cost_usd: cost, cost_is_estimate: estimate });
    }

    /// Adds another total to this one (a job's to a port's, an hour's to a day's).
    pub fn add(&mut self, other: &Usage) {
        self.turns = self.turns.saturating_add(other.turns);
        self.tokens_known &= other.tokens_known;
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        if let Some(c) = other.cost_usd {
            self.cost_usd = Some(self.cost_usd.unwrap_or(0.0) + c);
            self.cost_is_estimate |= other.cost_is_estimate;
        }
    }

    /// The sum of `parts`.
    pub fn total<'a>(parts: impl IntoIterator<Item = &'a Usage>) -> Usage {
        parts.into_iter().fold(Usage::default(), |mut sum, u| {
            sum.add(u);
            sum
        })
    }

    /// `$1.2345`, `~$1.2345` for an estimate, `no cost reported` when no turn had one.
    pub fn cost_text(&self) -> String {
        match self.cost_usd {
            Some(c) if self.cost_is_estimate => format!("~${c:.4}"),
            Some(c) => format!("${c:.4}"),
            None => "no cost reported".to_owned(),
        }
    }

    /// Tokens, turns and cost in one line.
    pub fn describe(&self) -> String {
        if self.turns == 0 {
            return "no agent turns yet".to_owned();
        }
        let tokens =
            if self.tokens_known { format!("{} tokens", self.input_tokens.saturating_add(self.output_tokens)) } else { "token count unknown".to_owned() };
        let cost = if self.cost_usd.is_some() { format!(" · {}", self.cost_text()) } else { String::new() };
        format!("{tokens} over {} turns{cost}", self.turns)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turns_add_up_and_an_estimate_marks_the_whole_total() {
        let mut usage = Usage::default();
        usage.add_turn(100, 20, false, Some(0.25), true);
        usage.add_turn(50, 5, false, Some(0.5), false);
        assert_eq!((usage.turns, usage.input_tokens + usage.output_tokens), (2, 175));
        assert!(usage.cost_text().starts_with("~$"), "{}", usage.cost_text());
        let exact = Usage::total([&Usage { turns: 1, cost_usd: Some(1.0), ..Usage::default() }]);
        assert!(exact.cost_text().starts_with('$'), "an exact cost has no ~");
    }

    #[test]
    fn a_turn_without_a_cost_keeps_the_total_but_not_the_estimate_flag() {
        let mut usage = Usage::default();
        usage.add_turn(10, 1, false, Some(0.1), false);
        usage.add_turn(10, 1, true, None, true);
        assert!(!usage.cost_is_estimate, "an unpriced turn does not make a priced total an estimate");
        assert!(usage.describe().contains("token count unknown"));
        assert!(Usage::default().cost_text().contains("no cost"));
    }

    #[test]
    fn a_port_total_is_the_sum_of_its_jobs() {
        let mut a = Usage::default();
        a.add_turn(1, 1, false, Some(0.2), true);
        let mut b = Usage::default();
        b.add_turn(2, 2, false, Some(0.3), false);
        let total = Usage::total([&a, &b]);
        assert_eq!(total.turns, 2);
        assert!(total.cost_usd.is_some_and(|c| c > a.cost_usd.unwrap_or(0.0) && c > b.cost_usd.unwrap_or(0.0)));
        assert!(total.cost_is_estimate, "one estimated job makes the port's total an estimate");
    }
}
