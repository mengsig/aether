//! Healing of the malformed tool arguments models commonly send (T12): an
//! array or object passed as JSON text, a quoted or spaced enum name, a
//! number or boolean passed as text. Healing only reshapes; it never invents
//! a value, and whatever still does not fit is reported by the parser.

use serde_json::{Map, Value};

#[derive(Debug, Clone, Copy)]
pub(crate) enum Kind {
    Text,
    Enum(&'static [&'static str]),
    Int,
    /// A hint rung, also accepted as `R0`..`R5`.
    Rung,
    Bool,
    Texts,
    /// A list of objects with these fields; a bare text element is kept.
    Objects(&'static [Field]),
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Field {
    pub name: &'static str,
    pub kind: Kind,
}

pub(crate) const fn field(name: &'static str, kind: Kind) -> Field {
    Field { name, kind }
}

/// Heal a whole argument object. Text that holds a JSON object is parsed;
/// `null` fields are dropped, so they read as absent.
pub(crate) fn heal_object(value: Value, fields: &[Field]) -> Value {
    let value = parse_text_as(value, Value::is_object);
    let Value::Object(map) = value else {
        return value;
    };
    let mut healed = Map::with_capacity(map.len());
    for (name, raw) in map {
        if raw.is_null() {
            continue;
        }
        let kind = fields.iter().find(|field| field.name == name);
        let fixed = match kind {
            Some(field) => heal_value(raw, field.kind),
            None => raw,
        };
        healed.insert(name, fixed);
    }
    debug_assert!(healed.values().all(|value| !value.is_null()));
    Value::Object(healed)
}

fn heal_value(value: Value, kind: Kind) -> Value {
    match kind {
        Kind::Text => number_as_text(value),
        Kind::Enum(names) => heal_enum(value, names),
        Kind::Int => heal_int(value, false),
        Kind::Rung => heal_int(value, true),
        Kind::Bool => heal_bool(value),
        // A lone text, as a recall key or a single concept is, is a list of one.
        Kind::Texts => match parse_text_as(number_as_text(value), Value::is_array) {
            Value::Array(items) => Value::Array(items.into_iter().map(number_as_text).collect()),
            Value::String(text) => Value::Array(vec![Value::String(text)]),
            other => other,
        },
        Kind::Objects(fields) => match parse_text_as(value, |v| v.is_array() || v.is_object()) {
            Value::Array(items) => {
                let healed = items.into_iter().map(|item| heal_element(item, fields));
                Value::Array(healed.collect())
            }
            Value::Object(map) => Value::Array(vec![heal_object(Value::Object(map), fields)]),
            other => other,
        },
    }
}

fn heal_element(item: Value, fields: &[Field]) -> Value {
    let item = parse_text_as(item, Value::is_object);
    if item.is_object() {
        heal_object(item, fields)
    } else {
        item
    }
}

/// Replace text by the JSON it holds when that JSON has the wanted shape.
fn parse_text_as(value: Value, wanted: impl Fn(&Value) -> bool) -> Value {
    let Value::String(text) = &value else {
        return value;
    };
    let trimmed = text.trim();
    if !(trimmed.starts_with('[') || trimmed.starts_with('{')) {
        return value;
    }
    match serde_json::from_str::<Value>(trimmed) {
        Ok(parsed) if wanted(&parsed) => parsed,
        _ => value,
    }
}

fn number_as_text(value: Value) -> Value {
    match value {
        Value::Number(number) => Value::String(number.to_string()),
        other => other,
    }
}

/// Strip quotes and backticks a model wrapped around a word, then trim.
fn unquote(text: &str) -> &str {
    let quotes: &[char] = &['"', '\'', '`'];
    text.trim().trim_matches(quotes).trim()
}

fn heal_enum(value: Value, names: &[&str]) -> Value {
    let Value::String(text) = &value else {
        return value;
    };
    let normalized: String = unquote(text)
        .chars()
        .map(|c| match c {
            ' ' | '-' => '_',
            c => c.to_ascii_lowercase(),
        })
        .collect();
    if names.contains(&normalized.as_str()) {
        Value::String(normalized)
    } else {
        value
    }
}

fn heal_int(value: Value, rung: bool) -> Value {
    let parsed = match &value {
        Value::String(text) => {
            let digits = unquote(text);
            let digits = if rung {
                digits.trim_start_matches(['R', 'r'])
            } else {
                digits
            };
            digits.parse::<u64>().ok()
        }
        Value::Number(number) => number
            .as_f64()
            .filter(|float| float.fract() == 0.0 && *float >= 0.0 && *float <= u32::MAX as f64)
            .map(|float| float as u64),
        _ => None,
    };
    parsed.map_or(value, Value::from)
}

fn heal_bool(value: Value) -> Value {
    let Value::String(text) = &value else {
        return value;
    };
    match unquote(text).to_ascii_lowercase().as_str() {
        "true" | "yes" => Value::Bool(true),
        "false" | "no" => Value::Bool(false),
        _ => value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const MODES: &[&str] = &["solved", "solved_with_hints"];
    const FIELDS: &[Field] = &[
        field("outcome", Kind::Enum(MODES)),
        field("rung", Kind::Rung),
        field("retest", Kind::Bool),
        field("targets", Kind::Texts),
        field("limit", Kind::Int),
        field("edges", Kind::Objects(&[field("from", Kind::Text)])),
    ];

    #[test]
    fn heals_the_common_malformations() {
        let input = json!({
            "outcome": "\"Solved With Hints\"",
            "rung": "R3",
            "retest": "'true'",
            "targets": "[\"p_a\", \"p_b\"]",
            "limit": "7",
            "edges": "[{\"from\": 1}]",
            "unknown": "kept",
            "dropped": null,
        });
        let healed = heal_object(input, FIELDS);
        assert_eq!(
            healed,
            json!({
                "outcome": "solved_with_hints",
                "rung": 3,
                "retest": true,
                "targets": ["p_a", "p_b"],
                "limit": 7,
                "edges": [{"from": "1"}],
                "unknown": "kept",
            })
        );
    }

    #[test]
    fn a_whole_stringified_object_is_parsed_and_bad_values_are_left_for_the_parser() {
        let input = Value::String(r#"{"outcome": "won", "rung": "high", "targets": "p_a"}"#.into());
        let healed = heal_object(input, FIELDS);
        assert_eq!(
            healed,
            json!({"outcome": "won", "rung": "high", "targets": ["p_a"]})
        );
        assert_eq!(heal_object(json!(3), FIELDS), json!(3));
    }
}
