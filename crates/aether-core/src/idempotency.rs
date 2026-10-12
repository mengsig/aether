//! Idempotency keys: what makes two tool calls the same effect, and what
//! that means for retrying them.
//!
//! A key is declared by the tool or derived from the call. Classes that can
//! compound an effect record their outcome under the key, scoped to the
//! invocation that produced the call: re-driving that invocation answers from
//! the record instead of repeating the effect, while a later invocation - a
//! live agent asking again - runs the call.
//!
//! [`Submission`] is the other scope: a key a *client* puts on a command that
//! creates work, so a request it sent twice - because it lost the answer to
//! the first - creates the work once. Same word, different record: a tool
//! outcome answers one re-driven attempt, a submission answers a retry of one
//! external request.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::id::{AgentId, InboxItemId, InvocationId, MessageId, ScheduleId, TaskId, ToolCallId};
use crate::inbox::MessageDelivery;
use crate::time::Timestamp;
use crate::tool::{SideEffectClass, ToolOutcome};

mod ledger;
pub use ledger::{InvalidHorizon, SubmissionHorizon, SubmissionLedger, UnprovenAbsence};

/// How a class of side effect is retried, and whether its outcome is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RetryPolicy {
    /// Attempts including the first. 1 means a failure is final.
    pub attempts: u32,
    /// Keep the outcome under the call's key so a repeat replays it.
    pub record_key: bool,
}

impl SideEffectClass {
    /// Retrying a read changes nothing, so it is retried freely and never
    /// recorded. Anything that can compound is recorded; only the class that
    /// cannot be repeated safely is refused a second attempt.
    pub fn retry_policy(self) -> RetryPolicy {
        match self {
            Self::ReadOnly => RetryPolicy {
                attempts: 3,
                record_key: false,
            },
            Self::IdempotentMutation => RetryPolicy {
                attempts: 3,
                record_key: true,
            },
            Self::Mutation => RetryPolicy {
                attempts: 1,
                record_key: true,
            },
            Self::External => RetryPolicy {
                attempts: 2,
                record_key: true,
            },
        }
    }
}

/// What makes two calls the same effect.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(transparent)]
pub struct IdempotencyKey(pub String);

impl fmt::Display for IdempotencyKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl IdempotencyKey {
    /// A key a tool declared for itself, namespaced by tool so two tools
    /// cannot collide on the same word. The `d:` marks it declared, so a
    /// tool naming a hex-shaped key cannot collide with a derived one.
    pub fn declared(tool: &str, key: &str) -> Self {
        Self(format!("{tool}:d:{key}"))
    }

    /// The key for a call the tool did not name: a digest over the tool and
    /// its input, canonicalized so key order in the JSON cannot change it.
    pub fn derive(tool: &str, input: &Value) -> Self {
        let mut hasher = <sha2::Sha256 as sha2::Digest>::new();
        sha2::Digest::update(&mut hasher, tool.as_bytes());
        sha2::Digest::update(&mut hasher, b"\0");
        sha2::Digest::update(&mut hasher, canonical(input).as_bytes());
        let digest = sha2::Digest::finalize(hasher);
        Self(format!("{tool}:h:{}", hex::encode(&digest[..16])))
    }
}

/// JSON with object keys in sorted order, so equal values render equal.
fn canonical(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            let body: Vec<String> = entries
                .into_iter()
                .map(|(k, v)| format!("{}:{}", Value::String(k.clone()), canonical(v)))
                .collect();
            format!("{{{}}}", body.join(","))
        }
        Value::Array(items) => {
            let body: Vec<String> = items.iter().map(canonical).collect();
            format!("[{}]", body.join(","))
        }
        other => other.to_string(),
    }
}

/// A committed outcome, kept so a re-driven invocation can be answered
/// without running the call again.
///
/// Scoped to one agent - two agents doing the same thing are two effects on
/// the world - and to one invocation: the record answers only the attempt it
/// belongs to. A fresh call from a later invocation runs, however identical.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RecordedOutcome {
    pub key: IdempotencyKey,
    pub agent_id: AgentId,
    /// The invocation whose re-drive this record answers.
    pub invocation_id: InvocationId,
    /// The call that actually ran.
    pub call_id: ToolCallId,
    pub tool: String,
    pub at: Timestamp,
    pub outcome: ToolOutcome,
}

