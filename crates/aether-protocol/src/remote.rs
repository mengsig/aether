//! Remote control types: devices, pairing, host services, attention and Web Push
//! (docs/remote.md). HTTP bodies and the push payload live here for the generated clients.

use std::path::PathBuf;

pub use aether_core::id::DeviceId;
use aether_core::id::{AgentId, ApprovalId, TaskId};
use aether_core::time::Timestamp;
use aether_core::tool::ApprovalRequest;
use serde::{Deserialize, Serialize};

/// What a paired device may do. See the authorization matrix in docs/remote.md.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum DeviceRole {
    /// Everything an operator at the host can do, minus the host-only set.
    #[default]
    Operator,
    /// Read-only: sees the fleet, changes nothing.
    Observer,
}

/// The device a connection authenticated as, echoed in [`crate::Welcome`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DeviceIdentity {
    pub id: DeviceId,
    pub name: String,
    pub role: DeviceRole,
}

/// A device's answer to the carrier's challenge, sent in [`crate::Hello`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DeviceProof {
    pub id: DeviceId,
    /// ECDSA P-256/SHA-256 over the signing input, as raw `r || s`
    /// (64 bytes), base64url without padding.
    pub signature: String,
}

/// One enrolled device as `aether remote devices` and a phone's settings list it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Device {
    pub id: DeviceId,
    pub name: String,
    pub role: DeviceRole,
    /// Lowercase hex SHA-256 of the raw public key. Only the host is told.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    pub created_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_at: Option<Timestamp>,
    /// Whether this is the device the request came from.
    #[serde(default)]
    pub current: bool,
    /// Set once revoked; a revoked device never authenticates again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<Timestamp>,
    /// Live connections authenticated as this device right now.
    #[serde(default)]
    pub connections: u32,
    /// Whether the device holds a push subscription.
    #[serde(default)]
    pub push: bool,
}

const STALE_REVISION: &str = "the remote settings are at revision ";

/// What the daemon says, as `invalid_argument`, to a `configure_remote` whose
/// `expected_revision` is no longer current.
pub fn stale_revision_message(current: u64, expected: u64) -> String {
    format!("{STALE_REVISION}{current}, not {expected}; read them again and retry")
}

/// Whether `reply` is the refusal [`stale_revision_message`] makes, as opposed
/// to any other `invalid_argument`.
pub fn is_stale_revision(reply: &crate::ErrorReply) -> bool {
    reply.code == crate::ErrorCode::InvalidArgument && reply.message.starts_with(STALE_REVISION)
}

/// The runtime-changeable remote settings, saved whole under a revision.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RemoteSettings {
    /// Whether the remote listener is open.
    #[serde(default)]
    pub enabled: bool,
    /// Whether `start_pairing` may open a pairing.
    #[serde(default)]
    pub pairing: bool,
    /// Exact origins browsers reach the listener by, e.g.
    /// `https://host.tailnet.ts.net`. The Host and Origin allowlist.
    #[serde(default)]
    pub public_origins: Vec<String>,
    /// Whether the daemon sends Web Push notifications.
    #[serde(default)]
    pub push: bool,
}

/// Reply to `get_remote` and `configure_remote`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RemoteStatus {
    /// Bumped by every accepted `configure_remote`.
    pub revision: u64,
    pub settings: RemoteSettings,
    /// `[remote] address` as configured.
    pub address: String,
    /// The address actually bound; absent while the listener is closed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listening: Option<String>,
    /// `[remote] allow_remote` as configured.
    #[serde(default)]
    pub allow_remote: bool,
    /// Enrolled, unrevoked devices.
    #[serde(default)]
    pub devices: u32,
    /// Live device connections.
    #[serde(default)]
    pub connections: u32,
    /// `[remote] push_previews` as configured.
    #[serde(default)]
    pub push_previews: bool,
    /// Why the listener is not open although enabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PairingState {
    /// Waiting for a device to claim the code.
    Open,
    /// A device claimed the code; waiting for the host to confirm.
    Claimed,
    Enrolled,
    Rejected,
    Expired,
    Cancelled,
}

/// The device asking to be enrolled, as the host must confirm it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PairingCandidate {
    pub name: String,
    /// Lowercase hex SHA-256 of the raw public key.
    pub fingerprint: String,
    /// Shown on both screens; the operator confirms they match.
    pub phrase: String,
    pub claimed_at: Timestamp,
}

