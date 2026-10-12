//! Web Push: VAPID-signed, encrypted notifications to paired devices when something waits on
//! the operator. docs/remote.md covers the pipeline, the push hosts and previews.

mod delivery;
pub mod ece;
pub mod notice;
mod notifier;
mod outbox;
pub mod subscription;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;
#[cfg(test)]
mod tests;
pub mod transport;
pub mod vapid;

use aether_protocol::remote::{AttentionKind, DeviceId, PushSubscription};

pub use notice::{AgentFacts, Fleet, OperatorBlock, title};
pub use notifier::{Notifier, NotifierConfig};
pub use subscription::{HostPolicy, check_endpoint};
pub use transport::{NetTransport, PushRequest, PushResponse, Transport, TransportError};

/// Longest preview a payload carries, in characters.
pub const MAX_PREVIEW_CHARS: usize = 200;
/// Longest endpoint URL stored.
pub const MAX_ENDPOINT_BYTES: usize = 2048;
/// How long a push service may hold an undelivered message, and how long
/// the daemon keeps retrying one.
pub const TTL_SECS: u32 = 3600;
/// The payload schema version in [`aether_protocol::remote::PushPayload::v`].
pub const PAYLOAD_VERSION: u32 = 1;

/// Why a push operation was refused or failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PushError {
    #[error(
        "a push endpoint must be an https URL of at most {MAX_ENDPOINT_BYTES} bytes, \
         on the default port and without credentials"
    )]
    BadEndpoint,
    #[error("the subscription keys are not base64url P-256 and auth values")]
    BadKeys,
    #[error(
        "`{host}` is not a known push service; an operator can allow it with \
         `[remote] push_hosts`"
    )]
    HostNotAllowed { host: String },
    #[error("push notifications are disabled")]
    Disabled,
    #[error("this device has no push subscription")]
    NotSubscribed,
    /// The push service refused the message or could not be reached.
    #[error("the push service did not accept the message: {0}")]
    Service(String),
    #[error("the VAPID key: {0}")]
    Key(String),
    #[error("building the push message: {0}")]
    Message(String),
    #[error("push store: {0}")]
    Store(String),
}

/// One device's stored subscription.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredSubscription {
    pub device: DeviceId,
    pub subscription: PushSubscription,
    /// Empty means every kind.
    pub kinds: Vec<AttentionKind>,
}

impl StoredSubscription {
    /// Whether this device asked to hear about `kind`. A test always goes out.
    pub fn wants(&self, kind: AttentionKind) -> bool {
        kind == AttentionKind::Test || self.kinds.is_empty() || self.kinds.contains(&kind)
    }
}

/// Durable subscriptions. The daemon's store implements it.
pub trait PushStore: Send + Sync + std::fmt::Debug + 'static {
    fn subscription(&self, device: DeviceId) -> Result<Option<StoredSubscription>, PushError>;
    /// Every readable subscription; a row that cannot be read is logged and
    /// left out, so one bad row cannot silence the others.
    fn subscriptions(&self) -> Result<Vec<StoredSubscription>, PushError>;
    /// Insert or replace the device's one subscription.
    fn put_subscription(&self, subscription: &StoredSubscription) -> Result<(), PushError>;
    fn remove_subscription(&self, device: DeviceId) -> Result<(), PushError>;
    /// Remove `dead` only while it is still what its device has stored. Says
    /// whether it was: a subscription that replaced it meanwhile stays.
    fn remove_exact_subscription(&self, dead: &StoredSubscription) -> Result<bool, PushError>;
}
