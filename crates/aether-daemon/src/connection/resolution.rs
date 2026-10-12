//! `resolve_submission`: what became of a keyed command whose reply was lost.
//! Reads only: a held receipt is replayed, an absent one judged by the ledger.

use aether_core::event::Actor;
use aether_core::idempotency::{Submission, SubmissionHorizon};
use aether_protocol::{Command, ErrorReply, Reply, SubmissionResolution as Resolved};
use aether_runtime::submission::{SubmissionResolution, SubmissionStanding};

use super::dispatch::{blocking, dispatch_authorized};
use super::map_err;
use crate::Daemon;
use crate::authz::AuthContext;

/// The first protocol version whose clients keep a horizon.
const RESOLVING_SINCE: u32 = 18;

pub(super) async fn resolve(
    daemon: &Daemon,
    command: Command,
    horizon: Option<String>,
    by: Actor,
    auth: &AuthContext,
) -> Result<Reply, ErrorReply> {
    let horizon = horizon
        .as_deref()
        .map(str::parse::<SubmissionHorizon>)
        .transpose()
        .map_err(|error| ErrorReply::invalid(error.to_string()))?;
    // Exactly the checks sending it would pass: a resolution reveals nothing
    // about a command its caller could not have sent.
    auth.authorize(&command)?;
    crate::authz::guard(daemon, auth, &command)?;
    let Some(submission) = auth.submission(&command)? else {
        return Err(ErrorReply::invalid(format!(
            "`{}` carries no idempotency_key, so there is no submission to resolve",
            command.name()
        )));
    };
    let found = standing(daemon, &submission, horizon.clone()).await?;
    if found.standing != SubmissionStanding::Recorded {
        return absent(found);
    }
    let replay = Some(submission.clone().replay_only());
    match dispatch_authorized(daemon, command, by, auth, replay).await {
        Ok(reply) => Ok(Reply::SubmissionResolved(Resolved::Executed {
            reply: Box::new(reply),
            horizon: found.horizon.to_string(),
        })),
        // The receipt may have been evicted since the lookup. If it was not,
        // this is the refusal a duplicate would get too.
        Err(refusal) => {
            let again = standing(daemon, &submission, horizon).await?;
            if again.standing == SubmissionStanding::Recorded {
                return Err(refusal);
            }
            absent(again)
        }
    }
}

/// The horizon a client is greeted with, from protocol 18 on. A store that
/// cannot say greets without one, and the client resolves nothing against it.
pub(super) fn greeting_horizon(daemon: &Daemon, speaking: u32) -> Option<String> {
    if speaking < RESOLVING_SINCE {
        return None;
    }
    match daemon.runtime.submission_horizon() {
        Ok(horizon) => Some(horizon.to_string()),
        Err(error) => {
            tracing::warn!(%error, "greeting without a submission horizon");
            None
        }
    }
}

async fn standing(
    daemon: &Daemon,
    submission: &Submission,
    horizon: Option<SubmissionHorizon>,
) -> Result<SubmissionResolution, ErrorReply> {
    debug_assert!(
        !submission.is_replay_only(),
        "the lookup itself replays nothing"
    );
    let (rt, submission) = (daemon.runtime.clone(), submission.clone());
    blocking(move || rt.resolve_submission(&submission, horizon.as_ref()))
        .await?
        .map_err(map_err)
}

fn absent(found: SubmissionResolution) -> Result<Reply, ErrorReply> {
    let horizon = found.horizon.to_string();
    let retained_since = found.retained_since;
    let resolved = match found.standing {
        SubmissionStanding::NeverExecuted => Resolved::AbsentSafe {
            horizon,
            retained_since,
        },
        SubmissionStanding::Unproven(reason) => Resolved::Unknown {
            reason,
            horizon,
            retained_since,
        },
        SubmissionStanding::Recorded => {
            return Err(ErrorReply::internal(
                "a held receipt was reported as absent",
            ));
        }
    };
    Ok(Reply::SubmissionResolved(resolved))
}
