//! Pairing: a single-use code on the host, a device's claim, and the host's confirmation.
//! One pairing is open at a time, in memory only; docs/remote.md has the state machine.

mod words;

use std::collections::VecDeque;
use std::time::Duration;

use aether_core::time::Timestamp;
use aether_protocol::auth::constant_time_eq;
use aether_protocol::remote::{
    DeviceIdentity, DeviceRole, HttpErrorCode, PairClaimRequest, PairClaimResponse,
    PairStatusResponse, Pairing, PairingCandidate, PairingState,
};

use crate::auth;
use crate::http::Refusal;

/// How long a code may be claimed, and a claim confirmed.
pub const CODE_TTL: Duration = Duration::from_secs(120);
/// Characters in a code, excluding the separating dash.
pub const CODE_CHARS: usize = 8;
/// Crockford base32: no `I`, `L`, `O` or `U` to misread.
pub const CODE_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
/// Codes compared with the open pairing's per second, across all peers and
/// right or wrong. Over [`CODE_TTL`] that is every guess one code can face.
pub const MAX_GUESSES_PER_SECOND: u32 = 10;
/// The route the pairing URL opens; the code rides in its fragment.
pub const PAIR_ROUTE: &str = "/pair";
/// How long a wrong code waits for its answer.
pub const WRONG_CODE_DELAY: Duration = Duration::from_millis(250);
/// How long a finished pairing stays readable by the device and the host.
pub const OUTCOME_TTL: Duration = Duration::from_secs(300);
/// Finished pairings remembered at once; the oldest is forgotten first.
const MAX_REMEMBERED: usize = 16;
/// The window `MAX_GUESSES_PER_SECOND` is counted over.
const GUESS_WINDOW: Duration = Duration::from_secs(1);
/// Separates the phrase digest from every other use of SHA-256 here.
const PHRASE_CONTEXT: &[u8] = b"aether-remote-pairing-phrase-v1\n";
/// Words in a phrase.
const PHRASE_WORDS: usize = 3;

/// The pairing URL for an origin: the code is in the fragment, which a
/// browser never sends to a server or a proxy.
pub fn pairing_url(origin: &str, code: &str) -> String {
    debug_assert!(!origin.ends_with('/'), "an origin has no path: {origin}");
    format!("{origin}{PAIR_ROUTE}#code={code}")
}

/// A code as typed: case-insensitive, dashes and spaces ignored.
pub fn normalize_code(typed: &str) -> String {
    typed
        .chars()
        .filter(|c| !matches!(c, '-' | ' '))
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// Whether a normalized code could be one at all. One that could not is
/// refused without being compared, so it tells its sender nothing.
fn is_code_shaped(normalized: &str) -> bool {
    normalized.len() == CODE_CHARS && normalized.bytes().all(|b| CODE_ALPHABET.contains(&b))
}

/// A code as printed: `XXXX-XXXX`.
pub fn display_code(code: &str) -> String {
    debug_assert_eq!(code.len(), CODE_CHARS, "a normalized code");
    let (head, tail) = code.split_at(code.len() / 2);
    format!("{head}-{tail}")
}

/// Forty random bits as eight Crockford symbols.
fn code_from(random: [u8; 5]) -> String {
    let bits = random
        .iter()
        .fold(0u64, |bits, byte| (bits << 8) | u64::from(*byte));
    let code: String = (0..CODE_CHARS)
        .rev()
        .map(|symbol| char::from(CODE_ALPHABET[((bits >> (symbol * 5)) & 0x1f) as usize]))
        .collect();
    debug_assert_eq!(code.len(), CODE_CHARS);
    code
}

/// The words both screens show: fixed by the pairing and the claimed key,
/// so a different key claiming the same code shows different words.
pub fn phrase(pairing: &str, public_key: &[u8]) -> String {
    use sha2::Digest as _;
    debug_assert_eq!(
        public_key.len(),
        auth::PUBLIC_KEY_BYTES,
        "a raw P-256 point"
    );
    let mut hasher = sha2::Sha256::new();
    hasher.update(PHRASE_CONTEXT);
    hasher.update(pairing.as_bytes());
    hasher.update(b"\n");
    hasher.update(public_key);
    let digest = hasher.finalize();
    let chosen: Vec<&str> = digest
        .iter()
        .take(PHRASE_WORDS)
        .map(|byte| words::WORDS[usize::from(*byte)])
        .collect();
    chosen.join("-")
}

fn after(at: Timestamp, span: Duration) -> Timestamp {
    at + chrono::TimeDelta::from_std(span).unwrap_or(chrono::TimeDelta::MAX)
}

/// Why the host could not act on a pairing.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PairingError {
    #[error("no such pairing; it may have been forgotten")]
    NotFound,
    #[error("the pairing is {state}; only a claimed pairing can be {asked}")]
    NotClaimed {
        state: &'static str,
        asked: &'static str,
    },
    #[error("{0}")]
    Disabled(&'static str),
    #[error("the system random source failed")]
    Random,
    #[error("could not enrol the device: {0}")]
    Enrol(String),
}

/// A claim that was refused, and whether its answer must wait.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimRefusal {
    pub refusal: Refusal,
    /// Answer after [`WRONG_CODE_DELAY`], so guessing is slow.
    pub delayed: bool,
}

