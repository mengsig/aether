//! Output bounding and redaction: large outputs become artifacts, and the
//! excerpt that reaches the model and the event log is redacted.
//!
//! The artifact is redacted too: the runtime's artifact sink masks the full
//! output before it is stored, so `read_artifact` and every device reading it
//! see the masked copy (ARCHITECTURE §13.2).

pub use aether_core::redact::Redactor;

use aether_core::artifact::ArtifactKind;
use aether_core::id::ArtifactId;

use crate::tool::{ToolContext, ToolError};

/// How much tool output may enter the model context directly.
#[derive(Debug, Clone, Copy)]
pub struct OutputPolicy {
    pub max_chars: usize,
    /// Keep both the start and the end of an oversized output.
    pub head_tail: bool,
}

impl Default for OutputPolicy {
    fn default() -> Self {
        Self {
            max_chars: 30_000,
            head_tail: true,
        }
    }
}

/// A bounded output plus the artifacts created to hold the rest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundedOutput {
    pub text: String,
    pub artifacts: Vec<ArtifactId>,
    pub truncated: bool,
}

impl OutputPolicy {
    /// Bound `text`, storing the full content as an artifact when it is too large.
    pub async fn bound(
        &self,
        text: String,
        name: &str,
        kind: ArtifactKind,
        ctx: &ToolContext,
    ) -> Result<BoundedOutput, ToolError> {
        let total = text.chars().count();
        if total <= self.max_chars {
            return Ok(BoundedOutput {
                text,
                artifacts: Vec::new(),
                truncated: false,
            });
        }
        let summary = format!("full output of {name} ({total} chars)");
        let artifact = ctx
            .artifacts
            .store(
                kind,
                name,
                &summary,
                Some("text/plain"),
                text.clone().into_bytes(),
                ctx.agent_id,
                ctx.task_id,
            )
            .await?;
        let mut excerpt = self.excerpt(&text, total);
        excerpt.push_str(&format!(
            "\n[truncated: {total} chars stored as artifact {}; use read_artifact to retrieve pages]",
            artifact.id
        ));
        Ok(BoundedOutput {
            text: excerpt,
            artifacts: vec![artifact.id],
            truncated: true,
        })
    }

    /// Bound `text` without storing anything; for content that already lives
    /// durably elsewhere (a file on disk).
    pub fn bound_plain(&self, text: &str, note: &str) -> BoundedOutput {
        let total = text.chars().count();
        if total <= self.max_chars {
            return BoundedOutput {
                text: text.to_owned(),
                artifacts: Vec::new(),
                truncated: false,
            };
        }
        let mut excerpt = self.excerpt(text, total);
        excerpt.push_str(&format!("\n[truncated: {total} chars; {note}]"));
        BoundedOutput {
            text: excerpt,
            artifacts: Vec::new(),
            truncated: true,
        }
    }

    fn excerpt(&self, text: &str, total_chars: usize) -> String {
        if self.head_tail {
            let half = self.max_chars / 2;
            let head_end = byte_offset_at_char(text, half);
            let tail_start = byte_offset_at_char(text, total_chars - half);
            format!(
                "{}\n... [{} chars omitted] ...\n{}",
                &text[..head_end],
                total_chars - 2 * half,
                &text[tail_start..]
            )
        } else {
            text[..byte_offset_at_char(text, self.max_chars)].to_owned()
        }
    }
}

fn byte_offset_at_char(text: &str, chars: usize) -> usize {
    text.char_indices()
        .nth(chars)
        .map(|(i, _)| i)
        .unwrap_or(text.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{MemorySink, ctx_in};

    #[tokio::test]
    async fn small_output_passes_through() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_in(dir.path());
        let out = OutputPolicy::default()
            .bound("hello".into(), "cmd", ArtifactKind::CommandOutput, &ctx)
            .await
            .unwrap();
        assert_eq!(out.text, "hello");
        assert!(!out.truncated);
        assert!(out.artifacts.is_empty());
    }

    #[tokio::test]
    async fn large_output_is_stored_and_excerpted() {
        let dir = tempfile::tempdir().unwrap();
        let sink = std::sync::Arc::new(MemorySink::default());
        let mut ctx = ctx_in(dir.path());
        ctx.artifacts = sink.clone();
        let text: String = (0..1000).map(|i| format!("line {i}\n")).collect();
        let policy = OutputPolicy {
            max_chars: 200,
            head_tail: true,
        };
        let out = policy
            .bound(text.clone(), "cmd", ArtifactKind::CommandOutput, &ctx)
            .await
            .unwrap();
        assert!(out.truncated);
        assert_eq!(out.artifacts.len(), 1);
        assert!(out.text.starts_with("line 0\n"));
        assert!(out.text.contains("line 999"));
        assert!(out.text.contains("stored as artifact art_"));
        let stored = sink.stored();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].1, text.into_bytes());
    }

    #[test]
    fn plain_bound_respects_char_boundaries() {
        let policy = OutputPolicy {
            max_chars: 4,
            head_tail: false,
        };
        let out = policy.bound_plain("héllo wörld", "note");
        assert!(out.truncated);
        assert!(out.text.starts_with("héll"));
    }
}
