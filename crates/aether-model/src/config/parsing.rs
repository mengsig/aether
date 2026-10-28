use serde::Deserialize;
use toml::de::{DeTable, DeValue};

use super::AetherConfig;

pub(super) fn parse_config(text: &str) -> Result<AetherConfig, String> {
    let mut table = DeTable::parse(text).map_err(|error| syntax_error(text, &error))?;
    let mut unknown = Vec::new();
    for _ in 0..64 {
        match AetherConfig::deserialize(toml::Deserializer::from(table.clone())) {
            Ok(config) if unknown.is_empty() => return Ok(config),
            Ok(_) => break,
            Err(error) => {
                if !error.message().starts_with("unknown field `") {
                    if unknown.is_empty() {
                        return Err(error.to_string());
                    }
                    break;
                }
                let Some(span) = error.span() else {
                    return Err(error.to_string());
                };
                let removed = remove_unknown(table.get_mut(), span.start, "");
                let Some(path) = removed else {
                    return Err(error.to_string());
                };
                debug_assert!(!path.is_empty());
                debug_assert!(span.start <= text.len());
                unknown.push((span.start, path));
            }
        }
    }
    unknown.sort_by_key(|(offset, _)| *offset);
    let mut seen = std::collections::BTreeSet::new();
    unknown.retain(|(_, path)| seen.insert(path.clone()));
    let names = unknown
        .iter()
        .map(|(_, path)| format!("`{path}`"))
        .collect::<Vec<_>>();
    Err(format!("unknown settings {}", names.join(", ")))
}

fn syntax_error(text: &str, error: &toml::de::Error) -> String {
    let offset = error.span().map_or(0, |span| span.start).min(text.len());
    debug_assert!(text.is_char_boundary(offset));
    let prefix = &text[..offset];
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let column = prefix.rsplit('\n').next().unwrap_or("").chars().count() + 1;
    debug_assert!(line > 0 && column > 0);
    let message = error
        .message()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    format!("TOML parse error at line {line}, column {column}: {message}")
}

fn remove_unknown(table: &mut DeTable<'_>, offset: usize, parent: &str) -> Option<String> {
    let mut removal = None;
    for (key, value) in table.iter_mut() {
        let name = key.get_ref();
        let name = if name.is_empty() || name.contains('.') {
            format!("{name:?}")
        } else {
            name.to_string()
        };
        let path = if parent.is_empty() {
            name
        } else {
            format!("{parent}.{name}")
        };
        if key.span().start == offset {
            removal = Some((key.clone(), path));
            break;
        }
        if let Some(path) = remove_nested(value.get_mut(), offset, &path) {
            return Some(path);
        }
    }
    let (key, path) = removal?;
    table.remove(&key)?;
    Some(path)
}

fn remove_nested(value: &mut DeValue<'_>, offset: usize, path: &str) -> Option<String> {
    match value {
        DeValue::Table(table) => remove_unknown(table, offset, path),
        DeValue::Array(values) => values
            .iter_mut()
            .find_map(|value| remove_nested(value.get_mut(), offset, path)),
        _ => None,
    }
}