/// The wire field a client puts its submission key in.
pub const SUBMISSION_KEY_FIELD: &str = "idempotency_key";

/// Longest submission key accepted. Room for a UUID, a ULID, or a client's
/// own `<source>:<id>` compound, and short enough to name in an error.
pub const MAX_SUBMISSION_KEY_BYTES: usize = 200;

/// How long a submission record answers a retry. Long enough for a client to
/// notice a lost acknowledgement, restart and come back; short enough that the
/// table stays small.
pub const SUBMISSION_RETENTION_DAYS: i64 = 7;

/// Records kept whatever their age, so a client that never stops submitting
/// cannot grow the table without bound. The oldest go first.
///
/// Sized for the window rather than the other way round: measured at 100k
/// records the table is 53 MB with UUID keys and 129 MB at
/// `MAX_SUBMISSION_KEY_BYTES`, and a client would have to submit 100k keyed
/// commands inside seven days to reach it. Evicting a record still inside the
/// window voids the guarantee for it, so the store says so out loud when it
/// happens (`submissions.evicted`).
pub const MAX_SUBMISSIONS: usize = 100_000;

/// How often the size pass runs, counted in recorded submissions since the
/// daemon process started. Finding the overflow costs a scan, and paying it
/// on every write to discover there is none is the expensive way to learn
/// nothing; the table is therefore bounded by `MAX_SUBMISSIONS +
/// SUBMISSION_SIZE_PASS_EVERY`, not by the cap exactly - provided the process
/// lives long enough to make this many keyed writes and run one size pass. A
/// process that restarts more often never runs a size pass, and the table is
/// then bounded only by `SUBMISSION_RETENTION_DAYS`.
pub const SUBMISSION_SIZE_PASS_EVERY: u64 = 512;

/// Which field set the stored `fingerprint` was taken over. A daemon that
/// changes the set bumps this, and a record from another scheme is refused
/// by name rather than compared against a digest that means something else.
pub const FINGERPRINT_SCHEME: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error(
    "idempotency_key must be 1-{MAX_SUBMISSION_KEY_BYTES} bytes of printable ASCII (0x21-0x7E): no spaces, no control or format characters"
)]
pub struct InvalidSubmissionKey;

/// A client's key on a command that creates work, with the digest of the
/// request it arrived on.
///
/// The key alone decides whether two commands are the same submission; the
/// fingerprint only decides whether the client meant them to be. Reusing a key
/// for different work is a client bug and is refused, never answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Submission {
    pub key: IdempotencyKey,
    pub fingerprint: String,
    /// Answer from the record or not at all; see [`Self::replay_only`].
    replay_only: bool,
}

impl Submission {
    /// `request` is the field set that decides whether two submissions are the
    /// same request - named per command by the protocol, never the whole
    /// re-serialized command, so a field added later cannot change the digest
    /// of a record an older daemon wrote. The key field is removed here rather
    /// than trusted absent, so no caller can leak it into the digest.
    ///
    /// ASCII printable only: a key is stored verbatim for the retention window
    /// and echoed back in the conflict error, and a zero-width or bidi code
    /// point there is two visually identical keys and a spoofable error line.
    pub fn new(key: &str, request: &Value) -> Result<Self, InvalidSubmissionKey> {
        if key.is_empty() || key.len() > MAX_SUBMISSION_KEY_BYTES {
            return Err(InvalidSubmissionKey);
        }
        if !key.bytes().all(|b| (0x21..=0x7E).contains(&b)) {
            return Err(InvalidSubmissionKey);
        }
        Ok(Self {
            key: IdempotencyKey(key.to_owned()),
            fingerprint: fingerprint(request),
            replay_only: false,
        })
    }

    /// The same submission, answerable only from its record: it replays what
    /// the key created or is refused, and never does the work.
    pub fn replay_only(mut self) -> Self {
        self.replay_only = true;
        self
    }

    pub fn is_replay_only(&self) -> bool {
        self.replay_only
    }

    /// The record to commit alongside whatever `outcome` names.
    pub fn record(&self, at: Timestamp, outcome: SubmissionOutcome) -> RecordedSubmission {
        assert!(
            !self.replay_only,
            "a replay-only submission of `{}` reached a commit",
            self.key
        );
        RecordedSubmission {
            key: self.key.clone(),
            fingerprint: self.fingerprint.clone(),
            scheme: FINGERPRINT_SCHEME,
            at,
            outcome,
        }
    }
}

