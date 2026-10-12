//! Device identity: challenge, signature, durable device record and live sessions. A device holds
//! a non-extractable ECDSA P-256 key and the daemon the public half: no bearer secret.

use std::collections::HashMap;
use std::sync::Arc;

use aether_core::time::Timestamp;
use aether_protocol::remote::{DeviceId, DeviceIdentity, DeviceProof, DeviceRole, signing_input};
use aws_lc_rs::signature::{ECDSA_P256_SHA256_FIXED, UnparsedPublicKey};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use tokio_util::sync::CancellationToken;

mod name;

pub use name::check_device_name;

/// A raw P-256 public key: `0x04 || X || Y`.
pub const PUBLIC_KEY_BYTES: usize = 65;
/// A fixed-width ECDSA P-256 signature: `r || s`.
pub const SIGNATURE_BYTES: usize = 64;
pub const NONCE_BYTES: usize = 32;
pub const MAX_DEVICE_NAME_CHARS: usize = 64;

/// Why a device was refused. Except where noted, the peer is told only
/// `unauthorized`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AuthError {
    #[error("the public key is not a raw uncompressed P-256 point")]
    MalformedKey,
    #[error("the signature is not 64 bytes of base64url")]
    MalformedSignature,
    #[error("the signature does not answer this challenge")]
    BadSignature,
    #[error("no such device")]
    UnknownDevice,
    #[error("the device was revoked")]
    Revoked,
    #[error("the challenge was already answered, or belongs to another connection")]
    ChallengeSpent,
    #[error(
        "the device name must be 1 to {MAX_DEVICE_NAME_CHARS} characters with no control, formatting or invisible characters"
    )]
    BadName,
    /// Nothing is wrong with the device: the peer is told `unavailable`.
    #[error("every device connection is in use")]
    Busy,
    #[error("device store: {0}")]
    Store(String),
}

/// What one carrier connection must sign. Bound to the connection, so an
/// answer cannot be replayed on another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Challenge {
    /// [`NONCE_BYTES`] random bytes, base64url without padding.
    pub nonce: String,
    /// The allowlisted `Origin` the connection arrived with.
    pub origin: String,
    pub instance_id: String,
}

impl Challenge {
    pub fn issue(instance_id: &str, origin: &str) -> Result<Self, AuthError> {
        let mut nonce = [0u8; NONCE_BYTES];
        aws_lc_rs::rand::fill(&mut nonce)
            .map_err(|_| AuthError::Store("the system random source failed".into()))?;
        Ok(Self {
            nonce: URL_SAFE_NO_PAD.encode(nonce),
            origin: origin.to_owned(),
            instance_id: instance_id.to_owned(),
        })
    }

    /// The exact bytes `device` signs for this challenge.
    pub fn signing_input(&self, device: DeviceId) -> Vec<u8> {
        signing_input(&self.instance_id, &self.origin, device, &self.nonce)
    }
}

/// Decode and validate a public key as a client submits it.
pub fn decode_public_key(encoded: &str) -> Result<Vec<u8>, AuthError> {
    let key = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| AuthError::MalformedKey)?;
    if key.len() != PUBLIC_KEY_BYTES || key[0] != 0x04 {
        return Err(AuthError::MalformedKey);
    }
    // A point off the curve would enrol and then never verify anything.
    UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, &key)
        .parse()
        .map_err(|_| AuthError::MalformedKey)?;
    Ok(key)
}

/// Lowercase hex SHA-256 of the raw public key.
pub fn fingerprint(public_key: &[u8]) -> String {
    use sha2::Digest as _;
    debug_assert_eq!(public_key.len(), PUBLIC_KEY_BYTES, "a raw P-256 point");
    hex::encode(sha2::Sha256::digest(public_key))
}

/// Check `signature` (base64url `r || s`) over `message` under `public_key`.
pub fn verify(public_key: &[u8], message: &[u8], signature: &str) -> Result<(), AuthError> {
    let signature = URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|_| AuthError::MalformedSignature)?;
    if signature.len() != SIGNATURE_BYTES {
        return Err(AuthError::MalformedSignature);
    }
    if public_key.len() != PUBLIC_KEY_BYTES {
        return Err(AuthError::MalformedKey);
    }
    UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, public_key)
        .verify(message, &signature)
        .map_err(|_| AuthError::BadSignature)
}

/// One enrolled device as the store keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceRecord {
    pub id: DeviceId,
    pub name: String,
    pub role: DeviceRole,
    /// Raw P-256 point, [`PUBLIC_KEY_BYTES`] long.
    pub public_key: Vec<u8>,
    pub created_at: Timestamp,
    pub last_seen_at: Option<Timestamp>,
    pub revoked_at: Option<Timestamp>,
}

