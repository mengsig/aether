//! The learner commands over the running service's `noesis.db`: concept
//! state and evidence reads, and the operator's correction.

use std::sync::Arc;

use aether_core::event::{Actor, EventDraft, EventKind};
use aether_core::idempotency::Submission;
use aether_noesis::learner::{self, LearnerError, Selection};
use aether_noesis::store::NoesisStore;
use aether_protocol::noesis::{ConceptId, ConceptState, ConceptStateKind};
use aether_protocol::{Command, ErrorReply, Reply};
use aether_runtime::Runtime;
use aether_runtime::noesis_guard::record_noesis_receipt;
use aether_store::Commit;

use super::{NoesisService, State};
use crate::connection::map_err;

impl NoesisService {
    /// Answer a validated learner command. Reads derive each state afresh, so
    /// decay is current; a correction appends evidence and announces it.
    pub(super) async fn learner(
        &self,
        runtime: &Arc<Runtime>,
        command: Command,
        by: Actor,
        submission: Option<&Submission>,
    ) -> Result<Reply, ErrorReply> {
        debug_assert_eq!(
            super::routes::group_of(&command),
            super::routes::Group::Learner,
            "only learner commands reach here"
        );
        let desk = {
            let state = self.state.lock().await;
            let State::Running(live) = &*state else {
                return Err(ErrorReply::noesis_disabled());
            };
            live.desk.clone()
        };
        let desk = if matches!(&command, Command::NoesisCorrectLearner { .. }) {
            super::writable_desk(&desk).await
        } else {
            desk.lock().await
        };
        let Some(noesis) = &desk.noesis else {
            return Err(ErrorReply::noesis_disabled());
        };
        let store = noesis.store();
        let now = chrono::Utc::now();
        match command {
            Command::NoesisCorrectLearner {
                concept,
                state,
                reason,
                ..
            } => {
                let correction = Correction {
                    concept,
                    state,
                    reason,
                    by,
                };
                correct(runtime, store, correction, submission, now).map(Reply::ConceptState)
            }
            read => answer_read(store, read, now),
        }
    }
}

/// `noesis_learner` and `noesis_learner_evidence`.
fn answer_read(
    store: &NoesisStore,
    command: Command,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Reply, ErrorReply> {
    let conn = store.connection();
    match command {
        Command::NoesisLearner {
            concepts,
            query,
            limit,
        } => {
            let selection = &Selection {
                concepts,
                query,
                limit,
            };
            let concepts = learner::states(conn, selection, now).map_err(reply_error)?;
            Ok(Reply::NoesisLearner { concepts })
        }
        Command::NoesisLearnerEvidence {
            concept,
            before,
            limit,
        } => {
            let page =
                learner::evidence_page(conn, &concept, before, limit).map_err(reply_error)?;
            Ok(Reply::NoesisEvidence {
                evidence: page.evidence,
                next_before: page.next_before,
            })
        }
        other => Err(ErrorReply::internal(format!(
            "`{}` is not a learner command",
            other.name()
        ))),
    }
}

struct Correction {
    concept: ConceptId,
    state: ConceptStateKind,
    reason: String,
    by: Actor,
}

/// Append the correction and record `noesis.learner_updated` with its
/// receipt; a retry under the same key replays the first state.
fn correct(
    runtime: &Arc<Runtime>,
    store: &NoesisStore,
    correction: Correction,
    submission: Option<&Submission>,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<ConceptState, ErrorReply> {
    if let Some(receipt) = runtime.noesis_receipt(submission).map_err(map_err)? {
        return serde_json::from_value(receipt)
            .map_err(|e| ErrorReply::internal(format!("decode the correction receipt: {e}")));
    }
    let corrected = learner::correct(
        store.connection(),
        &correction.concept,
        correction.state,
        &correction.reason,
        now,
    )
    .map_err(reply_error)?;
    debug_assert_eq!(corrected.state, correction.state, "a correction wins");
    let mut commit = Commit::new();
    commit.push_event(EventDraft::new(
        correction.by,
        EventKind::NoesisLearnerUpdated {
            concepts: vec![correction.concept],
        },
    ));
    record_noesis_receipt(&mut commit, submission, &corrected).map_err(map_err)?;
    runtime.apply(commit).map_err(map_err)?;
    Ok(corrected)
}

fn reply_error(error: LearnerError) -> ErrorReply {
    match error {
        LearnerError::UnknownConcept(id) => ErrorReply::not_found(format!("concept `{id}`")),
        LearnerError::Invalid(why) => ErrorReply::invalid(why),
        other => ErrorReply::internal(other),
    }
}
