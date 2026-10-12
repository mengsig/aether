use std::collections::HashSet;

use aether_protocol::remote::DeviceId;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use super::*;

const ORIGIN: &str = "https://box.tail.ts.net";

fn start() -> Timestamp {
    aether_core::time::now()
}

fn secs(at: Timestamp, seconds: i64) -> Timestamp {
    at + chrono::TimeDelta::seconds(seconds)
}

/// A real P-256 public key, as a device submits it.
fn public_key() -> String {
    use aws_lc_rs::signature::KeyPair as _;
    URL_SAFE_NO_PAD.encode(crate::testing::key_pair().public_key().as_ref())
}

fn claim_of(code: &str) -> PairClaimRequest {
    PairClaimRequest {
        code: code.to_owned(),
        public_key: public_key(),
        device_name: "  Robin's iPhone ".into(),
    }
}

fn opened(book: &mut PairingBook, at: Timestamp) -> (String, String) {
    let pairing = book
        .start(DeviceRole::Operator, ORIGIN, at)
        .expect("random source");
    (
        pairing.id,
        pairing.code.expect("the start reply has the code"),
    )
}

/// A code that is certainly not `code`.
fn wrong(code: &str) -> String {
    let flipped = if code.starts_with('0') { "1" } else { "0" };
    format!("{flipped}{}", &code[1..])
}

fn identity(enrolment: Enrolment<'_>) -> Result<DeviceIdentity, String> {
    Ok(DeviceIdentity {
        id: DeviceId::new(),
        name: enrolment.name.to_owned(),
        role: enrolment.role,
    })
}

fn refused(enrolment: Enrolment<'_>) -> Result<DeviceIdentity, String> {
    panic!("a rejection must not enrol {enrolment:?}")
}

#[test]
fn the_code_rides_in_the_fragment() {
    let url = pairing_url("https://box.tail.ts.net", "ABCD-EFGH");
    assert_eq!(url, "https://box.tail.ts.net/pair#code=ABCD-EFGH");
    let (sent, fragment) = url.split_once('#').expect("a fragment");
    assert!(!sent.contains("ABCD"), "{sent}");
    assert_eq!(fragment, "code=ABCD-EFGH");
}

#[test]
fn a_typed_code_is_normalized() {
    assert_eq!(normalize_code("abcd-efgh"), "ABCDEFGH");
    assert_eq!(normalize_code(" AB CD EF GH "), "ABCDEFGH");
    assert_eq!(normalize_code("ABCDEFGH").len(), CODE_CHARS);
}

#[test]
fn a_code_is_forty_bits_of_crockford_base32() {
    assert_eq!(code_from([0; 5]), "00000000");
    assert_eq!(code_from([0xff; 5]), "ZZZZZZZZ");
    assert_eq!(code_from([0, 0, 0, 0, 0x21]), "00000011");
    assert_eq!(display_code("7K2M9QXD"), "7K2M-9QXD");
    let mut book = PairingBook::default();
    let now = start();
    let codes: HashSet<String> = (0..32).map(|_| opened(&mut book, now).1).collect();
    assert_eq!(codes.len(), 32, "codes are random");
    for code in codes {
        assert_eq!(code.len(), CODE_CHARS + 1);
        assert!(
            normalize_code(&code)
                .bytes()
                .all(|b| CODE_ALPHABET.contains(&b)),
            "{code}"
        );
    }
}

#[test]
fn the_word_list_is_unambiguous() {
    let unique: HashSet<&str> = words::WORDS.iter().copied().collect();
    assert_eq!(unique.len(), words::WORDS.len(), "every word is distinct");
    for word in words::WORDS {
        assert!(
            word.len() >= 3 && word.bytes().all(|b| b.is_ascii_lowercase()),
            "{word}"
        );
    }
}

