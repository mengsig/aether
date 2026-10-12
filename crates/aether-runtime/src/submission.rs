//! Client-supplied idempotency keys on the commands that create work.
//!
//! A client that loses the answer to a submission cannot tell a request that
//! never landed from one that landed and whose reply was dropped. Retrying
//! blind creates the work twice; not retrying loses it. A key on the command
//! settles it: the key, a fingerprint of the request and what it created are
//! committed in the same transaction as the work, so a retry under the same
//! key is answered with the first submission's identity and creates nothing.
//!
//! Two things make that hold. The key is checked and recorded inside one
//! [`Runtime::hold`], so two retries in flight at once cannot both find
//! nothing; and `submissions.key` is a primary key written with a plain
//! INSERT, so a duplicate that somehow reached the store fails its whole
//! commit rather than landing beside the first.

use aether_core::idempotency::{
    FINGERPRINT_SCHEME, RecordedSubmission, Submission, SubmissionHorizon, SubmissionOutcome,
    UnprovenAbsence,
};
use aether_core::time::Timestamp;

use crate::{CommitHold, Result, Runtime, RuntimeError};

/// Whether a keyed submission was executed, as far as the store can prove.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmissionStanding {
    /// Its receipt is held: it ran, and a retry would be answered from it.
    Recorded,
    /// No receipt, and none recorded since its horizon was ever dropped.
    NeverExecuted,
    /// No receipt, and nothing proves there never was one.
    Unproven(UnprovenAbsence),
}

/// The answer to [`Runtime::resolve_submission`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmissionResolution {
    pub standing: SubmissionStanding,
    /// The store's horizon now, for whatever the client sends next.
    pub horizon: SubmissionHorizon,
    /// Every receipt stamped after this is still held. For display only.
    pub retained_since: Timestamp,
}

impl Runtime {
    pub(crate) fn replay_receipt<T: serde::de::DeserializeOwned>(
        &self,
        hold: &CommitHold<'_>,
        submission: Option<&Submission>,
    ) -> Result<Option<T>> {
        let Some(outcome) = self.replayed_submission(hold, submission)? else {
            return Ok(None);
        };
        let SubmissionOutcome::Receipt { value } = outcome else {
            return Err(wrong_shape(&outcome, "receipt"));
        };
        serde_json::from_value(value)
            .map(Some)
            .map_err(|error| RuntimeError::Internal(format!("decode submission receipt: {error}")))
    }

    pub(crate) fn commit_receipt<T: serde::Serialize>(
        &self,
        hold: CommitHold<'_>,
        mut commit: aether_store::Commit,
        submission: Option<&Submission>,
        receipt: &T,
    ) -> Result<()> {
        record_receipt(&mut commit, submission, receipt)?;
        self.apply_held(hold, commit)?;
        Ok(())
    }
    /// What `submission` already created, if this store has seen its key.
    /// `None` means the caller must do the work and commit the record with it.
    ///
    /// `hold` is the proof the commit lock is held: the check and the record
    /// it decides on must be one critical section, or two retries in flight
    /// at once both find nothing and both do the work.
    pub(crate) fn replayed_submission(
        &self,
        _hold: &CommitHold<'_>,
        submission: Option<&Submission>,
    ) -> Result<Option<SubmissionOutcome>> {
        self.recorded_submission(submission)
    }

