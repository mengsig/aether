//! The shared secret on a webhook, in both directions.
//!
//! HMAC-SHA256, hex, in one header - the shape GitHub and Stripe use, so the
//! other side of a hook usually already knows how to produce or check it.
//!
//! What is signed is not the body alone. A signature over the body alone is
//! valid forever, and a captured POST replayed is another trigger waking an
//! agent that runs tools; the listener is plaintext by design (a TLS
//! terminator is the operator's job), so a captured request is not a strong
//! assumption. So each delivery carries a timestamp and a nonce, both signed,
//! both refused outside the window by [`super::replay`].
//!
//! Signature bytes omit the route for receiver compatibility. Inbound routes
//! require secrets distinct from other routes and outbound subscriptions.

use hmac::{Hmac, KeyInit, Mac};
use rand::RngExt as _;
use sha2::Sha256;

/// The header carrying the signature, in both directions.
pub const HEADER: &str = "x-aether-signature";

/// Unix seconds this delivery was signed at. Signed, so it cannot be moved.
pub const HEADER_TIMESTAMP: &str = "x-aether-timestamp";

/// A value unique to this delivery. Signed, so it cannot be swapped for one
/// the receiver has not seen.
pub const HEADER_NONCE: &str = "x-aether-nonce";

/// Optional sender delivery identifier; the signed nonce is always checked too.
pub const HEADER_DELIVERY_ID: &str = "x-aether-delivery-id";

/// Minimum inbound signing key length in bytes.
pub const MIN_SECRET_BYTES: usize = 32;

/// The only prefix accepted, so the algorithm is never inferred.
const PREFIX: &str = "sha256=";

/// Nonce length in characters, as this side mints them.
const NONCE_CHARS: usize = 32;

/// Longest nonce accepted. A bound is what keeps the seen-set's memory a
/// function of the request rate rather than of what a caller sends.
const MAX_NONCE_CHARS: usize = 128;

/// What pins one delivery to one moment: when it was signed, and a value that
/// identifies it. Both travel in headers and both are inside the HMAC, so
/// neither can be edited without invalidating the signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stamp {
    /// Unix seconds.
    pub timestamp: i64,
    pub nonce: String,
}

impl Stamp {
    /// A stamp for a delivery being sent now.
    pub fn now() -> Stamp {
        // Not a secret: it identifies a delivery, it does not authorize one.
        // Uniqueness is the whole requirement, and 32 base32 characters of
        // the thread rng gives it with room to spare.
        const ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
        let mut rng = rand::rng();
        let nonce = (0..NONCE_CHARS)
            .map(|_| ALPHABET[rng.random_range(0..ALPHABET.len())] as char)
            .collect();
        Stamp {
            timestamp: aether_core::time::now().timestamp(),
            nonce,
        }
    }

    /// A stamp as it arrived, or why it cannot be one. Both fields are
    /// validated here rather than at the HMAC, so what is signed is always a
    /// shape the payload encoding can represent unambiguously.
    pub fn parse(timestamp: &str, nonce: &str) -> Result<Stamp, String> {
        let timestamp = timestamp
            .trim()
            .parse::<i64>()
            .map_err(|_| format!("{HEADER_TIMESTAMP} must be unix seconds"))?;
        let nonce = nonce.trim();
        if nonce.is_empty() || nonce.len() > MAX_NONCE_CHARS {
            return Err(format!(
                "{HEADER_NONCE} must be 1 to {MAX_NONCE_CHARS} characters"
            ));
        }
        if !nonce
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(format!(
                "{HEADER_NONCE} may hold only letters, digits, `-` and `_`"
            ));
        }
        Ok(Stamp {
            timestamp,
            nonce: nonce.to_owned(),
        })
    }
}

/// The bytes the HMAC actually covers: the stamp, then the body. A timestamp
/// is digits and a nonce holds no `.`, so splitting on the separators is
/// unambiguous and no body can impersonate a different stamp.
pub fn signed_payload(stamp: &Stamp, body: &[u8]) -> Vec<u8> {
    let mut payload = format!("{}.{}.", stamp.timestamp, stamp.nonce).into_bytes();
    payload.extend_from_slice(body);
    payload
}

