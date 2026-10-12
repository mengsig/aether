use crate::id::{AgentId, ApprovalRuleId, DeviceId, ToolCallId};
use serde::{Deserialize, Serialize};

/// Server-observed connection identity, never a bearer token or client-supplied name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "transport", rename_all = "snake_case")]
pub enum OperatorOrigin {
    LocalSocket {
        connection: String,
    },
    Tcp {
        peer: std::net::SocketAddr,
        connection: String,
    },
    /// A paired remote device, named as the daemon enrolled it.
    Device {
        device: DeviceId,
        name: String,
        connection: String,
    },
}

/// Who caused an event.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "actor", rename_all = "snake_case")]
pub enum Actor {
    System,
    Scheduler,
    User {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<OperatorOrigin>,
    },
    Agent {
        id: AgentId,
    },
    Tool {
        call: ToolCallId,
    },
    /// A remembered decision answered on a person's behalf.
    Rule {
        rule: ApprovalRuleId,
    },
    Client {
        id: String,
    },
    /// An actor that acted on another machine, read out of an export. The
    /// original travels for the record; nothing here acted.
    Imported {
        was: Box<Actor>,
    },
}

impl Actor {
    pub fn agent(id: AgentId) -> Self {
        Self::Agent { id }
    }

    pub fn user() -> Self {
        Self::User {
            name: None,
            origin: None,
        }
    }

    pub fn connected_user(origin: OperatorOrigin) -> Self {
        Self::User {
            name: None,
            origin: Some(origin),
        }
    }

    pub fn agent_id(&self) -> Option<AgentId> {
        match self {
            Self::Agent { id } => Some(*id),
            _ => None,
        }
    }

    pub fn label(&self) -> String {
        match self {
            Self::System => "system".into(),
            Self::Scheduler => "scheduler".into(),
            Self::User { name, origin } => {
                let user = name
                    .as_ref()
                    .map_or_else(|| "user".into(), |n| format!("user:{n}"));
                match origin {
                    None => user,
                    Some(OperatorOrigin::LocalSocket { connection }) => {
                        format!("{user}:local_socket#{connection}")
                    }
                    Some(OperatorOrigin::Tcp { peer, connection }) => {
                        format!("{user}:tcp:{peer}#{connection}")
                    }
                    Some(OperatorOrigin::Device {
                        device,
                        name,
                        connection,
                    }) => format!(
                        "{user}:device:{}({device})#{connection}",
                        provenance_name(name)
                    ),
                }
            }
            Self::Agent { id } => id.to_string(),
            Self::Tool { call } => call.to_string(),
            Self::Rule { rule } => format!("rule:{rule}"),
            Self::Client { id } => format!("client:{id}"),
            Self::Imported { was } => format!("imported:{}", was.label()),
        }
    }

    /// The actor with no id in it.
    ///
    /// For derived text that is compared across machines: an id inside a
    /// sentence is text, not a reference, so an import leaves it as it was
    /// written - and a line that embeds one differs either side of a
    /// transfer. A client id and a user's name are not minted ids and stay.
    pub fn role_label(&self) -> String {
        match self {
            Self::Agent { .. } => "agent".into(),
            Self::Tool { .. } => "tool".into(),
            Self::Rule { .. } => "rule".into(),
            Self::Imported { was } => was.role_label(),
            other => other.label(),
        }
    }

    /// Who acted, with an import's wrapper taken off.
    ///
    /// A report says what a run did, wherever it ran, so it reads the
    /// original - which is also what keeps the report of an import equal to
    /// the report of the export. The local event log keeps the wrapper,
    /// because there the question is who acted *here*.
    pub fn original(&self) -> &Actor {
        match self {
            Self::Imported { was } => was.original(),
            other => other,
        }
    }

    /// This actor as it should be recorded on a machine that only read about
    /// it. Wrapping an already-imported actor would only nest.
    pub fn imported(self) -> Actor {
        match self {
            Self::Imported { .. } => self,
            other => Self::Imported {
                was: Box::new(other),
            },
        }
    }
}

/// Encode punctuation and non-ASCII bytes so a name cannot delimit another marker.
pub fn provenance_name(name: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(name.len());
    for byte in name.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b' ' | b'-' | b'_' | b'\'') {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(HEX[usize::from(byte & 15)]));
        }
    }
    debug_assert!(encoded.is_ascii());
    debug_assert!(!encoded.contains(['\n', '\r', '[', ']', '(', ')', '#', ':']));
    encoded
}