/// One pairing attempt. Reply to the four pairing commands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Pairing {
    pub id: String,
    pub state: PairingState,
    /// The role the device is enrolled with on confirmation.
    pub role: DeviceRole,
    pub expires_at: Timestamp,
    /// The single-use code. Present only in the reply to `start_pairing`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// The pairing URL with the code in its fragment. Same presence as `code`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Present from `claimed` on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate: Option<PairingCandidate>,
    /// Present once `enrolled`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceIdentity>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PathCompletionKind {
    /// Directories anywhere on the host, for choosing a workspace.
    Directory,
    /// Files and directories inside `workspace`, for mentions.
    WorkspaceFile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PathCompletion {
    /// Absolute for `directory`; relative to the workspace for
    /// `workspace_file`. A directory ends in `/`.
    pub path: String,
    pub is_dir: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PathCompletions {
    /// Sorted by `path`.
    pub entries: Vec<PathCompletion>,
    /// More matched than `limit` allowed.
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct GitSummary {
    /// Whether the directory is inside a git work tree at all.
    pub repository: bool,
    /// Top level of the work tree; absent when not a repository.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<PathBuf>,
    /// Current branch; absent when detached or not a repository.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Abbreviated commit of `HEAD`, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    #[serde(default)]
    pub detached: bool,
    /// Uncommitted changes to tracked files, or untracked files.
    #[serde(default)]
    pub dirty: bool,
    /// Commits the branch has that its upstream lacks; absent without an upstream.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ahead: Option<u32>,
    /// Commits its upstream has that the branch lacks; absent without an upstream.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behind: Option<u32>,
    /// Paths with changes staged in the index.
    #[serde(default)]
    pub staged: u32,
    /// Tracked paths changed in the work tree but not staged, conflicts included.
    #[serde(default)]
    pub modified: u32,
    /// Untracked paths that are not ignored.
    #[serde(default)]
    pub untracked: u32,
}

/// One workspace the daemon knows: it holds or held a root agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Workspace {
    /// Canonical absolute path on the host.
    pub path: PathBuf,
    /// Live agents in trees rooted here.
    pub agents: u32,
    pub running: u32,
    /// Latest update of any root agent here; what orders the list.
    pub active_at: Timestamp,
    /// The directory is still there.
    pub exists: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<GitSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkspaceValidation {
    /// Whether an agent can be started here.
    pub valid: bool,
    /// The canonical path, when the input resolved to one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    /// Why not, in one sentence. Absent when `valid`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<GitSummary>,
}

/// What can wait on the operator, and what a push notification is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttentionKind {
    Approval,
    Question,
    AgentFailed,
    AgentBlocked,
    TaskCompleted,
    TaskFailed,
    /// Sent only by `test_push`.
    Test,
}

/// An agent's question nobody has answered or superseded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PendingQuestion {
    pub agent: AgentId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    /// What `reply_to_question` takes.
    pub question_seq: u64,
    pub question: String,
    pub asked_at: Timestamp,
}

/// A failed agent, or one blocked on something only the operator can clear.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentAttention {
    pub agent: AgentId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    /// `agent_failed` or `agent_blocked`.
    pub kind: AttentionKind,
    pub reason: String,
    pub since: Timestamp,
}

/// Everything waiting on the operator, read at one sequence number.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Attention {
    /// The log position this was read at; subscribe after it to stay current.
    pub last_seq: u64,
    pub approvals: Vec<ApprovalRequest>,
    pub questions: Vec<PendingQuestion>,
    pub agents: Vec<AgentAttention>,
}

/// A browser's `PushSubscription`, as `toJSON()` gives it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PushSubscription {
    /// The push service URL. Must be https.
    pub endpoint: String,
    /// The subscription's P-256 public key, base64url without padding.
    pub p256dh: String,
    /// The subscription's auth secret, base64url without padding.
    pub auth: String,
}

/// Manual: the endpoint and its keys are a capability to notify the device.
impl std::fmt::Debug for PushSubscription {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PushSubscription")
            .field("endpoint", &"<redacted>")
            .field("p256dh", &"<redacted>")
            .field("auth", &"<redacted>")
            .finish()
    }
}

/// Reply to the push commands: what the daemon sends this device.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PushConfig {
    /// Whether the daemon sends notifications at all.
    pub enabled: bool,
    /// The VAPID application server key (uncompressed P-256 point,
    /// base64url without padding). Absent while push was never enabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vapid_public_key: Option<String>,
    /// Whether payloads may carry `preview` text.
    #[serde(default)]
    pub previews: bool,
    /// Whether the asking device holds a subscription.
    #[serde(default)]
    pub subscribed: bool,
    /// The kinds the asking device is notified about; empty means all.
    #[serde(default)]
    pub kinds: Vec<AttentionKind>,
}