/// The signature for a payload under a secret, as it goes in the header, or
/// `None` when the secret cannot key an HMAC. HMAC takes a key of any length,
/// so that is not a case anyone will meet - but a caller must not send an
/// unsigned body believing it signed one, so it is returned rather than
/// asserted away.
pub fn sign(secret: &str, payload: &[u8]) -> Option<String> {
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(secret.as_bytes()).ok()?;
    mac.update(payload);
    Some(format!(
        "{PREFIX}{}",
        hex::encode(mac.finalize().into_bytes())
    ))
}

/// Whether `presented` is this payload's signature. Compared in constant time
/// by the MAC itself, and only after the algorithm prefix matches.
pub fn verify(secret: &str, payload: &[u8], presented: &str) -> bool {
    let Some(hex_digest) = presented.trim().strip_prefix(PREFIX) else {
        return false;
    };
    let Ok(expected) = hex::decode(hex_digest) else {
        return false;
    };
    let Ok(mut mac) = <Hmac<Sha256> as KeyInit>::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(payload);
    mac.verify_slice(&expected).is_ok()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;

    fn stamp() -> Stamp {
        Stamp {
            timestamp: 1_700_000_000,
            nonce: "abc123".into(),
        }
    }

    #[test]
    fn a_signature_verifies_against_its_own_payload_and_nothing_else() {
        let body = br#"{"ref":"main"}"#;
        let payload = signed_payload(&stamp(), body);
        let signature = sign("secret", &payload).expect("signed");
        assert!(signature.starts_with(PREFIX));
        assert!(verify("secret", &payload, &signature));
        assert!(!verify("other secret", &payload, &signature));
        assert!(!verify("secret", b"{}", &signature));
    }

    /// The point of signing the stamp: the same body under a different
    /// timestamp or nonce is a different signature, so neither header can be
    /// edited on a captured request.
    #[test]
    fn the_stamp_is_inside_the_signature() {
        let body = b"payload";
        let signature = sign("secret", &signed_payload(&stamp(), body)).expect("signed");
        let moved = Stamp {
            timestamp: 1_700_000_001,
            ..stamp()
        };
        assert!(!verify("secret", &signed_payload(&moved, body), &signature));
        let renonced = Stamp {
            nonce: "abc124".into(),
            ..stamp()
        };
        assert!(!verify(
            "secret",
            &signed_payload(&renonced, body),
            &signature
        ));
    }

    #[test]
    fn a_malformed_signature_is_refused_rather_than_parsed() {
        let payload = signed_payload(&stamp(), b"payload");
        for presented in ["", "sha256=", "deadbeef", "sha1=deadbeef", "sha256=nothex"] {
            assert!(!verify("secret", &payload, presented), "{presented}");
        }
    }

    #[test]
    fn a_stamp_is_parsed_only_in_the_shape_the_payload_can_encode() {
        assert_eq!(
            Stamp::parse(" 17 ", " a-b_C9 ").expect("parsed").timestamp,
            17
        );
        for (timestamp, nonce) in [
            ("not-a-number", "abc"),
            ("17", ""),
            // A `.` would make the signed payload ambiguous.
            ("17", "a.b"),
            ("17", &"n".repeat(MAX_NONCE_CHARS + 1)),
        ] {
            assert!(
                Stamp::parse(timestamp, nonce).is_err(),
                "{timestamp} {nonce}"
            );
        }
    }

    #[test]
    fn a_minted_stamp_is_fresh_and_unique() {
        let (a, b) = (Stamp::now(), Stamp::now());
        assert_ne!(a.nonce, b.nonce);
        assert_eq!(a.nonce.len(), NONCE_CHARS);
        assert!(Stamp::parse(&a.timestamp.to_string(), &a.nonce).is_ok());
    }

    /// RFC 4231 test case 2: the construction, not just our own round trip.
    #[test]
    fn the_construction_matches_the_published_vector() {
        assert_eq!(
            sign("Jefe", b"what do ya want for nothing?").as_deref(),
            Some("sha256=5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843")
        );
    }
}
