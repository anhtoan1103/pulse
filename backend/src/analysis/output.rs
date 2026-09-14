//! Structured AI output (docs/pulse-ai-design.md #3): JSON schema sent to
//! the provider, and strict validation of what comes back before anything is
//! stored in `incidents`.
//!
//! Wrong types, missing fields, an unknown `confidence` or an empty
//! `possible_cause` are rejected (→ one retry, then `failed`). Merely
//! over-long output is trimmed instead: >5 list items are cut to 5, long
//! strings truncated.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const MAX_ITEMS: usize = 5;
pub const MAX_CAUSE_LEN: usize = 500;
pub const MAX_ITEM_LEN: usize = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    High,
    Medium,
    Low,
}

impl Confidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiAnalysis {
    pub possible_cause: String,
    pub confidence: Confidence,
    pub evidence: Vec<String>,
    pub suggested_steps: Vec<String>,
}

/// JSON schema for `response_format` (OpenAI-compatible structured output).
/// Sticks to widely supported keywords (no `maxItems`: some providers'
/// strict modes reject it) — limits are stated in descriptions and enforced
/// by [`parse_analysis`].
pub fn json_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "possible_cause": {
                "type": "string",
                "description": "1-2 sentences: the most likely cause, based only on the provided data."
            },
            "confidence": { "type": "string", "enum": ["high", "medium", "low"] },
            "evidence": {
                "type": "array",
                "items": { "type": "string" },
                "description": "At most 5 short one-line facts taken directly from the provided data."
            },
            "suggested_steps": {
                "type": "array",
                "items": { "type": "string" },
                "description": "At most 5 short, concrete investigation actions."
            }
        },
        "required": ["possible_cause", "confidence", "evidence", "suggested_steps"],
        "additionalProperties": false
    })
}

/// Parses model output into a validated [`AiAnalysis`]. The error string is
/// safe to log (it never echoes the model output itself).
pub fn parse_analysis(raw: &str) -> Result<AiAnalysis, String> {
    let json = strip_code_fence(raw.trim());
    let mut analysis: AiAnalysis = serde_json::from_str(json).map_err(|e| {
        // Category + position only: serde's message can quote output values.
        format!(
            "output does not match schema ({:?} error at line {}, column {})",
            e.classify(),
            e.line(),
            e.column()
        )
    })?;

    analysis.possible_cause = clean(&analysis.possible_cause, MAX_CAUSE_LEN);
    if analysis.possible_cause.is_empty() {
        return Err("possible_cause is empty".into());
    }
    analysis.evidence = clean_list(analysis.evidence);
    analysis.suggested_steps = clean_list(analysis.suggested_steps);
    Ok(analysis)
}

/// Some providers wrap JSON in a markdown fence even in JSON mode.
fn strip_code_fence(text: &str) -> &str {
    let Some(rest) = text.strip_prefix("```") else {
        return text;
    };
    let rest = rest.strip_prefix("json").unwrap_or(rest);
    rest.strip_suffix("```").unwrap_or(rest).trim()
}

fn clean_list(items: Vec<String>) -> Vec<String> {
    items
        .iter()
        .map(|item| clean(item, MAX_ITEM_LEN))
        .filter(|item| !item.is_empty())
        .take(MAX_ITEMS)
        .collect()
}

/// Control chars → space, whitespace collapsed, truncated (char-safe).
fn clean(text: &str, max_chars: usize) -> String {
    let normalized: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if normalized.chars().count() <= max_chars {
        normalized
    } else {
        let mut cut: String = normalized.chars().take(max_chars - 1).collect();
        cut.push('…');
        cut
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid() -> Value {
        json!({
            "possible_cause": "The upstream database is saturated.",
            "confidence": "medium",
            "evidence": ["avg latency 320ms → 2800ms"],
            "suggested_steps": ["Check DB connection pool usage"]
        })
    }

    #[test]
    fn parses_valid_output() {
        let a = parse_analysis(&valid().to_string()).unwrap();
        assert_eq!(a.confidence, Confidence::Medium);
        assert_eq!(a.evidence, ["avg latency 320ms → 2800ms"]);
    }

    #[test]
    fn accepts_fenced_json_and_empty_lists() {
        let mut v = valid();
        v["evidence"] = json!([]);
        v["confidence"] = json!("low");
        let a = parse_analysis(&format!("```json\n{v}\n```")).unwrap();
        assert!(a.evidence.is_empty());
    }

    #[test]
    fn trims_overlong_output_instead_of_failing() {
        let mut v = valid();
        v["evidence"] = json!((0..8).map(|i| format!("fact {i}")).collect::<Vec<_>>());
        v["suggested_steps"] = json!(["  ", "x".repeat(1000)]);
        v["possible_cause"] = json!(format!("line1\nline2 {}", "y".repeat(1000)));
        let a = parse_analysis(&v.to_string()).unwrap();
        assert_eq!(a.evidence.len(), MAX_ITEMS);
        assert_eq!(a.suggested_steps.len(), 1, "blank items dropped");
        assert_eq!(a.suggested_steps[0].chars().count(), MAX_ITEM_LEN);
        assert!(a.possible_cause.starts_with("line1 line2"));
        assert_eq!(a.possible_cause.chars().count(), MAX_CAUSE_LEN);
    }

    #[test]
    fn rejects_schema_violations() {
        let mut missing = valid();
        missing.as_object_mut().unwrap().remove("suggested_steps");
        let mut bad_confidence = valid();
        bad_confidence["confidence"] = json!("certain");
        let mut wrong_type = valid();
        wrong_type["evidence"] = json!("not a list");
        let mut extra = valid();
        extra["action"] = json!("delete the database");
        let mut empty_cause = valid();
        empty_cause["possible_cause"] = json!("   ");

        for (name, v) in [
            ("missing", missing),
            ("confidence", bad_confidence),
            ("type", wrong_type),
            ("extra", extra),
            ("empty cause", empty_cause),
        ] {
            assert!(parse_analysis(&v.to_string()).is_err(), "{name}");
        }
        assert!(parse_analysis("The cause is probably the DB.").is_err());
    }

    #[test]
    fn error_messages_do_not_echo_output() {
        let err = parse_analysis("SECRET-LOOKING free text").unwrap_err();
        assert!(!err.contains("SECRET"), "{err}");
        let err = parse_analysis(
            r#"{"possible_cause":"x","confidence":"SECRET","evidence":[],"suggested_steps":[]}"#,
        )
        .unwrap_err();
        assert!(!err.contains("SECRET"), "{err}");
    }
}
