use super::remote::{DeviceIdentity, DeviceProof};
#[expect(
    unused_imports,
    reason = "preserve intra-doc links and schema descriptions"
)]
use super::{Command, compat};
use super::{Deserialize, PROTOCOL_VERSION, Serialize, features};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Hello {
    pub protocol_version: u32,
    pub client_name: String,
    pub client_version: String,
    #[serde(default)]
    pub features: Vec<String>,
    /// Presented on a transport that has no other authorization - the TCP
    /// listener. The Unix socket ignores it: its file mode is the boundary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// A paired device's answer to the challenge the remote carrier sent.
    /// Other transports ignore it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceProof>,
}

/// Manual, so that one `tracing::debug!(?hello)` or `?frame` added later
/// cannot put the TCP token in a log file - the same reason `TcpListen` and
/// `ConnectionPolicy` hand-write theirs.
impl std::fmt::Debug for Hello {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hello")
            .field("protocol_version", &self.protocol_version)
            .field("client_name", &self.client_name)
            .field("client_version", &self.client_version)
            .field("features", &self.features)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .field("device", &self.device.as_ref().map(|proof| proof.id))
            .finish()
    }
}

impl Hello {
    pub fn new(client_name: impl Into<String>, client_version: impl Into<String>) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            client_name: client_name.into(),
            client_version: client_version.into(),
            features: vec![features::DELTAS.into(), features::ARTIFACT_BYTES.into()],
            token: None,
            device: None,
        }
    }

    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.token = Some(token.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Welcome {
    /// The version in effect for this connection, which is the client's own
    /// when the daemon translated down to it (`compat`).
    pub protocol_version: u32,
    pub daemon_version: String,
    pub instance_id: String,
    pub schema_version: u32,
    pub pid: u32,
    #[serde(default)]
    pub features: Vec<String>,
    /// Newest version the daemon speaks. Absent from a daemon built before
    /// N-1 support, which only ever matched exactly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daemon_protocol_version: Option<u32>,
    /// Oldest version the daemon still serves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_protocol_version: Option<u32>,
    /// How long this connection may go without sending a frame before the
    /// daemon closes it. Set on the TCP listener; absent on the socket, which
    /// has no timeout. A client that subscribes and then falls silent must
    /// send [`Command::Ping`] more often than this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_timeout_secs: Option<u64>,
    /// The paired device this connection authenticated as. Absent on the
    /// Unix socket and the token listener.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceIdentity>,
    /// Opaque receipt count at the handshake, kept with each keyed send for
    /// [`Command::ResolveSubmission`]. Absent before protocol 18.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submission_horizon: Option<String>,
}
