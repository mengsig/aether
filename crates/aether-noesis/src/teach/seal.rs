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

/// Replace every scalar in `value`, object keys included, whose text holds
/// a secret by [`SEALED`]; keys that collide once sealed are numbered.
pub(super) fn scrub(value: &mut Value, secrets: &[String]) {
    match value {
        Value::String(text) if holds(text, secrets) => *value = Value::String(SEALED.into()),
        // The parser reads a number, boolean or null as its text, so sealing does too.
        Value::Number(_) | Value::Bool(_) | Value::Null if holds(&value.to_string(), secrets) => {
            *value = Value::String(SEALED.into());
        }
        Value::Array(items) => items.iter_mut().for_each(|item| scrub(item, secrets)),
        Value::Object(map) => {
            let entries = std::mem::take(map);
            for (index, (key, mut item)) in entries.into_iter().enumerate() {
                scrub(&mut item, secrets);
                let key = if holds(&key, secrets) {
                    format!("{SEALED}{index}")
                } else {
                    key
                };
                map.insert(key, item);
            }
            debug_assert!(map.keys().all(|key| !holds(key, secrets)));
        }
        _ => {}
    }
}

/// Whether any scalar or key in `value` holds a secret.
fn reveals(value: &Value, secrets: &[String]) -> bool {
    match value {
        Value::String(text) => holds(text, secrets),
        Value::Array(items) => items.iter().any(|item| reveals(item, secrets)),
        Value::Object(map) => map
            .iter()
            .any(|(key, item)| holds(key, secrets) || reveals(item, secrets)),
        scalar => holds(&scalar.to_string(), secrets),
    }
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
    fn scrubbing_seals_every_scalar_holding_a_secret_at_any_depth() {
        let secrets = vec!["x = 4".to_string()];
        let mut value = json!({"a": "so x = 4.", "b": ["fine", {"c": "X = 4"}], "d": 4});
        scrub(&mut value, &secrets);
        assert_eq!(
            value,
            json!({"a": SEALED, "b": ["fine", {"c": SEALED}], "d": 4})
        );
        let mut scalars = json!({"query": [42], "other": 7, "flag": true, "off": false});
        scrub(&mut scalars, &["42".to_string(), "true".to_string()]);
        assert_eq!(
            scalars,
            json!({"query": [SEALED], "other": 7, "flag": SEALED, "off": false})
        );
        let mut empty = json!({"query": null, "keep": null});
        scrub(&mut empty, &["null".to_string()]);
        assert_eq!(empty, json!({"query": SEALED, "keep": SEALED}));
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