/// The decrypted body of one Web Push message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PushPayload {
    /// Payload schema version; 1.
    pub v: u32,
    pub kind: AttentionKind,
    /// The daemon that sent it; a client ignores another instance's.
    pub instance_id: String,
    /// The event that caused it.
    pub seq: u64,
    pub at: Timestamp,
    /// Fixed text per kind; never transcript content.
    pub title: String,
    /// Where a tap lands: a fragment route of the web app, such as
    /// `#attention?item=apr_...`, relative to the service worker's scope.
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<TaskId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<ApprovalId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question_seq: Option<u64>,
    /// At most 200 characters of content; only with `[remote] push_previews`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}

/// Body of `POST /api/pair/claim`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PairClaimRequest {
    /// The single-use code, as printed or read from the URL fragment.
    pub code: String,
    /// The device's raw P-256 public key (65-byte uncompressed point),
    /// base64url without padding.
    pub public_key: String,
    /// 1 to 64 characters once whitespace is folded; no control, format or invisible
    /// characters (docs/remote.md, Device names).
    pub device_name: String,
}

/// Manual, so a logged request cannot spend the pairing code.
impl std::fmt::Debug for PairClaimRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PairClaimRequest")
            .field("code", &"<redacted>")
            .field("public_key", &self.public_key)
            .field("device_name", &self.device_name)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PairClaimResponse {
    /// Names this attempt in `POST /api/pair/status`.
    pub pairing: String,
    pub fingerprint: String,
    /// Show it; the operator compares it with the host's.
    pub phrase: String,
    pub expires_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PairStatusRequest {
    pub pairing: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PairStatusResponse {
    /// `claimed`, `enrolled`, `rejected`, `expired` or `cancelled`.
    pub state: PairingState,
    /// Present once `enrolled`: the identity to authenticate as.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceIdentity>,
}

/// Body of `GET /api/health`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Health {
    /// Always `ok`: a daemon that cannot answer does not answer.
    pub status: String,
    pub protocol_version: u32,
    pub min_protocol_version: u32,
    /// Whether a pairing is open for a device to claim.
    pub pairing_open: bool,
}

/// Why an HTTP request was refused. See the table in docs/remote.md.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HttpErrorCode {
    BadRequest,
    ForbiddenHost,
    ForbiddenOrigin,
    NotFound,
    MethodNotAllowed,
    UnsupportedMediaType,
    PayloadTooLarge,
    PairingClosed,
    InvalidCode,
    Expired,
    RateLimited,
    Unavailable,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct HttpErrorBody {
    pub code: HttpErrorCode,
    pub message: String,
}

/// Every non-2xx HTTP response body: `{"error": {"code", "message"}}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct HttpError {
    pub error: HttpErrorBody,
}

/// Domain separator of the challenge signing input.
pub const AUTH_CONTEXT: &str = "aether-remote-auth-v1";

/// The exact bytes a device signs to answer a challenge: five fields joined
/// by `\n`, no trailing newline. `nonce` is the string as received.
pub fn signing_input(instance_id: &str, origin: &str, device: DeviceId, nonce: &str) -> Vec<u8> {
    format!("{AUTH_CONTEXT}\n{instance_id}\n{origin}\n{device}\n{nonce}").into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_signing_input_is_five_newline_joined_fields() {
        let device = DeviceId::new();
        let input = signing_input("inst", "https://h.example", device, "bm9uY2U");
        let text = String::from_utf8(input).expect("ASCII fields");
        let fields: Vec<&str> = text.split('\n').collect();
        assert_eq!(
            fields,
            [
                "aether-remote-auth-v1",
                "inst",
                "https://h.example",
                device.to_string().as_str(),
                "bm9uY2U"
            ]
        );
        assert!(!text.ends_with('\n'));
    }

    #[test]
    fn debug_prints_no_pairing_code_or_push_capability() {
        let claim = PairClaimRequest {
            code: "SECRETCODE".into(),
            public_key: "key".into(),
            device_name: "phone".into(),
        };
        assert!(!format!("{claim:?}").contains("SECRETCODE"));
        let subscription = PushSubscription {
            endpoint: "https://push.example/secret-endpoint".into(),
            p256dh: "p256dh-secret".into(),
            auth: "auth-secret".into(),
        };
        let printed = format!("{subscription:?}");
        assert!(!printed.contains("secret"), "{printed}");
    }
}
