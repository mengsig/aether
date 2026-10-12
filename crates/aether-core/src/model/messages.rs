use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::attachment::AttachmentRef;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    User,
    Assistant,
}

/// One turn of conversation as the model sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Message {
    pub role: MessageRole,
    pub content: Vec<ContentBlock>,
}

impl Message {
    pub fn user_text(text: impl Into<String>) -> Self {
        Self {
            role: MessageRole::User,
            content: vec![ContentBlock::Text { text: text.into() }],
        }
    }

    pub fn assistant_text(text: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: vec![ContentBlock::Text { text: text.into() }],
        }
    }

    pub fn user_blocks(content: Vec<ContentBlock>) -> Self {
        Self {
            role: MessageRole::User,
            content,
        }
    }

    pub fn assistant_blocks(content: Vec<ContentBlock>) -> Self {
        Self {
            role: MessageRole::Assistant,
            content,
        }
    }

    /// Concatenated text of all text blocks.
    pub fn text(&self) -> String {
        text_of(&self.content)
    }

    /// True when every block is a tool result.
    pub fn is_tool_results_only(&self) -> bool {
        !self.content.is_empty()
            && self
                .content
                .iter()
                .all(|b| matches!(b, ContentBlock::ToolResult { .. }))
    }
}

/// Concatenate the text blocks of a content list.
pub fn text_of(content: &[ContentBlock]) -> String {
    let mut out = String::new();
    for block in content {
        if let ContentBlock::Text { text } = block {
            out.push_str(text);
        }
    }
    out
}

/// A block inside a message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    Thinking {
        text: String,
        /// Provider-opaque data needed to replay the block on a later turn.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
        #[serde(default)]
        redacted: bool,
    },
    ToolUse {
        /// Provider-issued call id, echoed back in the matching tool result.
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: Vec<ToolResultBlock>,
        #[serde(default)]
        is_error: bool,
    },
    Image {
        media_type: String,
        /// Base64 payload.
        data: String,
    },
    /// An image a person sent with a message, kept by the daemon. Durable
    /// state carries this reference; the bytes are read back only to build
    /// a model request.
    ImageRef(AttachmentRef),
}

impl ContentBlock {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    pub fn tool_result_text(
        tool_use_id: impl Into<String>,
        text: impl Into<String>,
        is_error: bool,
    ) -> Self {
        Self::ToolResult {
            tool_use_id: tool_use_id.into(),
            content: vec![ToolResultBlock::Text { text: text.into() }],
            is_error,
        }
    }
}

/// Content allowed inside a tool result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolResultBlock {
    Text { text: String },
    Image { media_type: String, data: String },
}