impl ClaimRefusal {
    fn now(code: HttpErrorCode, message: &'static str) -> Self {
        Self {
            refusal: Refusal::new(code, message),
            delayed: false,
        }
    }

    fn wrong_code() -> Self {
        Self {
            delayed: true,
            ..Self::now(HttpErrorCode::InvalidCode, WRONG_CODE)
        }
    }
}

const WRONG_CODE: &str = "that code is not open for pairing";

/// A claim whose key or name is not of the documented shape.
fn malformed(error: auth::AuthError) -> ClaimRefusal {
    let message = match error {
        auth::AuthError::BadName => {
            "device_name must be 1 to 64 characters with no control, formatting or invisible characters"
        }
        _ => "public_key must be a raw uncompressed P-256 point, base64url without padding",
    };
    ClaimRefusal::now(HttpErrorCode::BadRequest, message)
}

/// What the host confirms: the device as it claimed, and the role it gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Enrolment<'a> {
    pub name: &'a str,
    /// Raw P-256 point, [`auth::PUBLIC_KEY_BYTES`] long.
    pub public_key: &'a [u8],
    pub fingerprint: &'a str,
    pub role: DeviceRole,
}

#[derive(Debug, Clone)]
struct Claimant {
    name: String,
    public_key: Vec<u8>,
    fingerprint: String,
    phrase: String,
    claimed_at: Timestamp,
}

#[derive(Debug, Clone)]
struct Entry {
    id: String,
    /// Normalized: [`CODE_CHARS`] symbols, no dash.
    code: String,
    state: PairingState,
    role: DeviceRole,
    expires_at: Timestamp,
    claimant: Option<Claimant>,
    device: Option<DeviceIdentity>,
    /// When it reached a terminal state.
    ended_at: Option<Timestamp>,
}

impl Entry {
    fn is_live(&self) -> bool {
        matches!(self.state, PairingState::Open | PairingState::Claimed)
    }

    fn end(&mut self, state: PairingState, at: Timestamp) {
        debug_assert!(self.is_live(), "a terminal pairing never changes");
        debug_assert!(!matches!(state, PairingState::Open | PairingState::Claimed));
        self.state = state;
        self.ended_at = Some(at);
    }

    /// The pairing as the host reads it; the code is never repeated.
    fn view(&self) -> Pairing {
        Pairing {
            id: self.id.clone(),
            state: self.state,
            role: self.role,
            expires_at: self.expires_at,
            code: None,
            url: None,
            candidate: self.claimant.as_ref().map(|claimant| PairingCandidate {
                name: claimant.name.clone(),
                fingerprint: claimant.fingerprint.clone(),
                phrase: claimant.phrase.clone(),
                claimed_at: claimant.claimed_at,
            }),
            device: self.device.clone(),
        }
    }
}

fn state_name(state: PairingState) -> &'static str {
    match state {
        PairingState::Open => "open",
        PairingState::Claimed => "claimed",
        PairingState::Enrolled => "enrolled",
        PairingState::Rejected => "rejected",
        PairingState::Expired => "expired",
        PairingState::Cancelled => "cancelled",
    }
}

/// The pairing in progress, the outcomes still being read, and the claim
/// rate limit. Every method takes the time, so the clock is the caller's.
#[derive(Debug, Default)]
pub struct PairingBook {
    /// Oldest first. At most one, the last, is `open` or `claimed`.
    entries: VecDeque<Entry>,
    /// When each code compared within the last window arrived, oldest first.
    guesses: VecDeque<Timestamp>,
}