#[test]
fn the_phrase_is_fixed_by_the_pairing_and_the_key() {
    let key = [4u8; auth::PUBLIC_KEY_BYTES];
    let other = [5u8; auth::PUBLIC_KEY_BYTES];
    let phrase_a = phrase("01JABC", &key);
    assert_eq!(phrase_a, phrase("01JABC", &key));
    assert_ne!(phrase_a, phrase("01JABC", &other));
    assert_ne!(phrase_a, phrase("01JABD", &key));
    let parts: Vec<&str> = phrase_a.split('-').collect();
    assert_eq!(parts.len(), 3, "{phrase_a}");
    assert!(parts.iter().all(|part| words::WORDS.contains(part)));
}

fn open_and_check(book: &mut PairingBook, now: Timestamp) -> Pairing {
    let started = book
        .start(DeviceRole::Observer, ORIGIN, now)
        .expect("random source");
    assert_eq!(started.state, PairingState::Open);
    assert_eq!(started.expires_at, secs(now, 120));
    let code = started.code.clone().expect("code");
    assert_eq!(
        started.url.as_deref(),
        Some(format!("{ORIGIN}/pair#code={code}").as_str())
    );
    assert!(book.is_open(now));
    assert_eq!(
        book.status(&started.id, now),
        Err(HttpErrorCode::NotFound),
        "nobody has claimed it, so nobody may ask"
    );
    started
}

fn claim_and_check(
    book: &mut PairingBook,
    started: &Pairing,
    claimed_at: Timestamp,
) -> PairClaimResponse {
    let code = started.code.as_deref().expect("code");
    let typed = format!(" {} ", code.to_lowercase());
    let claimed = book
        .claim(&claim_of(&typed), claimed_at)
        .expect("right code");
    assert_eq!(claimed.pairing, started.id);
    assert_eq!(claimed.fingerprint.len(), 64);
    assert_eq!(claimed.expires_at, secs(claimed_at, 120), "a new window");
    assert!(!book.is_open(claimed_at));

    let read = book.get(&started.id, claimed_at).expect("known");
    assert_eq!(read.state, PairingState::Claimed);
    assert_eq!((read.code, read.url), (None, None), "never repeated");
    let candidate = read.candidate.expect("claimed");
    assert_eq!(candidate.name, "Robin's iPhone");
    assert_eq!(candidate.phrase, claimed.phrase);
    assert_eq!(candidate.fingerprint, claimed.fingerprint);
    let waiting = book.status(&started.id, claimed_at).expect("claimant");
    assert_eq!(
        (waiting.state, waiting.device),
        (PairingState::Claimed, None)
    );
    claimed
}

#[test]
fn a_pairing_runs_from_open_through_claimed_to_enrolled() {
    let mut book = PairingBook::default();
    let now = start();
    let started = open_and_check(&mut book, now);
    let claimed_at = secs(now, 100);
    let claimed = claim_and_check(&mut book, &started, claimed_at);

    let confirmed_at = secs(claimed_at, 60);
    let enrolled = book
        .confirm(&started.id, true, confirmed_at, |enrolment| {
            assert_eq!(enrolment.role, DeviceRole::Observer);
            assert_eq!(enrolment.fingerprint, claimed.fingerprint);
            assert_eq!(enrolment.public_key.len(), auth::PUBLIC_KEY_BYTES);
            identity(enrolment)
        })
        .expect("claimed");
    assert_eq!(enrolled.state, PairingState::Enrolled);
    let device = enrolled.device.clone().expect("enrolled");
    assert_eq!(device.role, DeviceRole::Observer);
    let told = book.status(&started.id, confirmed_at).expect("claimant");
    assert_eq!(told.state, PairingState::Enrolled);
    assert_eq!(told.device, Some(device));

    let again = book
        .confirm(&started.id, true, confirmed_at, refused)
        .expect("the same answer again is the same reply");
    assert_eq!(again, enrolled);
    assert!(matches!(
        book.confirm(&started.id, false, confirmed_at, refused),
        Err(PairingError::NotClaimed { .. })
    ));
}

