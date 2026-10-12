//! Web Push encryption: RFC 8188 `aes128gcm`, keyed as RFC 8291 derives it from a subscription.
//! A notification always fits one record, so [`seal`] writes only that.

use aws_lc_rs::{aead, agreement, hkdf};

/// An uncompressed P-256 point.
pub const PUBLIC_KEY_BYTES: usize = 65;
/// The subscription's authentication secret.
pub const AUTH_SECRET_BYTES: usize = 16;
pub(crate) const SALT_BYTES: usize = 16;
pub(crate) const TAG_BYTES: usize = 16;
const KEY_BYTES: usize = 16;
const NONCE_BYTES: usize = 12;
const IKM_BYTES: usize = 32;
/// The record size announced in the header. One record never reaches it.
const RECORD_SIZE: u32 = 4096;
/// Salt, record size, key id length and the sender's public key.
pub(crate) const HEADER_BYTES: usize = SALT_BYTES + 4 + 1 + PUBLIC_KEY_BYTES;
/// A push service must take a 4096-byte body (RFC 8030); nothing longer is sent.
pub const MAX_PLAINTEXT_BYTES: usize = 4096 - HEADER_BYTES - TAG_BYTES - 1;
/// Ends the last record of a message.
const LAST_RECORD: u8 = 0x02;

const CEK_INFO: &[u8] = b"Content-Encoding: aes128gcm\0";
const NONCE_INFO: &[u8] = b"Content-Encoding: nonce\0";
const WEBPUSH_INFO: &[u8] = b"WebPush: info\0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum EceError {
    #[error("a {0}-byte payload does not fit one push message")]
    TooLong(usize),
    #[error("the subscription's public key is not a P-256 point")]
    BadPeerKey,
    #[error("the cryptography library refused the operation")]
    Crypto,
    #[error("the message is not a well-formed aes128gcm body")]
    Malformed,
}

/// The user agent's half of a subscription, decoded.
#[derive(Clone, PartialEq, Eq)]
pub struct Recipient {
    pub p256dh: [u8; PUBLIC_KEY_BYTES],
    pub auth: [u8; AUTH_SECRET_BYTES],
}

impl std::fmt::Debug for Recipient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Recipient(<redacted>)")
    }
}

/// The content-encryption key and base nonce of one message.
pub(crate) struct ContentKeys {
    key: [u8; KEY_BYTES],
    nonce: [u8; NONCE_BYTES],
}

impl ContentKeys {
    /// RFC 8188 section 2.2 and 2.3: both come from the salt and the keying material.
    pub(crate) fn derive(salt: &[u8], ikm: &[u8]) -> Result<Self, EceError> {
        assert_eq!(salt.len(), SALT_BYTES, "an aes128gcm salt is 16 bytes");
        assert!(!ikm.is_empty(), "keying material is never empty");
        let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, salt).extract(ikm);
        let mut key = [0u8; KEY_BYTES];
        let mut nonce = [0u8; NONCE_BYTES];
        expand(&prk, CEK_INFO, &mut key)?;
        expand(&prk, NONCE_INFO, &mut nonce)?;
        Ok(Self { key, nonce })
    }

    /// The nonce of record `sequence`: the base nonce XOR the counter.
    fn nonce(&self, sequence: u64) -> aead::Nonce {
        let mut nonce = self.nonce;
        let counter = sequence.to_be_bytes();
        for (byte, count) in nonce[NONCE_BYTES - counter.len()..].iter_mut().zip(counter) {
            *byte ^= count;
        }
        aead::Nonce::assume_unique_for_key(nonce)
    }

    fn cipher(&self) -> Result<aead::LessSafeKey, EceError> {
        aead::UnboundKey::new(&aead::AES_128_GCM, &self.key)
            .map(aead::LessSafeKey::new)
            .map_err(|_| EceError::Crypto)
    }

    /// Encrypt `record` (already padded and delimited) in place as record `sequence`.
    fn seal_record(&self, sequence: u64, record: &mut Vec<u8>) -> Result<(), EceError> {
        assert!(!record.is_empty(), "a record carries its delimiter");
        self.cipher()?
            .seal_in_place_append_tag(self.nonce(sequence), aead::Aad::empty(), record)
            .map_err(|_| EceError::Crypto)
    }

    /// Decrypt record `sequence` in place and return its padded plaintext.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn open_record<'a>(
        &self,
        sequence: u64,
        record: &'a mut [u8],
    ) -> Result<&'a mut [u8], EceError> {
        if record.len() <= TAG_BYTES {
            return Err(EceError::Malformed);
        }
        self.cipher()?
            .open_in_place(self.nonce(sequence), aead::Aad::empty(), record)
            .map_err(|_| EceError::Malformed)
    }
}

