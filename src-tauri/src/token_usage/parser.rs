use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{TokenCounts, TokenEvent};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseResult {
    pub events: Vec<TokenEvent>,
    pub malformed_lines: usize,
    pub unknown_events: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct ParserContext {
    pub(super) session_id: Option<String>,
    pub(super) model: Option<String>,
    pub(super) next_event_ordinal: u64,
    pub(super) anchor_hash: Option<String>,
    pub(super) file_identity: Option<String>,
}

pub(super) enum ParsedLine {
    Event(TokenEvent),
    Known,
    Unknown,
    Malformed,
}

pub fn parse_jsonl(input: &str, fallback_session_id: &str) -> ParseResult {
    let mut context = ParserContext::default();
    let mut result = ParseResult {
        events: Vec::new(),
        malformed_lines: 0,
        unknown_events: 0,
    };
    for line in input.lines() {
        match parse_line(line.as_bytes(), &mut context, fallback_session_id) {
            ParsedLine::Event(event) => result.events.push(event),
            ParsedLine::Known => {}
            ParsedLine::Unknown => result.unknown_events += 1,
            ParsedLine::Malformed => result.malformed_lines += 1,
        }
    }
    result
}

pub(super) fn parse_line(line: &[u8], context: &mut ParserContext, fallback: &str) -> ParsedLine {
    if line.iter().all(u8::is_ascii_whitespace) {
        return ParsedLine::Known;
    }
    let value: Value = match serde_json::from_slice(line) {
        Ok(value) => value,
        Err(_) => return ParsedLine::Malformed,
    };
    let event_type = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let payload = value.get("payload").unwrap_or(&Value::Null);
    match event_type {
        "session_meta" => {
            if let Some(id) = payload
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
            {
                context.session_id = Some(id.to_string());
            }
            ParsedLine::Known
        }
        "turn_context" => {
            if let Some(model) = payload
                .get("model")
                .and_then(Value::as_str)
                .filter(|model| !model.is_empty())
            {
                context.model = Some(model.to_string());
            }
            ParsedLine::Known
        }
        "event_msg" if payload.get("type").and_then(Value::as_str) == Some("token_count") => {
            let Some(usage) = payload
                .get("info")
                .and_then(|info| info.get("last_token_usage"))
                .filter(|usage| usage.is_object())
            else {
                // Cumulative usage is reconciliation metadata, not a second request event.
                return ParsedLine::Known;
            };
            let observed_at = value
                .get("timestamp")
                .and_then(Value::as_str)
                .and_then(|timestamp| DateTime::parse_from_rfc3339(timestamp).ok())
                .map(|timestamp| {
                    timestamp
                        .with_timezone(&Utc)
                        .to_rfc3339_opts(SecondsFormat::Millis, true)
                });
            let Some(observed_at) = observed_at else {
                return ParsedLine::Malformed;
            };
            let input_tokens = usage_u64(usage, "input_tokens");
            let output_tokens = usage_u64(usage, "output_tokens");
            let counts = TokenCounts {
                input_tokens,
                cached_input_tokens: usage_u64(usage, "cached_input_tokens"),
                cache_write_input_tokens: usage_u64(usage, "cache_write_input_tokens")
                    .max(usage_u64(usage, "cache_creation_input_tokens")),
                output_tokens,
                reasoning_output_tokens: usage_u64(usage, "reasoning_output_tokens"),
                total_tokens: input_tokens.saturating_add(output_tokens),
            };
            let event_ordinal = context.next_event_ordinal;
            context.next_event_ordinal = context.next_event_ordinal.saturating_add(1);
            ParsedLine::Event(TokenEvent {
                session_id: context
                    .session_id
                    .clone()
                    .unwrap_or_else(|| fallback.to_string()),
                model: context
                    .model
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string()),
                observed_at,
                counts,
                event_ordinal,
            })
        }
        _ => ParsedLine::Unknown,
    }
}

fn usage_u64(usage: &Value, key: &str) -> u64 {
    usage.get(key).and_then(Value::as_u64).unwrap_or(0)
}
