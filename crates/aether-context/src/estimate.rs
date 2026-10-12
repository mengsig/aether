//! Token estimation without a tokenizer. Deliberately conservative: it
//! overestimates slightly so budgets err on the side of compacting early.

use aether_core::model::{ContentBlock, Message, ToolResultBlock, ToolSpec};

/// What a stored image costs: the providers scale one down to about this
/// many tokens whatever its file size.
const STORED_IMAGE_TOKENS: u32 = 1_600;

#[derive(Debug, Clone, Copy, Default)]
pub struct TokenEstimator;

impl TokenEstimator {
    /// Roughly 3.6 characters per token for mixed prose and code.
    pub fn text(&self, text: &str) -> u32 {
        (text.len() as f64 / 3.6).ceil() as u32
    }

    pub fn message(&self, message: &Message) -> u32 {
        let mut total = 4; // role and framing overhead
        for block in &message.content {
            total += match block {
                ContentBlock::Text { text } => self.text(text),
                // No request builder replays unsigned reasoning (e.g. Codex's).
                ContentBlock::Thinking {
                    signature: None,
                    redacted: false,
                    ..
                } => 0,
                ContentBlock::Thinking {
                    text, signature, ..
                } => self.text(text) + signature.as_deref().map(|s| self.text(s)).unwrap_or(0),
                ContentBlock::ToolUse { name, input, .. } => {
                    self.text(name) + self.text(&input.to_string()) + 8
                }
                ContentBlock::ToolResult { content, .. } => {
                    8 + content
                        .iter()
                        .map(|c| match c {
                            ToolResultBlock::Text { text } => self.text(text),
                            ToolResultBlock::Image { data, .. } => (data.len() / 100) as u32 + 100,
                        })
                        .sum::<u32>()
                }
                ContentBlock::Image { data, .. } => (data.len() / 100) as u32 + 100,
                ContentBlock::ImageRef(_) => STORED_IMAGE_TOKENS,
            };
        }
        total
    }

    pub fn messages(&self, messages: &[Message]) -> u32 {
        messages.iter().map(|m| self.message(m)).sum()
    }

    pub fn tools(&self, tools: &[ToolSpec]) -> u32 {
        tools
            .iter()
            .map(|t| {
                self.text(&t.name)
                    + self.text(&t.description)
                    + self.text(&t.input_schema.to_string())
                    + 12
            })
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimates_scale_with_length() {
        let e = TokenEstimator;
        assert!(e.text("") == 0);
        assert!(e.text("hello world") >= 3);
        let short = Message::user_text("hi");
        let long = Message::user_text("word ".repeat(200));
        assert!(e.message(&long) > e.message(&short) * 10);
    }

    #[test]
    fn only_reasoning_a_provider_is_sent_counts() {
        let e = TokenEstimator;
        let thinking = |signature: Option<&str>| ContentBlock::Thinking {
            text: "reasoning ".repeat(3_600),
            signature: signature.map(str::to_owned),
            redacted: false,
        };
        let with = |block| Message {
            content: vec![
                block,
                ContentBlock::Text {
                    text: "done".into(),
                },
            ],
            ..Message::assistant_text("done")
        };
        let plain = e.message(&Message::assistant_text("done"));
        assert_eq!(e.message(&with(thinking(None))), plain);
        assert!(e.message(&with(thinking(Some("sig")))) > plain + 9_000);
    }
}
