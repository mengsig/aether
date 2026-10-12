//! The published vectors: RFC 8291 section 5 and appendix A, and RFC 8188
//! section 3.1 and 3.2.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use super::*;
use crate::push::testing::{UserAgent, open_aes128gcm};

fn b64(text: &str) -> Vec<u8> {
    URL_SAFE_NO_PAD.decode(text).expect("base64url")
}

fn array<const N: usize>(text: &str) -> [u8; N] {
    b64(text).try_into().expect("the vector's length")
}

mod rfc8291 {
    pub const PLAINTEXT: &[u8] = b"When I grow up, I want to be a watermelon";
    pub const AS_PUBLIC: &str =
        "BP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8";
    pub const AS_PRIVATE: &str = "yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw";
    pub const UA_PUBLIC: &str =
        "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4";
    pub const UA_PRIVATE: &str = "q1dXpw3UpT5VOmu_cf_v6ih07Aems3njxI-JWgLcM94";
    pub const SALT: &str = "DGv6ra1nlYgDCS1FRnbzlw";
    pub const AUTH_SECRET: &str = "BTBZMqHH6r4Tts7J_aSIgg";
    pub const IKM: &str = "S4lYMb_L0FxCeq0WhDx813KgSYqU26kOyzWUdsXYyrg";
    pub const CEK: &str = "oIhVW04MRdy2XN9CiKLxTg";
    pub const NONCE: &str = "4h_95klXJ5E_qnoN";
    pub const BODY: &str = "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPTpK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN";
}

fn rfc8291_sender() -> agreement::PrivateKey {
    agreement::PrivateKey::from_private_key(&agreement::ECDH_P256, &b64(rfc8291::AS_PRIVATE))
        .expect("the vector's sender key")
}

fn rfc8291_recipient() -> Recipient {
    Recipient {
        p256dh: array(rfc8291::UA_PUBLIC),
        auth: array(rfc8291::AUTH_SECRET),
    }
}

#[test]
fn rfc8291_intermediate_values_match() {
    let sender = rfc8291_sender();
    let recipient = rfc8291_recipient();
    let sender_public = public_point(&sender).expect("a public key");
    assert_eq!(sender_public.to_vec(), b64(rfc8291::AS_PUBLIC));

    let ikm = shared_ikm(
        &sender,
        &recipient.p256dh,
        &recipient.auth,
        &recipient.p256dh,
        &sender_public,
    )
    .expect("key agreement");
    assert_eq!(ikm.to_vec(), b64(rfc8291::IKM));

    let keys = ContentKeys::derive(&b64(rfc8291::SALT), &ikm).expect("derivation");
    assert_eq!(keys.key.to_vec(), b64(rfc8291::CEK));
    assert_eq!(keys.nonce.to_vec(), b64(rfc8291::NONCE));
}

#[test]
fn rfc8291_message_is_reproduced_byte_for_byte() {
    let body = seal_with(
        &rfc8291_recipient(),
        &rfc8291_sender(),
        &array(rfc8291::SALT),
        rfc8291::PLAINTEXT,
    )
    .expect("sealing the vector");
    assert_eq!(URL_SAFE_NO_PAD.encode(&body), rfc8291::BODY);
}

#[test]
fn rfc8291_message_opens_with_the_user_agent_key() {
    let agent = UserAgent::from_parts(&b64(rfc8291::UA_PRIVATE), array(rfc8291::AUTH_SECRET));
    assert_eq!(agent.recipient(), rfc8291_recipient());
    let plaintext = agent
        .open(&b64(rfc8291::BODY))
        .expect("the vector decrypts");
    assert_eq!(plaintext, rfc8291::PLAINTEXT);
}

