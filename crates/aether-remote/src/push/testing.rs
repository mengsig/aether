//! Test support: the other side of Web Push. A user agent that holds a
//! subscription's private key and decrypts what arrives, a verifier for the
//! VAPID token, and in-memory stand-ins for the notifier's collaborators.

// Test support panics on a broken fixture, as a test would.
#![allow(clippy::expect_used)]

use std::collections::{BTreeMap, HashMap, VecDeque};

use aether_core::id::{AgentId, ApprovalId};
use aether_protocol::remote::{DeviceId, PushPayload, PushSubscription};
use aws_lc_rs::{agreement, signature};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use parking_lot::Mutex;

use super::ece::{
    AUTH_SECRET_BYTES, ContentKeys, EceError, PUBLIC_KEY_BYTES, Recipient, SALT_BYTES, TAG_BYTES,
};
use super::notice::{AgentFacts, Fleet};
use super::transport::{PushRequest, PushResponse, Transport, TransportError};
use super::{PushError, PushStore, StoredSubscription};

/// Decode an RFC 8188 `aes128gcm` body of any number of records.
/// `ikm_for` turns the header's key id into the input keying material.
pub fn open_aes128gcm(
    body: &[u8],
    ikm_for: impl FnOnce(&[u8]) -> Result<Vec<u8>, EceError>,
) -> Result<Vec<u8>, EceError> {
    let (salt, rest) = body
        .split_at_checked(SALT_BYTES)
        .ok_or(EceError::Malformed)?;
    let (size, rest) = rest.split_at_checked(4).ok_or(EceError::Malformed)?;
    let record_size = u32::from_be_bytes(size.try_into().expect("four bytes")) as usize;
    let (&id_len, rest) = rest.split_first().ok_or(EceError::Malformed)?;
    let (key_id, records) = rest
        .split_at_checked(usize::from(id_len))
        .ok_or(EceError::Malformed)?;
    if record_size <= TAG_BYTES + 1 || records.is_empty() {
        return Err(EceError::Malformed);
    }
    let keys = ContentKeys::derive(salt, &ikm_for(key_id)?)?;
    let count = records.len().div_ceil(record_size);
    let mut plaintext = Vec::with_capacity(records.len());
    for (sequence, record) in records.chunks(record_size).enumerate() {
        let mut record = record.to_vec();
        let padded = keys.open_record(sequence as u64, &mut record)?;
        let last = sequence + 1 == count;
        plaintext.extend_from_slice(unpad(padded, last)?);
    }
    Ok(plaintext)
}

/// Strip a record's padding: zeros back to the delimiter, 2 on the last
/// record and 1 on every other.
fn unpad(padded: &[u8], last: bool) -> Result<&[u8], EceError> {
    let end = padded
        .iter()
        .rposition(|byte| *byte != 0)
        .ok_or(EceError::Malformed)?;
    let delimiter = if last { 2 } else { 1 };
    if padded[end] != delimiter {
        return Err(EceError::Malformed);
    }
    Ok(&padded[..end])
}

/// A browser's half of a push subscription.
pub struct UserAgent {
    private: agreement::PrivateKey,
    public: [u8; PUBLIC_KEY_BYTES],
    auth: [u8; AUTH_SECRET_BYTES],
}

impl std::fmt::Debug for UserAgent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("UserAgent")
    }
}

impl UserAgent {
    pub fn generate() -> Self {
        let private = agreement::PrivateKey::generate(&agreement::ECDH_P256).expect("a P-256 key");
        let mut auth = [0u8; AUTH_SECRET_BYTES];
        aws_lc_rs::rand::fill(&mut auth).expect("random bytes");
        Self::assemble(private, auth)
    }

    /// A user agent with a known private scalar, for published test vectors.
    pub fn from_parts(private: &[u8], auth: [u8; AUTH_SECRET_BYTES]) -> Self {
        let private = agreement::PrivateKey::from_private_key(&agreement::ECDH_P256, private)
            .expect("a P-256 private scalar");
        Self::assemble(private, auth)
    }

    fn assemble(private: agreement::PrivateKey, auth: [u8; AUTH_SECRET_BYTES]) -> Self {
        let public = super::ece::public_point(&private).expect("a public key");
        Self {
            private,
            public,
            auth,
        }
    }

    pub fn recipient(&self) -> Recipient {
        Recipient {
            p256dh: self.public,
            auth: self.auth,
        }
    }

    /// The subscription a browser would hand the page for `endpoint`.
    pub fn subscription(&self, endpoint: &str) -> PushSubscription {
        PushSubscription {
            endpoint: endpoint.to_owned(),
            p256dh: URL_SAFE_NO_PAD.encode(self.public),
            auth: URL_SAFE_NO_PAD.encode(self.auth),
        }
    }

    /// Decrypt a push message body as RFC 8291 section 3.4 has the receiver do.
    pub fn open(&self, body: &[u8]) -> Result<Vec<u8>, EceError> {
        open_aes128gcm(body, |key_id| {
            let server: [u8; PUBLIC_KEY_BYTES] =
                key_id.try_into().map_err(|_| EceError::Malformed)?;
            let ikm =
                super::ece::shared_ikm(&self.private, &server, &self.auth, &self.public, &server)?;
            Ok(ikm.to_vec())
        })
    }

    /// Decrypt and parse a notification.
    pub fn payload(&self, body: &[u8]) -> PushPayload {
        let plaintext = self.open(body).expect("a message for this user agent");
        serde_json::from_slice(&plaintext).expect("a push payload")
    }
}