impl DeviceRecord {
    pub fn identity(&self) -> DeviceIdentity {
        DeviceIdentity {
            id: self.id,
            name: self.name.clone(),
            role: self.role,
        }
    }
}

/// Durable device records and the trail of their use. The daemon's store
/// implements it; enrolling and revoking are the daemon's own commits.
pub trait DeviceStore: Send + Sync + std::fmt::Debug + 'static {
    fn device(&self, id: DeviceId) -> Result<Option<DeviceRecord>, AuthError>;
    /// Oldest first.
    fn devices(&self, include_revoked: bool) -> Result<Vec<DeviceRecord>, AuthError>;
    /// Record an accepted hello from `origin`: stamp `last_seen_at`, audit it.
    /// An error refuses the hello, so no session exists that left no trace.
    fn accepted(&self, device: &DeviceRecord, origin: &str) -> Result<(), AuthError>;
    /// Record a refused hello. The refusal stands whatever happens here.
    fn refused(&self, device: DeviceId, origin: &str, reason: &AuthError);
}

/// An authenticated device connection. `closed` fires when the device is
/// revoked or its role changes; the connection must end at once.
#[derive(Debug, Clone)]
pub struct DeviceSession {
    pub identity: DeviceIdentity,
    pub closed: CancellationToken,
}

/// Turns a hello's proof into a session, or refuses it.
pub trait DeviceAuthenticator: Send + Sync + std::fmt::Debug + 'static {
    fn authenticate(
        &self,
        challenge: &Challenge,
        proof: &DeviceProof,
    ) -> Result<DeviceSession, AuthError>;
}

/// What a carrier connection hands the daemon: the challenge it must send,
/// and who checks the answer.
#[derive(Debug, Clone)]
pub struct DeviceGate {
    pub challenge: Challenge,
    pub authenticator: Arc<dyn DeviceAuthenticator>,
}

/// Live sessions by device, so revoking one closes all of its connections.
#[derive(Debug, Default)]
pub struct Sessions {
    by_device: parking_lot::Mutex<HashMap<DeviceId, CancellationToken>>,
}

impl Sessions {
    /// A token for one new connection of `device`.
    pub fn open(&self, device: DeviceId) -> CancellationToken {
        self.by_device
            .lock()
            .entry(device)
            .or_default()
            .child_token()
    }

    /// End every connection of `device`, now and for tokens already handed out.
    pub fn close(&self, device: DeviceId) {
        if let Some(parent) = self.by_device.lock().remove(&device) {
            parent.cancel();
        }
    }
}

/// The store-backed authenticator: looks the device up, checks the
/// signature, and opens a session under [`Sessions`].
#[derive(Debug)]
pub struct StoredDevices {
    pub devices: Arc<dyn DeviceStore>,
    pub sessions: Arc<Sessions>,
}

impl StoredDevices {
    fn admit(
        &self,
        challenge: &Challenge,
        proof: &DeviceProof,
    ) -> Result<DeviceSession, AuthError> {
        let enrolled = self
            .devices
            .device(proof.id)?
            .ok_or(AuthError::UnknownDevice)?;
        if enrolled.revoked_at.is_some() {
            return Err(AuthError::Revoked);
        }
        verify(
            &enrolled.public_key,
            &challenge.signing_input(enrolled.id),
            &proof.signature,
        )?;
        // Opened before the record is read again: a revocation or a new role
        // landing in between is either read below or cancels this token.
        let closed = self.sessions.open(enrolled.id);
        let current = self
            .devices
            .device(enrolled.id)?
            .ok_or(AuthError::UnknownDevice)?;
        if current.revoked_at.is_some() {
            return Err(AuthError::Revoked);
        }
        debug_assert_eq!(
            current.public_key, enrolled.public_key,
            "a key is never replaced"
        );
        self.devices.accepted(&current, &challenge.origin)?;
        Ok(DeviceSession {
            identity: current.identity(),
            closed,
        })
    }
}