#[test]
fn rfc8188_single_record_example() {
    let ikm = b64("yqdlZ-tYemfogSmv7Ws5PQ");
    let keys = ContentKeys::derive(&b64("I1BsxtFttlv3u_Oo94xnmw"), &ikm).expect("derivation");
    assert_eq!(keys.key.to_vec(), b64("_wniytB-ofscZDh4tbSjHw"));
    assert_eq!(keys.nonce.to_vec(), b64("Bcs8gkIRKLI8GeI8"));

    let body = b64("I1BsxtFttlv3u_Oo94xnmwAAEAAA-NAVub2qFgBEuQKRapoZu-IxkIva3MEB1PD-ly8Thjg");
    let plaintext = open_aes128gcm(&body, |key_id| {
        assert!(key_id.is_empty(), "the example names no key");
        Ok(ikm.clone())
    })
    .expect("the example decrypts");
    assert_eq!(plaintext, b"I am the walrus");
}

#[test]
fn rfc8188_two_record_example_with_padding() {
    let body = b64(
        "uNCkWiNYzKTnBN9ji3-qWAAAABkCYTHOG8chz_gnvgOqdGYovxyjuqRyJFjEDyoF1Fvkj6hQPdPHI51OEUKEpgz3SsLWIqS_uA",
    );
    let plaintext = open_aes128gcm(&body, |key_id| {
        assert_eq!(key_id, b"a1");
        Ok(b64("BO3ZVPxUlnLORbVGMpbT1Q"))
    })
    .expect("the example decrypts");
    assert_eq!(plaintext, b"I am the walrus");
}

#[test]
fn a_fresh_message_round_trips_and_never_repeats() {
    let agent = UserAgent::generate();
    let first = seal(&agent.recipient(), b"hello").expect("sealing");
    let second = seal(&agent.recipient(), b"hello").expect("sealing");
    assert_ne!(first, second, "a fresh salt and sender key per message");
    assert_eq!(first.len(), HEADER_BYTES + 5 + 1 + TAG_BYTES);
    assert_eq!(agent.open(&first).expect("opening"), b"hello");
    assert_eq!(agent.open(&second).expect("opening"), b"hello");

    let stranger = UserAgent::generate();
    assert_eq!(stranger.open(&first), Err(EceError::Malformed));
}

#[test]
fn a_tampered_or_truncated_message_is_refused() {
    let agent = UserAgent::generate();
    let body = seal(&agent.recipient(), b"hello").expect("sealing");
    // Salt, sender key, ciphertext and tag. The record size is not authenticated.
    for index in [0, SALT_BYTES + 5, HEADER_BYTES, body.len() - 1] {
        let mut tampered = body.clone();
        tampered[index] ^= 0x01;
        assert!(agent.open(&tampered).is_err(), "byte {index}");
    }
    for length in [0, SALT_BYTES, HEADER_BYTES, HEADER_BYTES + TAG_BYTES] {
        assert_eq!(
            agent.open(&body[..length]),
            Err(EceError::Malformed),
            "{length}"
        );
    }
}

#[test]
fn a_message_is_bounded_by_the_push_service_limit() {
    let agent = UserAgent::generate();
    let largest = vec![b'x'; MAX_PLAINTEXT_BYTES];
    let body = seal(&agent.recipient(), &largest).expect("the largest payload");
    assert_eq!(body.len(), 4096);
    assert_eq!(agent.open(&body).expect("opening"), largest);

    let too_long = vec![b'x'; MAX_PLAINTEXT_BYTES + 1];
    assert_eq!(
        seal(&agent.recipient(), &too_long),
        Err(EceError::TooLong(MAX_PLAINTEXT_BYTES + 1))
    );
}

#[test]
fn a_key_that_is_not_on_the_curve_is_refused() {
    let mut point = [0x01u8; PUBLIC_KEY_BYTES];
    point[0] = 0x04;
    let recipient = Recipient {
        p256dh: point,
        auth: [7; AUTH_SECRET_BYTES],
    };
    assert_eq!(seal(&recipient, b"hello"), Err(EceError::BadPeerKey));
    assert!(
        !format!("{recipient:?}").contains('7'),
        "the auth secret is not printed"
    );
}