#[test]
fn a_code_is_single_use() {
    let mut book = PairingBook::default();
    let now = start();
    let (id, code) = opened(&mut book, now);
    book.claim(&claim_of(&code), now).expect("first claim");
    let second = book.claim(&claim_of(&code), now).expect_err("spent");
    assert_eq!(second.refusal.code, HttpErrorCode::InvalidCode);
    assert!(second.delayed);
    book.confirm(&id, true, now, identity).expect("claimed");
    let after_enrolment = book.claim(&claim_of(&code), now).expect_err("spent");
    assert_eq!(after_enrolment.refusal.code, HttpErrorCode::InvalidCode);
}

#[test]
fn an_unclaimed_code_expires_and_so_does_an_unconfirmed_claim() {
    let mut book = PairingBook::default();
    let now = start();
    let (id, code) = opened(&mut book, now);
    let late = book
        .claim(&claim_of(&code), secs(now, 120))
        .expect_err("120 s is the whole window");
    assert_eq!(late.refusal.code, HttpErrorCode::Expired);
    assert!(!late.delayed);
    assert_eq!(
        book.get(&id, secs(now, 121))
            .map(|p| (p.state, p.expires_at)),
        Ok((PairingState::Expired, secs(now, 120)))
    );

    let (id, code) = opened(&mut book, secs(now, 200));
    book.claim(&claim_of(&code), secs(now, 210))
        .expect("in time");
    let too_late = secs(now, 210 + 120);
    assert!(matches!(
        book.confirm(&id, true, too_late, refused),
        Err(PairingError::NotClaimed {
            state: "expired",
            ..
        })
    ));
    assert_eq!(
        book.status(&id, too_late).map(|status| status.state),
        Ok(PairingState::Expired)
    );
}

#[test]
fn wrong_codes_never_close_the_pairing() {
    let mut book = PairingBook::default();
    let now = start();
    let (id, code) = opened(&mut book, now);
    let guess = claim_of(&wrong(&code));
    for second in 0..60 {
        let refusal = book.claim(&guess, secs(now, second)).expect_err("wrong");
        assert_eq!(refusal.refusal.code, HttpErrorCode::InvalidCode, "{second}");
        assert!(refusal.delayed, "a wrong code waits for its answer");
    }
    assert_eq!(
        book.get(&id, secs(now, 60)).map(|pairing| pairing.state),
        Ok(PairingState::Open)
    );
    book.claim(&claim_of(&code), secs(now, 60))
        .expect("the right code still works");
}

#[test]
fn guesses_are_rate_limited_across_all_peers() {
    let mut book = PairingBook::default();
    let now = start();
    let (_, code) = opened(&mut book, now);
    let guess = claim_of(&wrong(&code));
    for _ in 0..MAX_GUESSES_PER_SECOND {
        let refusal = book.claim(&guess, now).expect_err("wrong");
        assert_eq!(refusal.refusal.code, HttpErrorCode::InvalidCode);
    }
    let soon = now + chrono::TimeDelta::milliseconds(400);
    let limited = book
        .claim(&claim_of(&code), soon)
        .expect_err("the eleventh in a second, though the code is right");
    assert_eq!(limited.refusal.code, HttpErrorCode::RateLimited);
    assert_eq!(limited.refusal.retry_after, Some(1));
    assert!(!limited.delayed);
    assert!(book.is_open(soon), "a refused claim spends nothing");
    book.claim(&claim_of(&code), secs(now, 1))
        .expect("the window moved on");
}

