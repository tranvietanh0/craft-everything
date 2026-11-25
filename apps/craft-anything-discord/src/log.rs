//! One event → one line for the batched "Log" thread. The board shows the summary; this keeps the detail.

use crate::events::Event;

/// `None` for events that stay off the Log thread (the board or a stored turn already shows them).
pub fn log_line(event: &Event) -> Option<String> {
    match event {
        Event::AgentText { text } => Some(text.clone()),
        Event::Status { text } => Some(format!("· {text}")),
        Event::Step { label, text, .. } => Some(format!("## {label}: {text}")),
        Event::Output { text } => Some(format!("stderr: {text}")),
        Event::ToolStarted { tool, .. } => Some(format!("▶ {tool}")),
        Event::ToolFinished { tool, ok, summary, .. } => {
            let mark = if *ok { "✔" } else { "✖" };
            Some(format!("{mark} {tool}: {summary}"))
        }
        Event::ConsentNeeded { tool, title, .. } => Some(format!("approval wanted for {tool} ({title}); refused under --unattended")),
        Event::Ask { title, .. } => Some(format!("question: {title}")),
        Event::Checked { check, passed, .. } => Some(format!("check {check}: {}", if *passed { "passed" } else { "failed" })),
        Event::Error { message, .. } => Some(format!("error: {message}")),
        Event::Preview { url } => Some(format!("preview: {url}")),
        Event::Project { .. } | Event::TurnDone { .. } | Event::Note {} | Event::Feature { .. } | Event::Finished { .. } | Event::Other => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{Line, parse_line};

    fn line_for(json: &str) -> Option<String> {
        match parse_line(json) {
            Ok(Some(Line::Event(e))) => log_line(&e),
            _ => None,
        }
    }

    #[test]
    fn agent_text_is_logged_verbatim() {
        assert_eq!(line_for(r#"{"type":"agent_text","text":"Built it."}"#).as_deref(), Some("Built it."));
    }

    #[test]
    fn board_only_events_stay_off_the_log() {
        assert_eq!(line_for(r#"{"type":"turn_done","backend":"x","input_tokens":1,"output_tokens":1,"tokens_unknown":false,"cost_usd":null,"cost_is_estimate":true}"#), None);
        assert_eq!(line_for(r#"{"type":"finished","ok":true,"message":"done"}"#), None);
    }

    #[test]
    fn a_refused_approval_says_it_was_refused() {
        let line = line_for(r#"{"type":"consent_needed","call_id":"c1","tool":"install","title":"Install SDK","explanation":"needs approval"}"#);
        assert!(line.is_some_and(|l| l.contains("refused")));
    }
}
