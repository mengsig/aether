//! The loop's last line of sealing: every text it records, shows or delivers
//! is checked against the sealed texts not yet revealed.

use serde_json::Value;

use super::engine::Step;
use super::question::contains_phrase;
use crate::tools::{self, SEALED};

/// What a step's visible text reads as once it gave away a sealed answer.
pub const WITHHELD: &str = "<withheld: this message gave away a sealed answer>";

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

/// Replace every string in `value` that holds a secret by [`SEALED`].
pub(super) fn scrub(value: &mut Value, secrets: &[String]) {
    match value {
        Value::String(text) if holds(text, secrets) => *value = Value::String(SEALED.into()),
        Value::Array(items) => items.iter_mut().for_each(|item| scrub(item, secrets)),
        Value::Object(map) => map.values_mut().for_each(|item| scrub(item, secrets)),
        _ => {}
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
    fn scrubbing_seals_every_string_holding_a_secret_at_any_depth() {
        let secrets = vec!["x = 4".to_string()];
        let mut value = json!({"a": "so x = 4.", "b": ["fine", {"c": "X = 4"}], "d": 4});
        scrub(&mut value, &secrets);
        assert_eq!(
            value,
            json!({"a": SEALED, "b": ["fine", {"c": SEALED}], "d": 4})
        );
        assert!(!holds("anything", &[]));
    }
}