    /// The commit lock, taken now, but only for a command that carries a key.
    ///
    /// A keyed command reads its key and writes its record inside one critical
    /// section, so it holds the lock across the reads in between. An unkeyed
    /// one has nothing to check and takes the lock where it always did: the
    /// guarantee is paid for by the callers that ask for it, not by every
    /// writer in the daemon.
    pub(crate) fn hold_for(&self, submission: Option<&Submission>) -> Option<CommitHold<'_>> {
        submission.map(|_| self.hold())
    }

    /// The same lookup without the hold, for a command with expensive work to
    /// do before it can take one. Sound as a fast path and useless as a
    /// guarantee: a record is durable and never rewritten, so finding one is
    /// final - but a submission committing right now is not visible yet, which
    /// is what the held check exists for.
    pub(crate) fn recorded_submission(
        &self,
        submission: Option<&Submission>,
    ) -> Result<Option<SubmissionOutcome>> {
        let Some(submission) = submission else {
            return Ok(None);
        };
        let outcome = self.held_receipt(submission)?;
        // Every keyed command asks here before it does any work, so this is
        // where a submission that may only be replayed is stopped.
        if outcome.is_none() && submission.is_replay_only() {
            return Err(receipt_gone(submission));
        }
        Ok(outcome)
    }

    fn held_receipt(&self, submission: &Submission) -> Result<Option<SubmissionOutcome>> {
        let Some(record) = self.store.submission(&submission.key)? else {
            return Ok(None);
        };
        // Before the fingerprint compare, not after: two digests taken over
        // different field sets are not comparable, and reporting a mismatch
        // between them as "a different request" would blame the client for
        // this daemon's upgrade.
        if record.scheme != FINGERPRINT_SCHEME {
            return Err(stale_scheme(&record));
        }
        if record.fingerprint != submission.fingerprint {
            return Err(conflict(&record));
        }
        Ok(Some(record.outcome))
    }

    /// Where a keyed submission stands; executes and writes nothing. Lock-free
    /// on purpose: one committing now reads as absent and its retry replays.
    pub fn resolve_submission(
        &self,
        submission: &Submission,
        horizon: Option<&SubmissionHorizon>,
    ) -> Result<SubmissionResolution> {
        // The receipt first and the ledger second: a ledger read after the
        // lookup covers every drop the lookup could have missed.
        let held = self.held_receipt(submission)?.is_some();
        let ledger = self.store.submission_ledger()?;
        let standing = match (held, ledger.proves_absence(horizon)) {
            (true, _) => SubmissionStanding::Recorded,
            (false, Ok(())) => SubmissionStanding::NeverExecuted,
            (false, Err(unproven)) => SubmissionStanding::Unproven(unproven),
        };
        Ok(SubmissionResolution {
            standing,
            horizon: ledger.horizon(),
            retained_since: ledger.retained_since(),
        })
    }

    /// What a client is told at the handshake and keeps with each key it
    /// sends, so a lost acknowledgement can later be resolved.
    pub fn submission_horizon(&self) -> Result<SubmissionHorizon> {
        Ok(self.store.submission_ledger()?.horizon())
    }
}

pub(crate) fn record_receipt<T: serde::Serialize>(
    commit: &mut aether_store::Commit,
    submission: Option<&Submission>,
    receipt: &T,
) -> Result<()> {
    let Some(submission) = submission else {
        return Ok(());
    };
    let value = serde_json::to_value(receipt)
        .map_err(|error| RuntimeError::Internal(format!("encode submission receipt: {error}")))?;
    debug_assert!(!submission.key.0.is_empty());
    debug_assert_eq!(submission.fingerprint.len(), 64);
    commit.push_mutation(aether_store::Mutation::RecordSubmission(submission.record(
        aether_core::time::now(),
        SubmissionOutcome::Receipt { value },
    )));
    Ok(())
}

/// A replay-only submission found no receipt to replay: it was evicted after
/// the resolution that saw it, and the caller resolves again.
fn receipt_gone(submission: &Submission) -> RuntimeError {
    RuntimeError::NotFound(format!(
        "the receipt held for idempotency_key `{}`",
        submission.key
    ))
}

/// A record whose fingerprint was taken over a different field set - written
/// by a daemon before or after this one's fingerprint scheme. Neither replayed
/// (the requests may differ) nor reported as a different request (they may
/// not), so it says what actually happened and what to do.
fn stale_scheme(record: &RecordedSubmission) -> RuntimeError {
    RuntimeError::Invalid(format!(
        "idempotency_key `{}` was recorded on {} by a daemon using request-fingerprint scheme {}, \
         and this daemon uses {}; it cannot tell a retry from a new request under that key - \
         submit under a new one",
        record.key,
        record.at.to_rfc3339(),
        record.scheme,
        FINGERPRINT_SCHEME
    ))
}

/// What a recorded submission created is gone from the store. Not `Internal`:
/// nothing purges agents, tasks or schedules today, so this can only follow an
/// operator or a future sweep removing the work while its record stands, and
/// the client's next move is a new key rather than a retry.
pub(crate) fn created_work_gone(what: &str, id: &dyn std::fmt::Display) -> RuntimeError {
    RuntimeError::NotFound(format!(
        "{what} {id} was created under this idempotency_key and is no longer in the store; the \
         retry cannot be answered - submit again under a new key"
    ))
}

/// A key reused for different work. Actionable rather than merely refused: it
/// names the key and what it was spent on, which is what a client needs to
/// tell a genuine collision from its own retry of a request it edited.
fn conflict(record: &RecordedSubmission) -> RuntimeError {
    RuntimeError::Invalid(format!(
        "idempotency_key `{}` already created a {} on {}; a different request needs a new key",
        record.key,
        record.outcome.created(),
        record.at.to_rfc3339()
    ))
}

/// A recorded outcome that is not the shape the replaying command expects.
///
/// Unreachable through the socket - the fingerprint covers the command tag, so
/// a key reused across two commands conflicts before it gets here - and
/// reported rather than unwrapped, because the alternative is answering one
/// command with another's identity.
pub(crate) fn wrong_shape(outcome: &SubmissionOutcome, wanted: &str) -> RuntimeError {
    RuntimeError::Internal(format!(
        "idempotency record created a {}, not a {wanted}",
        outcome.created()
    ))
}