#[test]
fn a_flood_gets_no_more_guesses_than_the_documented_bound() {
    const CLAIMS_PER_SECOND: i64 = 50;
    let mut book = PairingBook::default();
    let now = start();
    let (id, code) = opened(&mut book, now);
    let guess = claim_of(&wrong(&code));
    let window = i64::try_from(CODE_TTL.as_secs()).expect("two minutes");
    let mut compared = 0u64;
    for claim in 0..window * CLAIMS_PER_SECOND {
        let at = now + chrono::TimeDelta::milliseconds(claim * 1000 / CLAIMS_PER_SECOND);
        let refusal = book.claim(&guess, at).expect_err("wrong");
        match refusal.refusal.code {
            HttpErrorCode::InvalidCode => compared += 1,
            HttpErrorCode::RateLimited => {}
            other => panic!("claim {claim} was answered {other:?}"),
        }
    }
    assert_eq!(
        compared,
        u64::from(MAX_GUESSES_PER_SECOND) * CODE_TTL.as_secs(),
        "docs/remote.md derives the odds of a guess from this"
    );
    assert_eq!(
        book.get(&id, secs(now, window))
            .map(|pairing| pairing.state),
        Ok(PairingState::Expired),
        "and then the code is gone"
    );
}

#[test]
fn only_a_code_that_could_be_right_is_counted_as_a_guess() {
    let mut book = PairingBook::default();
    let now = start();
    for attempt in 0..=MAX_GUESSES_PER_SECOND {
        let closed = book
            .claim(&claim_of("AAAA-AAAA"), now)
            .expect_err("none open");
        assert_eq!(
            closed.refusal.code,
            HttpErrorCode::PairingClosed,
            "{attempt}"
        );
    }
    let (_, code) = opened(&mut book, now);
    let misshapen = [
        "",
        "AAAA",
        "AAAA-AAAA-A",
        "IIII-LLLL",
        "\u{e9}\u{e9}\u{e9}\u{e9}",
    ];
    for _ in 0..=MAX_GUESSES_PER_SECOND {
        for typed in misshapen {
            let refusal = book.claim(&claim_of(typed), now).expect_err("no code");
            assert_eq!(refusal.refusal.code, HttpErrorCode::InvalidCode, "{typed}");
            assert!(!refusal.delayed, "nothing was compared: {typed}");
        }
    }
    book.claim(&claim_of(&code), now)
        .expect("none of that was a guess");
}

#[test]
fn a_malformed_claim_is_a_bad_request_and_spends_no_attempt() {
    let mut book = PairingBook::default();
    let now = start();
    let (_, code) = opened(&mut book, now);
    let mut bad_key = claim_of(&code);
    bad_key.public_key = "AAAA".into();
    let mut bad_name = claim_of(&code);
    bad_name.device_name = "a\nb".into();
    let mut compressed = claim_of(&code);
    compressed.public_key = URL_SAFE_NO_PAD.encode([2u8; auth::PUBLIC_KEY_BYTES]);
    let mut off_curve = claim_of(&code);
    let mut point = [7u8; auth::PUBLIC_KEY_BYTES];
    point[0] = 0x04;
    off_curve.public_key = URL_SAFE_NO_PAD.encode(point);
    for _ in 0..=MAX_GUESSES_PER_SECOND {
        for bad in [&bad_key, &bad_name, &compressed, &off_curve] {
            let refusal = book.claim(bad, now).expect_err("malformed");
            assert_eq!(refusal.refusal.code, HttpErrorCode::BadRequest, "{bad:?}");
        }
    }
    book.claim(&claim_of(&code), now)
        .expect("the code survived, and so did the guess budget");
}

#[test]
fn a_new_pairing_cancels_the_one_before_it() {
    let mut book = PairingBook::default();
    let now = start();
    let (first, first_code) = opened(&mut book, now);
    book.claim(&claim_of(&first_code), now).expect("claimed");
    let (second, second_code) = opened(&mut book, now);
    assert_ne!(first, second);
    assert_eq!(
        book.status(&first, now).map(|status| status.state),
        Ok(PairingState::Cancelled)
    );
    assert!(matches!(
        book.confirm(&first, true, now, refused),
        Err(PairingError::NotClaimed {
            state: "cancelled",
            ..
        })
    ));
    let stale = book
        .claim(&claim_of(&first_code), now)
        .expect_err("old code");
    assert_eq!(stale.refusal.code, HttpErrorCode::InvalidCode);
    book.claim(&claim_of(&second_code), now)
        .expect("the new code");
}