impl PairingBook {
    /// Expire what ran out of time and forget what nobody reads any more.
    fn settle(&mut self, now: Timestamp) {
        for entry in &mut self.entries {
            if entry.is_live() && entry.expires_at <= now {
                let at = entry.expires_at;
                entry.end(PairingState::Expired, at);
            }
        }
        self.entries.retain(|entry| {
            entry
                .ended_at
                .is_none_or(|ended| after(ended, OUTCOME_TTL) > now)
        });
        while self.entries.len() > MAX_REMEMBERED {
            self.entries.pop_front();
        }
        debug_assert!(self.entries.iter().filter(|entry| entry.is_live()).count() <= 1);
    }

    fn find(&mut self, pairing: &str) -> Option<&mut Entry> {
        self.entries.iter_mut().find(|entry| entry.id == pairing)
    }

    /// Open a pairing for `role`, cancelling one still open or claimed.
    /// The reply is the only place the code and the URL ever appear.
    pub fn start(
        &mut self,
        role: DeviceRole,
        origin: &str,
        now: Timestamp,
    ) -> Result<Pairing, PairingError> {
        let mut random = [0u8; 5];
        aws_lc_rs::rand::fill(&mut random).map_err(|_| PairingError::Random)?;
        self.cancel_live(now);
        let entry = Entry {
            id: ulid::Ulid::generate().to_string(),
            code: code_from(random),
            state: PairingState::Open,
            role,
            expires_at: after(now, CODE_TTL),
            claimant: None,
            device: None,
            ended_at: None,
        };
        let shown = display_code(&entry.code);
        let mut pairing = entry.view();
        pairing.url = Some(pairing_url(origin, &shown));
        pairing.code = Some(shown);
        self.entries.push_back(entry);
        Ok(pairing)
    }

    /// Admit one comparison to the rate limit, or say how long to wait.
    fn admit_guess(guesses: &mut VecDeque<Timestamp>, now: Timestamp) -> Result<(), ClaimRefusal> {
        while let Some(oldest) = guesses.front() {
            if after(*oldest, GUESS_WINDOW) > now {
                break;
            }
            guesses.pop_front();
        }
        debug_assert!(guesses.len() <= MAX_GUESSES_PER_SECOND as usize);
        if guesses.len() < MAX_GUESSES_PER_SECOND as usize {
            guesses.push_back(now);
            return Ok(());
        }
        let wait = guesses
            .front()
            .map(|oldest| after(*oldest, GUESS_WINDOW) - now)
            .and_then(|wait| wait.to_std().ok())
            .unwrap_or(GUESS_WINDOW);
        let mut refusal = Refusal::new(
            HttpErrorCode::RateLimited,
            "too many pairing attempts; wait and try again",
        );
        let whole_seconds = wait.as_secs() + u64::from(wait.subsec_nanos() > 0);
        refusal.retry_after = Some(whole_seconds.max(1));
        Err(ClaimRefusal {
            refusal,
            delayed: false,
        })
    }

    /// The pairing a claim is made against, if it can still be claimed.
    fn claimable(entries: &mut VecDeque<Entry>) -> Result<&mut Entry, ClaimRefusal> {
        let closed = || {
            ClaimRefusal::now(
                HttpErrorCode::PairingClosed,
                "no pairing is open; run `aether remote pair` on the host",
            )
        };
        let entry = entries.back_mut().ok_or_else(closed)?;
        match entry.state {
            PairingState::Open => Ok(entry),
            PairingState::Expired => Err(ClaimRefusal::now(
                HttpErrorCode::Expired,
                "the pairing expired; start a new one on the host",
            )),
            PairingState::Cancelled => Err(closed()),
            // Already claimed: the code is spent, whoever presents it.
            PairingState::Claimed | PairingState::Enrolled | PairingState::Rejected => {
                Err(ClaimRefusal::wrong_code())
            }
        }
    }

