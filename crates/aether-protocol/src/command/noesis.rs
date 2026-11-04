//! What is Noesis about a command: its keyed fingerprints and the two checks
//! the daemon's dispatch makes before routing.

use serde_json::json;

use super::{Command, KeyedRequest};

impl Command {
    /// Whether this is one of the `noesis_*` commands.
    pub fn is_noesis(&self) -> bool {
        matches!(
            self,
            Self::NoesisStatus
                | Self::NoesisConfigure { .. }
                | Self::NoesisCreateSession { .. }
                | Self::NoesisSend { .. }
                | Self::NoesisAnswer { .. }
                | Self::NoesisGetSession { .. }
                | Self::NoesisListSessions { .. }
                | Self::NoesisDeleteSession { .. }
                | Self::NoesisGraph { .. }
                | Self::NoesisDeleteProposition { .. }
                | Self::NoesisDeleteSource { .. }
                | Self::NoesisExtract { .. }
                | Self::NoesisImportSource { .. }
                | Self::NoesisJobs { .. }
                | Self::NoesisCancelJob { .. }
                | Self::NoesisLearner { .. }
                | Self::NoesisLearnerEvidence { .. }
                | Self::NoesisCorrectLearner { .. }
        )
    }

    /// Whether the command needs the running Noesis service. Status and
    /// configure do not: they work while Noesis is disabled.
    pub fn needs_noesis_service(&self) -> bool {
        !matches!(self, Self::NoesisStatus | Self::NoesisConfigure { .. }) && self.is_noesis()
    }

    /// The fingerprint of a keyed Noesis mutation: its tag and every field
    /// that changes what it does, none skipped when absent.
    pub(super) fn noesis_keyed_request(&self) -> Option<serde_json::Result<KeyedRequest<'_>>> {
        let (key, fields) = match self {
            Self::NoesisConfigure {
                patch,
                idempotency_key,
            } => (
                idempotency_key.as_deref()?,
                json!({"cmd": "noesis_configure", "patch": patch}),
            ),
            Self::NoesisCreateSession {
                goal,
                mode,
                context,
                practice_targets,
                idempotency_key,
            } => (
                idempotency_key.as_deref()?,
                json!({"cmd": "noesis_create_session", "goal": goal, "mode": mode,
                    "context": context, "practice_targets": practice_targets}),
            ),
            Self::NoesisSend {
                session,
                text,
                idempotency_key,
            } => (
                idempotency_key.as_deref()?,
                json!({"cmd": "noesis_send", "session": session, "text": text}),
            ),
            Self::NoesisAnswer {
                session,
                question,
                answer,
                idempotency_key,
            } => (
                idempotency_key.as_deref()?,
                json!({"cmd": "noesis_answer", "session": session, "question": question,
                    "answer": answer}),
            ),
            _ => return self.noesis_keyed_deletion(),
        };
        Some(Ok(KeyedRequest { key, fields }))
    }

    fn noesis_keyed_deletion(&self) -> Option<serde_json::Result<KeyedRequest<'_>>> {
        let (key, fields) = match self {
            Self::NoesisDeleteSession {
                session,
                graph,
                idempotency_key,
            } => (
                idempotency_key.as_deref()?,
                json!({"cmd": "noesis_delete_session", "session": session, "graph": graph}),
            ),
            Self::NoesisDeleteProposition {
                proposition,
                idempotency_key,
            } => (
                idempotency_key.as_deref()?,
                json!({"cmd": "noesis_delete_proposition", "proposition": proposition}),
            ),
            Self::NoesisDeleteSource {
                source,
                graph,
                idempotency_key,
            } => (
                idempotency_key.as_deref()?,
                json!({"cmd": "noesis_delete_source", "source": source, "graph": graph}),
            ),
            _ => return self.noesis_keyed_work(),
        };
        Some(Ok(KeyedRequest { key, fields }))
    }

    fn noesis_keyed_work(&self) -> Option<serde_json::Result<KeyedRequest<'_>>> {
        let (key, fields) = match self {
            Self::NoesisExtract {
                session,
                source,
                idempotency_key,
            } => (
                idempotency_key.as_deref()?,
                json!({"cmd": "noesis_extract", "session": session, "source": source}),
            ),
            Self::NoesisImportSource {
                name,
                artifact,
                idempotency_key,
            } => (
                idempotency_key.as_deref()?,
                json!({"cmd": "noesis_import_source", "name": name, "artifact": artifact}),
            ),
            Self::NoesisCancelJob {
                job,
                idempotency_key,
            } => (
                idempotency_key.as_deref()?,
                json!({"cmd": "noesis_cancel_job", "job": job}),
            ),
            Self::NoesisCorrectLearner {
                concept,
                state,
                reason,
                idempotency_key,
            } => (
                idempotency_key.as_deref()?,
                json!({"cmd": "noesis_correct_learner", "concept": concept, "state": state,
                    "reason": reason}),
            ),
            _ => return None,
        };
        Some(Ok(KeyedRequest { key, fields }))
    }
}