/// Digest over a request with its own key removed, object keys sorted, so two
/// frames asking for the same work fingerprint equally however their JSON was
/// ordered.
///
/// Untyped values reach it as the client spelled them: a `trigger` payload or
/// a schedule payload is digested literally, so `1` and `1.0`, or the two
/// unicode normalizations of the same text, are two submissions. A retry must
/// therefore be byte-faithful in those fields; every typed field is normalized
/// by the decode that precedes this.
fn fingerprint(request: &Value) -> String {
    let mut request = request.clone();
    if let Some(fields) = request.as_object_mut() {
        fields.remove(SUBMISSION_KEY_FIELD);
    }
    let mut hasher = <sha2::Sha256 as sha2::Digest>::new();
    sha2::Digest::update(&mut hasher, canonical(&request).as_bytes());
    hex::encode(sha2::Digest::finalize(hasher))
}

/// What a submission created. Identities only: a retry is answered by reading
/// them back, so it sees the work as it is now rather than a stale copy of how
/// it looked when it was created.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "created", rename_all = "snake_case")]
pub enum SubmissionOutcome {
    /// Immutable result committed atomically with a state-changing command.
    Receipt {
        value: Value,
    },
    Message {
        message: MessageId,
    },
    /// One per live agent in the subtree, in delivery order.
    FannedOut {
        deliveries: Vec<MessageDelivery>,
    },
    /// The task created with the agent travels with it: an agent that has
    /// moved on to later work would otherwise answer a retry with a task the
    /// submission never created.
    Agent {
        agent: AgentId,
        task: Option<TaskId>,
    },
    Task {
        task: TaskId,
    },
    /// A trigger answers `Ok`; the item is kept so the record names what it
    /// delivered.
    Trigger {
        item: InboxItemId,
    },
    Schedule {
        schedule: ScheduleId,
    },
}

impl SubmissionOutcome {
    /// The wire tag, for saying what a key was used for when a replay asks
    /// for something else.
    pub fn created(&self) -> &'static str {
        match self {
            Self::Receipt { .. } => "receipt",
            Self::Message { .. } => "message",
            Self::FannedOut { .. } => "fanned_out",
            Self::Agent { .. } => "agent",
            Self::Task { .. } => "task",
            Self::Trigger { .. } => "trigger",
            Self::Schedule { .. } => "schedule",
        }
    }
}

