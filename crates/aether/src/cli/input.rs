//! Free-text arguments: words on the command line, or stdin for `-` or an
//! omitted argument.

use anyhow::{Context as _, bail};
use std::io::{IsTerminal as _, Read};
use std::path::Path;

const MAX_SOURCE_LEN: usize = 64;

/// The text the words stand for. `-` alone reads `stdin`; no words read it
/// too when `omitted_reads_stdin` and it is not a terminal.
pub fn text_from_words(
    words: Vec<String>,
    omitted_reads_stdin: bool,
    stdin: &mut impl Read,
    stdin_is_terminal: bool,
) -> anyhow::Result<String> {
    let dash = words.len() == 1 && words[0] == "-";
    let omitted = words.is_empty() && omitted_reads_stdin && !stdin_is_terminal;
    if !dash && !omitted {
        return Ok(words.join(" "));
    }
    let mut text = String::new();
    stdin
        .read_to_string(&mut text)
        .context("read text from stdin")?;
    Ok(text)
}

/// [`text_from_words`] on the process's own stdin.
pub fn text_from_process_stdin(
    words: Vec<String>,
    omitted_reads_stdin: bool,
) -> anyhow::Result<String> {
    let stdin = std::io::stdin();
    let is_terminal = stdin.is_terminal();
    text_from_words(words, omitted_reads_stdin, &mut stdin.lock(), is_terminal)
}

pub fn text_from_file(path: &Path) -> anyhow::Result<String> {
    std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))
}

/// A trigger's source is a short name such as `ci`; anything else is
/// most likely a payload typed into the wrong place.
pub fn validate_trigger_source(source: &str) -> anyhow::Result<()> {
    let name_char = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ':' | '/');
    if source.is_empty() || source.len() > MAX_SOURCE_LEN || !source.chars().all(name_char) {
        bail!(
            "`{source}` is not a valid trigger source (letters, digits and . _ - : /, up to \
             {MAX_SOURCE_LEN}); usage: `trigger AGENT SOURCE --payload JSON`"
        );
    }
    Ok(())
}

pub fn parse_trigger_payload(payload: &str) -> anyhow::Result<serde_json::Value> {
    serde_json::from_str(payload).with_context(|| {
        format!("--payload must be JSON (quote a plain string as '\"text\"'), got `{payload}`")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(list: &[&str]) -> Vec<String> {
        list.iter().map(|w| w.to_string()).collect()
    }

    #[test]
    fn words_join_and_never_touch_stdin() {
        let mut stdin = std::io::Cursor::new(b"ignored".to_vec());
        let text = text_from_words(words(&["a", "b"]), true, &mut stdin, false).unwrap();
        assert_eq!(text, "a b");
        assert_eq!(stdin.position(), 0);
    }

    #[test]
    fn dash_and_omitted_words_read_stdin() {
        for given in [words(&["-"]), words(&[])] {
            let mut stdin = std::io::Cursor::new(b"hi\n".to_vec());
            assert_eq!(
                text_from_words(given, true, &mut stdin, false).unwrap(),
                "hi\n"
            );
        }
    }

    #[test]
    fn omitted_words_leave_a_terminal_or_a_disabled_stdin_alone() {
        let mut stdin = std::io::Cursor::new(b"hi".to_vec());
        assert_eq!(
            text_from_words(words(&[]), true, &mut stdin, true).unwrap(),
            ""
        );
        assert_eq!(
            text_from_words(words(&[]), false, &mut stdin, false).unwrap(),
            ""
        );
        assert_eq!(stdin.position(), 0);
    }

    #[test]
    fn trigger_source_rejects_text_that_is_not_a_name() {
        for good in ["ci", "github:pr/1", "cron.daily"] {
            validate_trigger_source(good).unwrap();
        }
        for bad in ["", "not json", "{\"a\":1}", &"x".repeat(65)] {
            assert!(validate_trigger_source(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn trigger_payload_must_be_json() {
        assert_eq!(
            parse_trigger_payload("null").unwrap(),
            serde_json::Value::Null
        );
        assert_eq!(
            parse_trigger_payload("{\"a\":1}").unwrap()["a"],
            serde_json::json!(1)
        );
        assert!(parse_trigger_payload("plain text").is_err());
    }
}