struct OutputLen(usize);

impl hkdf::KeyType for OutputLen {
    fn len(&self) -> usize {
        self.0
    }
}

fn expand(prk: &hkdf::Prk, info: &[u8], out: &mut [u8]) -> Result<(), EceError> {
    prk.expand(&[info], OutputLen(out.len()))
        .and_then(|okm| okm.fill(out))
        .map_err(|_| EceError::Crypto)
}

/// RFC 8291 section 3.3 and 3.4: the keying material both sides derive from
/// the ECDH secret, the authentication secret and the two public keys.
pub(crate) fn shared_ikm(
    ours: &agreement::PrivateKey,
    theirs: &[u8; PUBLIC_KEY_BYTES],
    auth: &[u8; AUTH_SECRET_BYTES],
    user_agent_public: &[u8; PUBLIC_KEY_BYTES],
    server_public: &[u8; PUBLIC_KEY_BYTES],
) -> Result<[u8; IKM_BYTES], EceError> {
    let mut info = Vec::with_capacity(WEBPUSH_INFO.len() + 2 * PUBLIC_KEY_BYTES);
    info.extend_from_slice(WEBPUSH_INFO);
    info.extend_from_slice(user_agent_public);
    info.extend_from_slice(server_public);
    let peer = agreement::UnparsedPublicKey::new(&agreement::ECDH_P256, theirs);
    agreement::agree(ours, peer, EceError::BadPeerKey, |ecdh_secret| {
        let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, auth).extract(ecdh_secret);
        let mut ikm = [0u8; IKM_BYTES];
        expand(&prk, &info, &mut ikm)?;
        Ok(ikm)
    })
}

/// The public half of `key` as an uncompressed point.
pub(crate) fn public_point(
    key: &agreement::PrivateKey,
) -> Result<[u8; PUBLIC_KEY_BYTES], EceError> {
    let public = key.compute_public_key().map_err(|_| EceError::Crypto)?;
    public.as_ref().try_into().map_err(|_| EceError::Crypto)
}

/// Encrypt `plaintext` for `recipient` under a fresh sender key and salt.
/// The result is the whole request body, header included.
pub fn seal(recipient: &Recipient, plaintext: &[u8]) -> Result<Vec<u8>, EceError> {
    let sender =
        agreement::PrivateKey::generate(&agreement::ECDH_P256).map_err(|_| EceError::Crypto)?;
    let mut salt = [0u8; SALT_BYTES];
    aws_lc_rs::rand::fill(&mut salt).map_err(|_| EceError::Crypto)?;
    seal_with(recipient, &sender, &salt, plaintext)
}

pub(crate) fn seal_with(
    recipient: &Recipient,
    sender: &agreement::PrivateKey,
    salt: &[u8; SALT_BYTES],
    plaintext: &[u8],
) -> Result<Vec<u8>, EceError> {
    if plaintext.len() > MAX_PLAINTEXT_BYTES {
        return Err(EceError::TooLong(plaintext.len()));
    }
    let sender_public = public_point(sender)?;
    let ikm = shared_ikm(
        sender,
        &recipient.p256dh,
        &recipient.auth,
        &recipient.p256dh,
        &sender_public,
    )?;
    let keys = ContentKeys::derive(salt, &ikm)?;
    let mut record = Vec::with_capacity(plaintext.len() + 1 + TAG_BYTES);
    record.extend_from_slice(plaintext);
    record.push(LAST_RECORD);
    keys.seal_record(0, &mut record)?;

    let mut body = Vec::with_capacity(HEADER_BYTES + record.len());
    body.extend_from_slice(salt);
    body.extend_from_slice(&RECORD_SIZE.to_be_bytes());
    body.push(PUBLIC_KEY_BYTES as u8);
    body.extend_from_slice(&sender_public);
    body.extend_from_slice(&record);
    assert!(body.len() <= 4096, "one record stays inside the body limit");
    Ok(body)
}

#[cfg(test)]
mod tests;
