use super::{
    AgentId, Command, Deserialize, ErrorReply, Event, InvocationId, Reply, Serialize, Usage,
};

/// Client to daemon. The request id is `req` on the wire so it can never
/// collide with a command's own `id` field after flattening.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ClientFrame {
    #[serde(rename = "req")]
    pub id: u64,
    #[serde(flatten)]
    pub command: Command,
}

/// Transient streaming signal. Not persisted; durable events reconcile it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "delta", rename_all = "snake_case")]
pub enum Delta {
    TurnStarted {
        turn: u32,
    },
    Text {
        text: String,
    },
    Thinking {
        text: String,
    },
    ToolInput {
        name: String,
        partial_json: String,
    },
    /// Absolute in-memory figures. Replace prior totals; never add these to durable usage.
    Accounting {
        total: Usage,
        turn_usage: Usage,
        context_tokens: Option<u64>,
    },
    TurnFinished,
}

/// Daemon to client.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "frame", rename_all = "snake_case")]
pub enum ServerFrame {
    Reply {
        /// The request this answers. Request ids start at 1, so **0 is
        /// reserved**: it is the daemon speaking on its own account rather
        /// than answering anything - the reason a connection is about to
        /// close, or a frame that did not decode. A client keeps it and
        /// reports it instead of a bare disconnect; it never resolves a
        /// waiting request.
        id: u64,
        result: Result<Reply, ErrorReply>,
    },
    Event {
        sub: u64,
        event: Event,
    },
    Delta {
        sub: u64,
        agent: AgentId,
        invocation: InvocationId,
        delta: Delta,
    },
    /// The remote carrier speaking first: what a device signs to
    /// authenticate its hello. Never sent on the socket or the token listener.
    Challenge {
        /// 32 random bytes, base64url without padding. Single use.
        nonce: String,
        instance_id: String,
    },
}

/// Encode one frame as a single JSON line.
pub fn encode<T: Serialize>(frame: &T) -> Result<String, serde_json::Error> {
    let mut line = serde_json::to_string(frame)?;
    line.push('\n');
    Ok(line)
}

/// Decode one JSON line.
pub fn decode<'a, T: Deserialize<'a>>(line: &'a str) -> Result<T, serde_json::Error> {
    serde_json::from_str(line.trim_end_matches(['\r', '\n']))
}