impl DeviceAuthenticator for StoredDevices {
    fn authenticate(
        &self,
        challenge: &Challenge,
        proof: &DeviceProof,
    ) -> Result<DeviceSession, AuthError> {
        self.admit(challenge, proof).inspect_err(|reason| {
            self.devices.refused(proof.id, &challenge.origin, reason);
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{MemoryDevices, key_pair, sign};
    use aws_lc_rs::signature::{EcdsaKeyPair, KeyPair as _};

    fn enrolled(key: &EcdsaKeyPair) -> (StoredDevices, Arc<MemoryDevices>, DeviceRecord) {
        let record = crate::testing::record(key);
        let devices = Arc::new(MemoryDevices::default());
        devices.put(&record);
        let authenticator = StoredDevices {
            devices: devices.clone(),
            sessions: Arc::new(Sessions::default()),
        };
        (authenticator, devices, record)
    }

    #[test]
    fn a_webcrypto_shaped_key_and_signature_authenticate() {
        let key = key_pair();
        let (authenticator, devices, record) = enrolled(&key);
        assert_eq!(record.public_key.len(), PUBLIC_KEY_BYTES);
        let encoded = URL_SAFE_NO_PAD.encode(&record.public_key);
        assert_eq!(decode_public_key(&encoded), Ok(record.public_key.clone()));
        assert_eq!(fingerprint(&record.public_key).len(), 64);

        let challenge = Challenge::issue("inst", "https://h.example").expect("nonce");
        let proof = DeviceProof {
            id: record.id,
            signature: sign(&key, &challenge.signing_input(record.id)),
        };
        let session = authenticator
            .authenticate(&challenge, &proof)
            .expect("the right key over the right bytes");
        assert_eq!(session.identity, record.identity());
        assert!(!session.closed.is_cancelled());
        assert_eq!(
            *devices.outcomes.lock(),
            [format!("accepted {} https://h.example", record.id)]
        );
    }

    #[test]
    fn an_answer_is_bound_to_its_challenge_and_origin() {
        let key = key_pair();
        let (authenticator, devices, record) = enrolled(&key);
        let challenge = Challenge::issue("inst", "https://h.example").expect("nonce");
        let proof = DeviceProof {
            id: record.id,
            signature: sign(&key, &challenge.signing_input(record.id)),
        };
        let replayed = Challenge::issue("inst", "https://h.example").expect("nonce");
        assert_ne!(replayed.nonce, challenge.nonce);
        let elsewhere = Challenge {
            origin: "https://evil.example".into(),
            ..challenge.clone()
        };
        for other in [replayed, elsewhere] {
            assert_eq!(
                authenticator.authenticate(&other, &proof).map(|_| ()),
                Err(AuthError::BadSignature)
            );
        }
        let stranger = DeviceProof {
            id: DeviceId::new(),
            signature: proof.signature.clone(),
        };
        assert_eq!(
            authenticator
                .authenticate(&challenge, &stranger)
                .map(|_| ()),
            Err(AuthError::UnknownDevice)
        );
        let outcomes = devices.outcomes.lock();
        assert_eq!(outcomes.len(), 3, "every refusal is reported: {outcomes:?}");
        assert!(outcomes.iter().all(|line| line.starts_with("refused ")));
    }

    #[test]
    fn a_revoked_device_is_refused_and_its_sessions_close() {
        let key = key_pair();
        let (authenticator, devices, mut record) = enrolled(&key);
        let challenge = Challenge::issue("inst", "https://h.example").expect("nonce");
        let proof = DeviceProof {
            id: record.id,
            signature: sign(&key, &challenge.signing_input(record.id)),
        };
        let session = authenticator.authenticate(&challenge, &proof).expect("ok");
        record.revoked_at = Some(aether_core::time::now());
        devices.put(&record);
        authenticator.sessions.close(record.id);
        assert!(session.closed.is_cancelled());
        assert_eq!(
            authenticator.authenticate(&challenge, &proof).map(|_| ()),
            Err(AuthError::Revoked)
        );
    }

    #[test]
    fn a_revocation_landing_mid_handshake_refuses_the_hello() {
        let key = key_pair();
        let (authenticator, devices, record) = enrolled(&key);
        let challenge = Challenge::issue("inst", "https://h.example").expect("nonce");
        let proof = DeviceProof {
            id: record.id,
            signature: sign(&key, &challenge.signing_input(record.id)),
        };
        // Revoked after the first read, so only the second read can see it.
        *devices.revoke_after_reads.lock() = Some(1);
        assert_eq!(
            authenticator.authenticate(&challenge, &proof).map(|_| ()),
            Err(AuthError::Revoked)
        );
        let outcomes = devices.outcomes.lock();
        assert_eq!(outcomes.len(), 1, "{outcomes:?}");
        assert!(outcomes[0].starts_with("refused "), "{outcomes:?}");
    }

    #[test]
    fn malformed_keys_and_signatures_are_refused() {
        assert_eq!(decode_public_key("AAAA"), Err(AuthError::MalformedKey));
        assert_eq!(
            decode_public_key("not base64!"),
            Err(AuthError::MalformedKey)
        );
        let key = key_pair();
        let public = key.public_key().as_ref().to_vec();
        assert_eq!(
            verify(&public, b"m", "AAAA"),
            Err(AuthError::MalformedSignature)
        );
    }
}
