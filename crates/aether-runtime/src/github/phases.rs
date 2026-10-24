//! The two halves of a repository pass, issue/comment polling and the review
//! sweep, keep their own backoff so a failure in one never delays the other.
use super::{RepositoryState, source::Error};
use aether_core::time::{Timestamp, now};
use serde::{Deserialize, Serialize};

/// The review sweep's backoff and resumable position.
#[derive(Default, Serialize, Deserialize)]
pub(super) struct ReviewState {
    pub failures: u32,
    pub next_poll_at: Option<Timestamp>,
    pub error: Option<String>,
    /// The cycle under way; `None` discovers a new one on the next pass.
    pub cycle: Option<ReviewCycle>,
}

/// One review sweep over a fixed set of pull requests, spread over passes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct ReviewCycle {
    /// When the cycle discovered its pull requests: the review cursor once
    /// every one of them is swept, so later activity falls in the next cycle.
    pub started_at: Timestamp,
    /// Pull requests not yet swept, strictly ascending: oldest first.
    pub pending: Vec<u64>,
}

/// Whether a phase whose next poll is `next` may run at `at`.
pub(super) fn due(next: Option<Timestamp>, at: Timestamp) -> bool {
    next.is_none_or(|next| next <= at)
}

/// Seconds GitHub asked every request to wait, from a phase's outcome.
pub(super) fn retry_after<T>(outcome: &Result<T, Error>) -> u64 {
    outcome.as_ref().err().map_or(0, |error| error.retry_after)
}

/// Seconds until a phase is due again after `failures` consecutive failures:
/// doubling up to an hour, never below the interval or GitHub's `Retry-After`.
fn delay(interval: u64, failures: u32, retry_after: u64) -> u64 {
    debug_assert!(interval > 0, "the poll interval is validated as positive");
    if failures == 0 {
        return interval;
    }
    interval
        .saturating_mul(1u64 << failures.min(5))
        .min(3600)
        .max(interval)
        .max(retry_after)
}

/// Count one outcome into a phase's failures and error; returns when it is next due.
fn settle(
    failures: &mut u32,
    error: &mut Option<String>,
    outcome: Option<&Error>,
    interval: u64,
) -> Timestamp {
    match outcome {
        None => {
            *failures = 0;
            *error = None;
        }
        Some(failed) => {
            *failures = failures.saturating_add(1);
            *error = Some(failed.message.clone());
        }
    }
    debug_assert_eq!(error.is_some(), *failures > 0, "an error is a failure");
    let retry_after = outcome.map_or(0, |failed| failed.retry_after);
    now() + chrono::Duration::seconds(delay(interval, *failures, retry_after) as i64)
}

impl RepositoryState {
    pub(super) fn record_comments(&mut self, outcome: Result<(), Error>, interval: u64) {
        let next = settle(
            &mut self.failures,
            &mut self.error,
            outcome.err().as_ref(),
            interval,
        );
        self.next_poll_at = Some(next);
    }

    /// A swept cycle with nothing pending ends: its start becomes the review
    /// cursor, exactly once per cycle. A failure keeps the stored position.
    pub(super) fn record_reviews(&mut self, outcome: Result<ReviewCycle, Error>, interval: u64) {
        let review = &mut self.review;
        let next = settle(
            &mut review.failures,
            &mut review.error,
            outcome.as_ref().err(),
            interval,
        );
        review.next_poll_at = Some(next);
        let Ok(cycle) = outcome else {
            return;
        };
        debug_assert!(
            cycle.pending.windows(2).all(|w| w[0] < w[1]),
            "pending pull requests stay strictly ascending"
        );
        debug_assert!(
            cycle.started_at >= self.review_cursor,
            "a cycle never moves the review cursor back"
        );
        if cycle.pending.is_empty() {
            self.review_cursor = cycle.started_at;
            review.cycle = None;
        } else {
            review.cycle = Some(cycle);
        }
    }

    /// Fold both phases into the status a client reads.
    pub(super) fn refresh_status(&mut self, at: Timestamp) {
        let status = &mut self.status;
        status.last_poll_at = Some(at);
        status.next_poll_at = [self.next_poll_at, self.review.next_poll_at]
            .into_iter()
            .map(|next| next.unwrap_or(at))
            .min();
        status.last_error = match (&self.error, &self.review.error) {
            (None, None) => None,
            (Some(error), None) => Some(error.clone()),
            (None, Some(review)) => Some(format!("review sweep: {review}")),
            (Some(error), Some(review)) => Some(format!("{error}; review sweep: {review}")),
        };
        if status.last_error.is_none() {
            status.last_success_at = Some(at);
        }
    }

    /// Forget backoff and any cycle under way, as on (re)activation.
    pub(super) fn restart_phases(&mut self, at: Timestamp) {
        self.failures = 0;
        self.next_poll_at = None;
        self.error = None;
        self.review = ReviewState::default();
        self.status.next_poll_at = Some(at);
        self.status.last_error = None;
    }
}