/// A committed submission, kept so a client that lost the answer to its
/// request is given the same one instead of creating the work twice.
///
/// Daemon-wide, and that is a property of the daemon having exactly one trust
/// level (ARCHITECTURE §16) rather than a permanent design fact: there is no
/// client identity to scope a key to, so keys are expected to be globally
/// unique - a UUID, or the client's own namespace prefix - and a key reused
/// for different work is refused by name. Introducing per-client identity
/// would mean re-keying this table on `(actor, key)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RecordedSubmission {
    pub key: IdempotencyKey,
    pub fingerprint: String,
    /// Which field set `fingerprint` covers. A record from another scheme
    /// cannot be compared and is refused rather than replayed; a row written
    /// before schemes existed reads as 0 and is refused the same way.
    #[serde(default)]
    pub scheme: u32,
    pub at: Timestamp,
    pub outcome: SubmissionOutcome,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_derived_key_ignores_json_key_order_but_not_values() {
        let a = IdempotencyKey::derive("bash", &json!({"command": "ls", "cwd": "/r"}));
        let b = IdempotencyKey::derive("bash", &json!({"cwd": "/r", "command": "ls"}));
        assert_eq!(a, b);
        let c = IdempotencyKey::derive("bash", &json!({"command": "rm", "cwd": "/r"}));
        assert_ne!(a, c);
        // Nested objects are canonicalized too.
        assert_eq!(
            IdempotencyKey::derive("t", &json!({"a": {"x": 1, "y": 2}})),
            IdempotencyKey::derive("t", &json!({"a": {"y": 2, "x": 1}}))
        );
    }

    #[test]
    fn keys_are_namespaced_by_tool() {
        let same_input = json!({"path": "a"});
        assert_ne!(
            IdempotencyKey::derive("read_file", &same_input),
            IdempotencyKey::derive("write_file", &same_input)
        );
        assert_eq!(
            IdempotencyKey::declared("bash", "nightly-build").0,
            "bash:d:nightly-build"
        );
        // A declared key shaped like a derived one still cannot collide.
        let derived = IdempotencyKey::derive("bash", &json!({"command": "ls"}));
        let hex = derived.0.rsplit(':').next().expect("hex tail");
        assert_ne!(IdempotencyKey::declared("bash", hex), derived);
    }

    /// The key is stored verbatim for a week and echoed back in the conflict
    /// error, so the charset the message promises is the charset enforced.
    /// `is_control` alone let category-Cf through: zero-width and bidi code
    /// points, i.e. two visually identical keys and a spoofable error line.
    #[test]
    fn a_key_is_printable_ascii_or_it_is_refused() {
        let work = json!({"cmd": "send_message"});
        for good in [
            "a",
            "01H8XGJWBWBAQ4",
            "gh:owner/repo:issue#42",
            "~!@$%^&*()_+",
        ] {
            assert!(Submission::new(good, &work).is_ok(), "{good} is a key");
        }
        for bad in [
            "",
            " ",
            "a b",
            "a\nb",
            "a\u{200b}b",
            "a\u{202e}b",
            "café",
            "\u{7f}",
        ] {
            assert_eq!(
                Submission::new(bad, &work),
                Err(InvalidSubmissionKey),
                "{bad:?} is not a key"
            );
        }
        assert!(Submission::new(&"k".repeat(MAX_SUBMISSION_KEY_BYTES), &work).is_ok());
        assert!(Submission::new(&"k".repeat(MAX_SUBMISSION_KEY_BYTES + 1), &work).is_err());
    }

    /// The key never enters the digest, whoever builds the request value, and
    /// the record stamps the scheme its fingerprint was taken under.
    #[test]
    fn the_digest_covers_the_work_and_the_record_names_its_scheme() {
        let work = json!({"cmd": "create_task", "objective": "ship"});
        let with_key = json!({
            "cmd": "create_task",
            "objective": "ship",
            SUBMISSION_KEY_FIELD: "k-1",
        });
        assert_eq!(
            Submission::new("k-1", &work).expect("key").fingerprint,
            Submission::new("k-2", &with_key).expect("key").fingerprint,
            "neither the key nor its field is part of the work asked for"
        );
        assert_ne!(
            Submission::new("k-1", &work).expect("key").fingerprint,
            Submission::new("k-1", &json!({"cmd": "create_task", "objective": "revert"}))
                .expect("key")
                .fingerprint
        );
        let record = Submission::new("k-1", &work).expect("key").record(
            crate::time::now(),
            SubmissionOutcome::Task {
                task: TaskId::new(),
            },
        );
        assert_eq!(record.scheme, FINGERPRINT_SCHEME);
        // A row written before schemes existed reads as 0, not as this one.
        let legacy: RecordedSubmission = serde_json::from_value(json!({
            "key": "k-1",
            "fingerprint": record.fingerprint,
            "at": record.at,
            "outcome": {"created": "task", "task": TaskId::new()},
        }))
        .expect("de");
        assert_eq!(legacy.scheme, 0);
    }

    #[test]
    fn retry_policy_follows_the_class() {
        // A read changes nothing, so it is repeated freely and never kept.
        assert_eq!(SideEffectClass::ReadOnly.retry_policy().attempts, 3);
        assert!(!SideEffectClass::ReadOnly.retry_policy().record_key);
        // Anything that can compound is kept; only what cannot be repeated
        // safely is refused a second attempt.
        assert_eq!(SideEffectClass::Mutation.retry_policy().attempts, 1);
        assert!(SideEffectClass::Mutation.retry_policy().record_key);
        assert_eq!(
            SideEffectClass::IdempotentMutation.retry_policy().attempts,
            3
        );
        assert!(
            SideEffectClass::IdempotentMutation
                .retry_policy()
                .record_key
        );
        assert_eq!(SideEffectClass::External.retry_policy().attempts, 2);
        assert!(SideEffectClass::External.retry_policy().record_key);
    }
}
