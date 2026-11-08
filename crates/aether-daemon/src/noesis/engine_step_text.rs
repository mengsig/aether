//! Decode only the protocol's leading `text` string. Everything after its
//! closing quote, including nested tool text and split secrets, stays private.

#[derive(Default)]
pub(super) struct PublicText {
    header: usize,
    body: bool,
    stopped: bool,
    escape: String,
    pub(super) emitted: String,
}

impl PublicText {
    pub(super) fn push(&mut self, chunk: &str) -> Result<String, &'static str> {
        debug_assert!(self.escape.len() <= 12);
        let mut text = String::new();
        for ch in chunk.chars() {
            if self.stopped {
                break;
            }
            if !self.body {
                self.header(ch);
            } else if !self.escape.is_empty() {
                self.escaped(ch, &mut text)?;
            } else {
                match ch {
                    '"' => self.stopped = true,
                    '\\' => self.escape.push(ch),
                    '\u{0}'..='\u{1f}' => return Err("unescaped control character in text"),
                    _ => text.push(ch),
                }
            }
        }
        self.emitted.push_str(&text);
        debug_assert!(self.emitted.ends_with(&text));
        Ok(text)
    }

    fn header(&mut self, ch: char) {
        const PREFIX: &[u8] = b"{\"text\":\"";
        debug_assert!(self.header < PREFIX.len());
        if matches!(self.header, 0 | 1 | 7 | 8) && matches!(ch, ' ' | '\n' | '\r' | '\t') {
            return;
        }
        if ch != char::from(PREFIX[self.header]) {
            self.stopped = true;
            return;
        }
        self.header += 1;
        self.body = self.header == PREFIX.len();
        debug_assert!(!self.body || ch == '"');
    }

    fn escaped(&mut self, ch: char, text: &mut String) -> Result<(), &'static str> {
        debug_assert!(self.escape.starts_with('\\'));
        if !ch.is_ascii() {
            return Err("invalid character in JSON escape");
        }
        self.escape.push(ch);
        let needed = if self.escape.starts_with("\\u") {
            self.unicode_length()?
        } else {
            2
        };
        if self.escape.len() < needed {
            return Ok(());
        }
        let decoded: String = serde_json::from_str(&format!("\"{}\"", self.escape))
            .map_err(|_| "invalid escape in text")?;
        debug_assert_eq!(decoded.chars().count(), 1);
        text.push_str(&decoded);
        self.escape.clear();
        Ok(())
    }

    fn unicode_length(&self) -> Result<usize, &'static str> {
        debug_assert!(self.escape.starts_with("\\u"));
        debug_assert!(self.escape.len() <= 12);
        if self.escape.len() < 6 {
            return Ok(6);
        }
        let first = self
            .escape
            .get(2..6)
            .ok_or("invalid Unicode escape in text")?;
        let code = u16::from_str_radix(first, 16).map_err(|_| "invalid Unicode escape in text")?;
        Ok(if (0xd800..=0xdbff).contains(&code) {
            12
        } else {
            6
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_chunk_boundary_decodes_text_but_never_nested_text_or_secrets() {
        let raw = r#"{ "text": "Hello\n\"world\" \\ $π$ \uD83D\uDE00", "tool_calls": [{"input": {"text": "PRIVATE", "answer_key": "SECRET", "solution": "PRIVATE-SOLUTION"}}]}"#;
        let expected = "Hello\n\"world\" \\ $π$ 😀";
        for split in raw.char_indices().map(|(index, _)| index) {
            let mut decoder = PublicText::default();
            let first = decoder.push(&raw[..split]).expect("prefix");
            let second = decoder.push(&raw[split..]).expect("suffix");
            assert_eq!(format!("{first}{second}"), expected, "split {split}");
            assert_eq!(decoder.emitted, expected);
        }
        let mut decoder = PublicText::default();
        for ch in raw.chars() {
            decoder.push(&ch.to_string()).expect("character");
        }
        assert_eq!(decoder.emitted, expected);
    }

    #[test]
    fn noncanonical_prefixes_wait_for_complete_validation() {
        for raw in [
            r#"{"tool_calls":[{"text":"SECRET"}],"text":"Public"}"#,
            r#"Here is {"text":"Public"}"#,
            r#"{"textual":"SECRET"}"#,
            r#"{"te xt":"SECRET"}"#,
            r#"{"text": {"text":"SECRET"}}"#,
        ] {
            let mut decoder = PublicText::default();
            assert!(decoder.push(raw).unwrap().is_empty());
            assert!(decoder.emitted.is_empty());
        }
    }

    #[test]
    fn invalid_escapes_fail_without_quoting_private_input() {
        for escape in [r"\x", r"\uZZZZ", r"\uDE00", r"\uD83Dprivate", r"\uD83D😀😀"] {
            let mut decoder = PublicText::default();
            let error = decoder.push(&format!("{{\"text\":\"{escape}"));
            assert!(error.is_err());
            assert!(!error.unwrap_err().contains("private"));
        }
    }
}
