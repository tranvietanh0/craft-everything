//! Typed events from `craft-anything-cli rebuild --events ndjson` (contract: `docs/rebuild-events.md`).
//! Only the fields the bot reads are named; serde ignores the rest, so added fields do not break it.
//! Unknown `type` values are ignored, as the contract asks; a known type with a wrong shape is an error.

use serde::Deserialize;
use serde_json::Value;

/// One parsed stdout line.
#[derive(Debug, Clone, PartialEq)]
pub enum Line {
    Event(Event),
    /// A `type` this build does not know. Consumers ignore it (logged at debug by the caller).
    Unknown(String),
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum LineError {
    #[error("not a JSON value: {0}")]
    NotJson(String),
    #[error("the JSON value has no string `type` field")]
    MissingType,
    #[error("`{kind}` event does not match the contract: {reason}")]
    Malformed { kind: String, reason: String },
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    Project {
        dir: String,
    },
    Step {
        step: String,
        label: String,
        text: String,
    },
    AgentText {
        text: String,
    },
    Status {
        text: String,
    },
    Output {
        text: String,
    },
    ToolStarted {
        call_id: String,
        tool: String,
    },
    ToolFinished {
        call_id: String,
        tool: String,
        ok: bool,
        summary: String,
    },
    ConsentNeeded {
        call_id: String,
        tool: String,
        title: String,
    },
    TurnDone {
        backend: String,
        input_tokens: u64,
        output_tokens: u64,
        tokens_unknown: bool,
        cost_usd: Option<f64>,
        cost_is_estimate: bool,
    },
    Error {
        message: String,
        remediation: Option<String>,
        log: Option<String>,
    },
    Note {},
    Ask {
        id: String,
        kind: String,
        title: String,
    },
    Checked {
        round: u64,
        check: String,
        passed: bool,
        similarity: Option<f64>,
        note: Option<String>,
    },
    Feature {
        name: String,
        state: String,
        guide: Option<String>,
        note: Option<String>,
        similarity: Option<f64>,
    },
    Preview {
        url: String,
    },
    Finished {
        ok: bool,
        #[serde(default)]
        paused: bool,
        message: String,
    },
    /// Any `type` not listed above (serde's catch-all for internally tagged enums).
    #[serde(other)]
    Other,
}

/// Parses one stdout line. A blank line is `Ok(None)`; anything else must be a JSON object with a `type`.
pub fn parse_line(line: &str) -> Result<Option<Line>, LineError> {
    let text = line.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let value: Value = serde_json::from_str(text).map_err(|e| LineError::NotJson(e.to_string()))?;
    let kind = value.get("type").and_then(Value::as_str).map(str::to_owned).ok_or(LineError::MissingType)?;
    let event: Event = serde_json::from_value(value).map_err(|e| LineError::Malformed { kind: kind.clone(), reason: e.to_string() })?;
    Ok(Some(match event {
        Event::Other => Line::Unknown(kind),
        known => Line::Event(known),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(line: &str) -> Event {
        match parse_line(line) {
            Ok(Some(Line::Event(e))) => e,
            other => panic!("expected an event, got {other:?}"),
        }
    }

    #[test]
    fn step_line_becomes_a_typed_event() {
        let e = event(r#"{"type":"step","step":"build","label":"Build","text":"Building the first version"}"#);
        assert_eq!(e, Event::Step { step: "build".into(), label: "Build".into(), text: "Building the first version".into() });
    }

    #[test]
    fn turn_done_accepts_a_null_cost() {
        let e = event(
            r#"{"type":"turn_done","backend":"Claude Code","input_tokens":10,"output_tokens":4,"tokens_unknown":false,"cost_usd":null,"cost_is_estimate":true}"#,
        );
        assert!(matches!(e, Event::TurnDone { cost_usd: None, input_tokens: 10, .. }));
    }

    #[test]
    fn unknown_type_is_reported_not_failed() {
        assert_eq!(parse_line(r#"{"type":"future_thing","x":1}"#), Ok(Some(Line::Unknown("future_thing".into()))));
    }

    #[test]
    fn blank_line_is_skipped() {
        assert_eq!(parse_line("   "), Ok(None));
    }

    #[test]
    fn malformed_json_is_an_error() {
        assert!(matches!(parse_line("{\"type\":"), Err(LineError::NotJson(_))));
    }

    #[test]
    fn missing_type_is_an_error() {
        assert_eq!(parse_line(r#"{"text":"no type"}"#), Err(LineError::MissingType));
    }

    #[test]
    fn known_type_with_a_missing_field_is_malformed() {
        let err = parse_line(r#"{"type":"step","step":"build"}"#).err();
        assert!(matches!(err, Some(LineError::Malformed { ref kind, .. }) if kind == "step"));
    }

    #[test]
    fn feature_lines_as_main_emits_them_parse() {
        let bare = event(r#"{"type":"feature","name":"Map","state":"in_progress","guide":null,"note":null,"similarity":null}"#);
        assert!(matches!(bare, Event::Feature { ref state, guide: None, similarity: None, .. } if state == "in_progress"));
        let scored = event(r#"{"type":"feature","name":"Map","state":"passed","guide":"guides/map.md","note":"ok","similarity":87}"#);
        assert!(matches!(scored, Event::Feature { similarity: Some(s), .. } if s > 0.0), "an integer similarity is accepted");
    }

    #[test]
    fn finished_defaults_paused_to_false() {
        let e = event(r#"{"type":"finished","ok":true,"message":"done"}"#);
        assert_eq!(e, Event::Finished { ok: true, paused: false, message: "done".into() });
    }
}