#[test]
fn a_rejection_enrols_nothing_and_repeats() {
    let mut book = PairingBook::default();
    let now = start();
    let (id, code) = opened(&mut book, now);
    book.claim(&claim_of(&code), now).expect("claimed");
    let rejected = book.confirm(&id, false, now, refused).expect("claimed");
    assert_eq!(rejected.state, PairingState::Rejected);
    assert_eq!(rejected.device, None);
    assert!(
        rejected.candidate.is_some(),
        "the audit names who was refused"
    );
    assert_eq!(book.confirm(&id, false, now, refused), Ok(rejected));
    let told = book.status(&id, now).expect("claimant");
    assert_eq!((told.state, told.device), (PairingState::Rejected, None));
}

#[test]
fn a_failed_enrolment_leaves_the_pairing_claimed() {
    let mut book = PairingBook::default();
    let now = start();
    let (id, code) = opened(&mut book, now);
    book.claim(&claim_of(&code), now).expect("claimed");
    let failed = book.confirm(&id, true, now, |_| Err("disk full".into()));
    assert_eq!(failed, Err(PairingError::Enrol("disk full".into())));
    assert_eq!(
        book.get(&id, now).map(|pairing| pairing.state),
        Ok(PairingState::Claimed)
    );
    book.confirm(&id, true, now, identity).expect("retried");
}

#[test]
fn cancelling_is_idempotent_and_never_undoes_an_outcome() {
    let mut book = PairingBook::default();
    let now = start();
    let (id, code) = opened(&mut book, now);
    assert_eq!(
        book.cancel(&id, now).map(|pairing| pairing.state),
        Ok(PairingState::Cancelled)
    );
    assert_eq!(
        book.cancel(&id, now).map(|pairing| pairing.state),
        Ok(PairingState::Cancelled)
    );
    assert_eq!(book.cancel("nope", now), Err(PairingError::NotFound));
    assert_eq!(book.cancel_live(now), None);
    let closed = book.claim(&claim_of(&code), now).expect_err("cancelled");
    assert_eq!(closed.refusal.code, HttpErrorCode::PairingClosed);

    let (id, code) = opened(&mut book, now);
    book.claim(&claim_of(&code), now).expect("claimed");
    book.confirm(&id, true, now, identity).expect("enrolled");
    assert_eq!(
        book.cancel(&id, now).map(|pairing| pairing.state),
        Ok(PairingState::Enrolled)
    );
}

#[test]
fn an_outcome_is_forgotten_after_five_minutes() {
    let mut book = PairingBook::default();
    let now = start();
    let (id, code) = opened(&mut book, now);
    book.claim(&claim_of(&code), now).expect("claimed");
    book.confirm(&id, false, now, refused).expect("rejected");
    assert!(book.get(&id, secs(now, 299)).is_ok());
    assert_eq!(book.get(&id, secs(now, 300)), Err(PairingError::NotFound));
    assert_eq!(
        book.status(&id, secs(now, 300)),
        Err(HttpErrorCode::NotFound)
    );
    let closed = book
        .claim(&claim_of(&code), secs(now, 300))
        .expect_err("forgotten");
    assert_eq!(closed.refusal.code, HttpErrorCode::PairingClosed);
}

#[test]
fn only_a_bounded_number_of_outcomes_is_remembered() {
    let mut book = PairingBook::default();
    let now = start();
    let ids: Vec<String> = (0..MAX_REMEMBERED + 4)
        .map(|_| opened(&mut book, now).0)
        .collect();
    assert_eq!(book.get(&ids[0], now), Err(PairingError::NotFound));
    let newest = ids.last().expect("started some");
    assert_eq!(
        book.get(newest, now).map(|pairing| pairing.state),
        Ok(PairingState::Open)
    );
    assert!(book.entries.len() <= MAX_REMEMBERED + 1);
}