    /// A device presenting the code with its key and name. The first valid claim consumes the code;
    /// only a claim whose code is compared spends the guess budget (docs/remote.md).
    pub fn claim(
        &mut self,
        request: &PairClaimRequest,
        now: Timestamp,
    ) -> Result<PairClaimResponse, ClaimRefusal> {
        self.settle(now);
        let public_key = auth::decode_public_key(&request.public_key).map_err(malformed)?;
        let name = auth::check_device_name(&request.device_name).map_err(malformed)?;
        let typed = normalize_code(&request.code);
        let entry = Self::claimable(&mut self.entries)?;
        if !is_code_shaped(&typed) {
            return Err(ClaimRefusal::now(HttpErrorCode::InvalidCode, WRONG_CODE));
        }
        Self::admit_guess(&mut self.guesses, now)?;
        debug_assert_eq!(
            entry.state,
            PairingState::Open,
            "only an open code is compared"
        );
        if !constant_time_eq(&typed, &entry.code) {
            return Err(ClaimRefusal::wrong_code());
        }
        let claimant = Claimant {
            fingerprint: auth::fingerprint(&public_key),
            phrase: phrase(&entry.id, &public_key),
            name,
            public_key,
            claimed_at: now,
        };
        entry.state = PairingState::Claimed;
        entry.expires_at = after(now, CODE_TTL);
        let response = PairClaimResponse {
            pairing: entry.id.clone(),
            fingerprint: claimant.fingerprint.clone(),
            phrase: claimant.phrase.clone(),
            expires_at: entry.expires_at,
        };
        entry.claimant = Some(claimant);
        Ok(response)
    }

    /// What the claiming device sees while it waits. Only a pairing somebody
    /// claimed has a claimant to answer; any other id is unknown.
    pub fn status(
        &mut self,
        pairing: &str,
        now: Timestamp,
    ) -> Result<PairStatusResponse, HttpErrorCode> {
        self.settle(now);
        let entry = self
            .find(pairing)
            .filter(|entry| entry.claimant.is_some())
            .ok_or(HttpErrorCode::NotFound)?;
        debug_assert!(
            entry.state != PairingState::Open,
            "a claimant means claimed"
        );
        Ok(PairStatusResponse {
            state: entry.state,
            device: entry.device.clone(),
        })
    }

    /// The pairing as the host reads it; the code is never repeated.
    pub fn get(&mut self, pairing: &str, now: Timestamp) -> Result<Pairing, PairingError> {
        self.settle(now);
        self.find(pairing)
            .map(|entry| entry.view())
            .ok_or(PairingError::NotFound)
    }

    /// The host's decision on a claimed pairing. `enrol` makes the device
    /// durable and runs only on acceptance; if it fails, nothing changes.
    pub fn confirm(
        &mut self,
        pairing: &str,
        accept: bool,
        now: Timestamp,
        enrol: impl FnOnce(Enrolment<'_>) -> Result<DeviceIdentity, String>,
    ) -> Result<Pairing, PairingError> {
        self.settle(now);
        let entry = self.find(pairing).ok_or(PairingError::NotFound)?;
        let (wanted, asked) = match accept {
            true => (PairingState::Enrolled, "accepted"),
            false => (PairingState::Rejected, "rejected"),
        };
        if entry.state == wanted {
            return Ok(entry.view());
        }
        let claimant = match (&entry.claimant, entry.state) {
            (Some(claimant), PairingState::Claimed) => claimant,
            _ => {
                return Err(PairingError::NotClaimed {
                    state: state_name(entry.state),
                    asked,
                });
            }
        };
        if accept {
            let device = enrol(Enrolment {
                name: &claimant.name,
                public_key: &claimant.public_key,
                fingerprint: &claimant.fingerprint,
                role: entry.role,
            })
            .map_err(PairingError::Enrol)?;
            entry.device = Some(device);
        }
        entry.end(wanted, now);
        Ok(entry.view())
    }

    /// Cancel a pairing. One already finished is returned unchanged.
    pub fn cancel(&mut self, pairing: &str, now: Timestamp) -> Result<Pairing, PairingError> {
        self.settle(now);
        let entry = self.find(pairing).ok_or(PairingError::NotFound)?;
        if entry.is_live() {
            entry.end(PairingState::Cancelled, now);
        }
        Ok(entry.view())
    }

    /// Cancel whatever is open or claimed. Returns it, if there was one.
    pub fn cancel_live(&mut self, now: Timestamp) -> Option<Pairing> {
        self.settle(now);
        let entry = self.entries.back_mut().filter(|entry| entry.is_live())?;
        entry.end(PairingState::Cancelled, now);
        Some(entry.view())
    }

    /// Whether a pairing is waiting to be claimed.
    pub fn is_open(&mut self, now: Timestamp) -> bool {
        self.settle(now);
        self.entries
            .back()
            .is_some_and(|entry| entry.state == PairingState::Open)
    }
}

#[cfg(test)]
mod tests;
