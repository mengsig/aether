//! The loop's last line of sealing: every text it records, shows or delivers
//! is checked against the sealed texts not yet revealed.

use serde_json::{Value, json};

use super::engine::Step;
use super::question::contains_phrase;
use crate::tools::{self, SEALED};

/// What a step's visible text reads as once it gave away a sealed answer.
pub const WITHHELD: &str = "<withheld: this message gave away a sealed answer>";

/// What a refusal reads as once its reason would repeat a sealed text.
pub const REFUSAL_WITHHELD: &str =
    "this call was refused; its reason is withheld because it repeats sealed text";

/// Whether `text` holds any of `secrets` as a whole phrase, ignoring case.
pub(super) fn holds(text: &str, secrets: &[String]) -> bool {
    if secrets.is_empty() {
        return false;
    }
    let haystack = text.to_lowercase();
    secrets
        .iter()
        .any(|secret| contains_phrase(&haystack, &secret.trim().to_lowercase()))
}

/// Replace `value` whole by [`SEALED`] when any part of it holds a secret.
/// Masking only the matching parts would show where a secret sits, which
/// among visible alternatives (a choice's options) names it.
pub(super) fn withhold(value: &mut Value, secrets: &[String]) {
    if reveals(value, secrets) {
        *value = Value::String(SEALED.into());
    }
    debug_assert!(!reveals(value, secrets) || value.as_str() == Some(SEALED));
}

/// Whether any scalar or key in `value` holds a secret; the parser reads a
/// number, boolean or null as its text and text holding JSON as that JSON,
/// so this does too.
fn reveals(value: &Value, secrets: &[String]) -> bool {
    match value {
        Value::String(text) => {
            holds(text, secrets) || embedded_json(text).is_some_and(|v| reveals(&v, secrets))
        }
        Value::Array(items) => items.iter().any(|item| reveals(item, secrets)),
        Value::Object(map) => map
            .iter()
            .any(|(key, item)| holds(key, secrets) || reveals(item, secrets)),
        scalar => holds(&scalar.to_string(), secrets),
    }
}

/// The JSON object or array `text` holds, as the argument healer parses it;
/// its escapes (`\u03c0`) decode, so a secret can hide in them. Each nested
/// level is strictly shorter text, so the recursion ends.
fn embedded_json(text: &str) -> Option<Value> {
    let trimmed = text.trim();
    if !(trimmed.starts_with('[') || trimmed.starts_with('{')) {
        return None;
    }
    let parsed: Value = serde_json::from_str(trimmed).ok()?;
    (parsed.is_object() || parsed.is_array()).then_some(parsed)
}

/// A refusal quotes the model's own names and ids, so one that holds a
/// secret is replaced whole by [`REFUSAL_WITHHELD`].
pub(super) fn seal_refusal(content: &mut Value, secrets: &[String]) {
    debug_assert!(
        content.get("error").is_some(),
        "only refusals are sealed whole"
    );
    if reveals(content, secrets) {
        *content = json!({ "error": REFUSAL_WITHHELD });
    }
    debug_assert!(content.is_object());
}

/// `text` itself, or [`SEALED`] when it holds a secret.
pub(super) fn sealed_text(text: &str, secrets: &[String]) -> String {
    if holds(text, secrets) {
        SEALED.to_string()
    } else {
        text.to_string()
    }
}

/// The sealed texts the calls of a step carry; `None` when any sealed input
/// is unreadable, so the caller fails closed.
pub(super) fn step_secrets(step: &Step) -> Option<Vec<String>> {
    let mut secrets = Vec::new();
    for call in &step.calls {
        secrets.extend(tools::sealed_texts(&call.name, &call.input)?);
    }
    debug_assert!(secrets.iter().all(|secret| !secret.trim().is_empty()));
    Some(secrets)
}

/// A step's text as it may be shown: withheld when it holds a secret or
/// when the secrets could not be read.
pub(super) fn shown_text(step: &Step, secrets: Option<&[String]>) -> String {
    match secrets {
        Some(secrets) if !holds(&step.text, secrets) => step.text.clone(),
        _ => WITHHELD.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_value_holding_a_secret_anywhere_is_withheld_whole() {
        let cases = [
            (
                json!({"a": "fine", "b": ["fine", {"c": "so X = 4."}]}),
                "x = 4",
            ),
            (json!({"query": [42], "other": 7}), "42"),
            (json!({"flag": true}), "true"),
            (json!({"query": null}), "null"),
            (json!({"query": "vocabulary", "sealed": true}), "sealed"),
        ];
        for (mut value, secret) in cases {
            withhold(&mut value, &[secret.to_string()]);
            assert_eq!(value, json!(SEALED), "{secret}");
        }
        let escaped = Value::String(r#"{"query":"\u03c0"}"#.into());
        let nested = json!({"nodes": r#"[{"title": "[\"\\u03c0\"]"}]"#});
        for mut value in [escaped, nested] {
            withhold(&mut value, &["π".to_string()]);
            assert_eq!(value, json!(SEALED));
        }
        let mut kept = json!({"query": "x = 5", "n": 4});
        withhold(&mut kept, &["x = 4".to_string()]);
        assert_eq!(kept, json!({"query": "x = 5", "n": 4}));
        assert!(!holds("anything", &[]));
    }

    #[test]
    fn a_refusal_holding_a_secret_is_withheld_whole() {
        let secrets = vec!["mitochondria".to_string()];
        let mut refusal = json!({"error": "node `mitochondria`: duplicate id"});
        seal_refusal(&mut refusal, &secrets);
        assert_eq!(refusal, json!({"error": REFUSAL_WITHHELD}));
        let mut kept = json!({"error": "node `cell-wall`: duplicate id"});
        seal_refusal(&mut kept, &secrets);
        assert_eq!(kept, json!({"error": "node `cell-wall`: duplicate id"}));
    }
}