/// A verified VAPID token.
#[derive(Debug, Clone, PartialEq)]
pub struct VapidToken {
    /// The `k` parameter: the application server key, base64url.
    pub key: String,
    pub header: serde_json::Value,
    pub claims: serde_json::Value,
}

/// Parse a `vapid t=..., k=...` header and check the ES256 signature
/// against the key it names.
pub fn verify_authorization(header: &str) -> Result<VapidToken, String> {
    let parameters = header
        .strip_prefix("vapid ")
        .ok_or("not the vapid scheme")?;
    let fields: BTreeMap<&str, &str> = parameters
        .split(',')
        .filter_map(|field| field.trim().split_once('='))
        .collect();
    let token = *fields.get("t").ok_or("no t parameter")?;
    let key = *fields.get("k").ok_or("no k parameter")?;
    let decode = |part: &str| {
        URL_SAFE_NO_PAD
            .decode(part)
            .map_err(|e| format!("base64url: {e}"))
    };
    let parts: Vec<&str> = token.split('.').collect();
    let [header, claims, signature] = parts[..] else {
        return Err("a JWT has three parts".into());
    };
    let signed = &token[..header.len() + 1 + claims.len()];
    signature::UnparsedPublicKey::new(&signature::ECDSA_P256_SHA256_FIXED, decode(key)?)
        .verify(signed.as_bytes(), &decode(signature)?)
        .map_err(|_| "the signature does not verify under k".to_owned())?;
    let json = |part: &str| -> Result<serde_json::Value, String> {
        serde_json::from_slice(&decode(part)?).map_err(|e| format!("json: {e}"))
    };
    Ok(VapidToken {
        key: key.to_owned(),
        header: json(header)?,
        claims: json(claims)?,
    })
}

/// Subscriptions in memory.
#[derive(Debug, Default)]
pub struct MemoryPushStore {
    subscriptions: Mutex<BTreeMap<DeviceId, StoredSubscription>>,
    writes: std::sync::atomic::AtomicUsize,
}

impl MemoryPushStore {
    /// How many times a subscription was written.
    pub fn writes(&self) -> usize {
        self.writes.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl PushStore for MemoryPushStore {
    fn subscription(&self, device: DeviceId) -> Result<Option<StoredSubscription>, PushError> {
        Ok(self.subscriptions.lock().get(&device).cloned())
    }

    fn subscriptions(&self) -> Result<Vec<StoredSubscription>, PushError> {
        Ok(self.subscriptions.lock().values().cloned().collect())
    }

    fn put_subscription(&self, subscription: &StoredSubscription) -> Result<(), PushError> {
        self.writes
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.subscriptions
            .lock()
            .insert(subscription.device, subscription.clone());
        Ok(())
    }

    fn remove_subscription(&self, device: DeviceId) -> Result<(), PushError> {
        self.subscriptions.lock().remove(&device);
        Ok(())
    }

    fn remove_exact_subscription(&self, dead: &StoredSubscription) -> Result<bool, PushError> {
        let mut subscriptions = self.subscriptions.lock();
        if subscriptions.get(&dead.device) != Some(dead) {
            return Ok(false);
        }
        subscriptions.remove(&dead.device);
        Ok(true)
    }
}

/// A fleet that knows what a test told it.
#[derive(Debug, Default)]
pub struct StaticFleet {
    agents: Mutex<HashMap<AgentId, AgentFacts>>,
    pending: Mutex<Vec<ApprovalId>>,
}

impl StaticFleet {
    pub fn set_agent(&self, agent: AgentId, facts: AgentFacts) {
        self.agents.lock().insert(agent, facts);
    }

    pub fn set_pending(&self, approval: ApprovalId) {
        self.pending.lock().push(approval);
    }
}

impl Fleet for StaticFleet {
    fn agent(&self, agent: AgentId) -> Result<Option<AgentFacts>, PushError> {
        Ok(self.agents.lock().get(&agent).cloned())
    }

    fn approval_pending(&self, approval: ApprovalId) -> Result<bool, PushError> {
        Ok(self.pending.lock().contains(&approval))
    }
}

/// A transport that records every request and answers from a script;
/// once the script runs out it answers `201 Created`.
#[derive(Debug, Default)]
pub struct RecordingTransport {
    requests: Mutex<Vec<(tokio::time::Instant, PushRequest)>>,
    script: Mutex<VecDeque<Result<PushResponse, TransportError>>>,
}

impl RecordingTransport {
    /// Answer the next unscripted request with `status`.
    pub fn answer(&self, status: u16) {
        self.script.lock().push_back(Ok(PushResponse {
            status,
            retry_after: None,
        }));
    }

    pub fn answer_with(&self, answer: Result<PushResponse, TransportError>) {
        self.script.lock().push_back(answer);
    }

    pub fn requests(&self) -> Vec<PushRequest> {
        self.requests
            .lock()
            .iter()
            .map(|(_, request)| request.clone())
            .collect()
    }

    /// When each request was made, on tokio's clock.
    pub fn times(&self) -> Vec<tokio::time::Instant> {
        self.requests.lock().iter().map(|(at, _)| *at).collect()
    }

    pub fn sent(&self) -> usize {
        self.requests.lock().len()
    }
}

#[async_trait::async_trait]
impl Transport for RecordingTransport {
    async fn post(&self, request: PushRequest) -> Result<PushResponse, TransportError> {
        self.requests
            .lock()
            .push((tokio::time::Instant::now(), request));
        let scripted = self.script.lock().pop_front();
        scripted.unwrap_or(Ok(PushResponse {
            status: 201,
            retry_after: None,
        }))
    }
}
