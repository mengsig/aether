//! VAPID (RFC 8292): the daemon's application server key and the token that identifies it. The
//! private key sits in a file only the daemon's user can read and leaves only as a signature.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair as _};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use super::ece::PUBLIC_KEY_BYTES;

/// How long a token is good for. RFC 8292 allows at most a day.
pub const TOKEN_LIFETIME: Duration = Duration::from_secs(12 * 3600);
/// The fixed JOSE header, base64url encoded once.
const JWT_HEADER: &str = r#"{"typ":"JWT","alg":"ES256"}"#;
/// A PKCS#8 P-256 key is 138 bytes; nothing near this is one.
const MAX_KEY_FILE_BYTES: u64 = 4096;

#[derive(Debug, thiserror::Error)]
pub enum VapidError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{0} must be a regular file only its owner can read")]
    Exposed(PathBuf),
    #[error("{0} does not hold a P-256 private key")]
    Malformed(PathBuf),
    #[error("the cryptography library refused the operation")]
    Crypto,
    #[error("the system clock is before 1970")]
    Clock,
}

/// The daemon's application server key.
pub struct VapidKey {
    pair: EcdsaKeyPair,
    public: String,
}

impl std::fmt::Debug for VapidKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VapidKey")
            .field("public", &self.public)
            .finish_non_exhaustive()
    }
}

impl VapidKey {
    fn from_pkcs8(pkcs8: &[u8]) -> Result<Self, aws_lc_rs::error::KeyRejected> {
        let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8)?;
        let point = pair.public_key().as_ref();
        assert_eq!(point.len(), PUBLIC_KEY_BYTES, "an uncompressed P-256 point");
        assert_eq!(point[0], 0x04, "an uncompressed P-256 point");
        let public = URL_SAFE_NO_PAD.encode(point);
        Ok(Self { pair, public })
    }

    /// The key at `path`, or `None` when push was never enabled.
    pub fn load(path: &Path) -> Result<Option<Self>, VapidError> {
        let io = |source| VapidError::Io {
            path: path.to_owned(),
            source,
        };
        let metadata = match std::fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(io(e)),
        };
        if !metadata.is_file() || !is_private(&metadata) {
            return Err(VapidError::Exposed(path.to_owned()));
        }
        if metadata.len() > MAX_KEY_FILE_BYTES {
            return Err(VapidError::Malformed(path.to_owned()));
        }
        let pkcs8 = std::fs::read(path).map_err(io)?;
        Self::from_pkcs8(&pkcs8)
            .map(Some)
            .map_err(|_| VapidError::Malformed(path.to_owned()))
    }

    /// The key at `path`, created there when it does not exist yet.
    pub fn load_or_create(path: &Path) -> Result<Self, VapidError> {
        if let Some(key) = Self::load(path)? {
            return Ok(key);
        }
        let pkcs8 =
            EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &SystemRandom::new())
                .map_err(|_| VapidError::Crypto)?;
        let key = Self::from_pkcs8(pkcs8.as_ref()).map_err(|_| VapidError::Crypto)?;
        write_private(path, pkcs8.as_ref())?;
        Ok(key)
    }

    /// The public key as devices subscribe with it: base64url, unpadded.
    pub fn public_key(&self) -> &str {
        &self.public
    }

    /// The `Authorization` header value for one request to `audience`, the
    /// push service's origin.
    pub fn authorization(
        &self,
        audience: &str,
        subject: Option<&str>,
        now: SystemTime,
    ) -> Result<String, VapidError> {
        assert!(
            audience.starts_with("https://"),
            "the audience is an https origin"
        );
        assert!(
            !audience["https://".len()..].contains('/'),
            "an origin has no path"
        );
        let issued = now
            .duration_since(UNIX_EPOCH)
            .map_err(|_| VapidError::Clock)?;
        let mut claims = serde_json::Map::new();
        claims.insert("aud".into(), audience.into());
        claims.insert("exp".into(), (issued + TOKEN_LIFETIME).as_secs().into());
        if let Some(subject) = subject {
            claims.insert("sub".into(), subject.into());
        }
        let claims = serde_json::Value::Object(claims).to_string();
        let mut token = URL_SAFE_NO_PAD.encode(JWT_HEADER);
        token.push('.');
        token.push_str(&URL_SAFE_NO_PAD.encode(claims));
        let signature = self
            .pair
            .sign(&SystemRandom::new(), token.as_bytes())
            .map_err(|_| VapidError::Crypto)?;
        token.push('.');
        token.push_str(&URL_SAFE_NO_PAD.encode(signature.as_ref()));
        Ok(format!("vapid t={token}, k={}", self.public))
    }
}

#[cfg(unix)]
fn is_private(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    metadata.mode() & 0o077 == 0
}

#[cfg(not(unix))]
fn is_private(_metadata: &std::fs::Metadata) -> bool {
    true
}

/// Write `bytes` to `path` so that no reader ever sees part of a key: a
/// private sibling file first, moved into place once it is on disk.
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), VapidError> {
    assert!(!bytes.is_empty(), "a key file is never empty");
    let staging = path.with_extension("tmp");
    assert_ne!(staging, path, "the staging file is a sibling of the key");
    let io = |at: &Path| {
        let at = at.to_owned();
        move |source| VapidError::Io { path: at, source }
    };
    match std::fs::remove_file(&staging) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(io(&staging)(e)),
    }
    let mut file = aether_core::private_file::create_new(&staging).map_err(io(&staging))?;
    file.write_all(bytes).map_err(io(&staging))?;
    file.sync_all().map_err(io(&staging))?;
    std::fs::rename(&staging, path).map_err(io(path))
}

#[cfg(test)]
mod tests;
