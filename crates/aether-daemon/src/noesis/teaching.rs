//! The teaching and session commands over the running service: F2's loop
//! driven by [`AetherEngine`]. Its events reach the log only through
//! `runtime.apply`, so the runtime's seal sees them; its learner evidence is
//! appended to the learner model and announced as `noesis.learner_updated`.
//! A command answers at once; the turn it starts runs in its own task.
//! Calls hold the service's desk, never its state lock, across a turn.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use aether_core::event::{Actor, EventDraft, EventKind};
use aether_core::idempotency::Submission;
use aether_core::noesis::{ConceptId, NoesisSession, SessionMode};
use aether_noesis::Noesis;
use aether_noesis::teach::records::{Answer, EvidenceDraft, Mode, NoesisEvent, SessionView};
use aether_noesis::teach::{TeachError, Teaching};
use aether_protocol::noesis::{AnswerInput, DeleteSessionGraph, EvidenceBundle};
use aether_protocol::{Command, ErrorReply, Reply};
use aether_runtime::Runtime;
use aether_runtime::noesis_guard::record_noesis_receipt;
use aether_store::Commit;

use super::budget::ServiceBudget;
use super::engine::{AetherEngine, EngineHost, noesis_run};
use super::routes::{Group, group_of};
use super::{Desk, NoesisService, State};
use crate::connection::map_err;

/// Sessions per page when the caller names no limit.
const DEFAULT_PAGE: u32 = 50;
/// The `unavailable` reason of a bundle sent while engineering context is off.
pub(crate) const ENGINEERING_CONTEXT_DISABLED: &str = "engineering_context_disabled";

/// What one teaching call produced for the log and the learner model.
#[derive(Default)]
struct Output {
    events: Vec<NoesisEvent>,
    evidence: Vec<EvidenceDraft>,
}

/// A command's reply, the session whose turn it starts, and whether that
/// session was created with an evidence bundle.
struct Done {
    reply: Result<Reply, TeachError>,
    advance: Option<String>,
    bundled: bool,
}

/// Counts a teaching call from before it waits for the state until it ends,
/// so a drain sees it without taking the lock the call holds.
struct CallGuard(Arc<NoesisService>);

impl CallGuard {
    fn enter(service: &Arc<NoesisService>) -> Self {
        service.teaching_calls.fetch_add(1, Ordering::SeqCst);
        Self(service.clone())
    }
}

impl Drop for CallGuard {
    fn drop(&mut self) {
        let before = self.0.teaching_calls.fetch_sub(1, Ordering::SeqCst);
        debug_assert!(before > 0, "every call that left had entered");
    }
}

/// A running service's teaching resources, taken under the state lock and
/// used after it is released.
struct Ticket {
    desk: Arc<tokio::sync::Mutex<Desk>>,
    budget: Arc<ServiceBudget>,
    cancel: tokio_util::sync::CancellationToken,
}

impl NoesisService {
    /// Answer a validated teaching command; a retry under the same key
    /// replays the first reply.
    pub(super) async fn teaching(
        self: &Arc<Self>,
        runtime: &Arc<Runtime>,
        command: Command,
        by: Actor,
        submission: Option<&Submission>,
    ) -> Result<Reply, ErrorReply> {
        debug_assert_eq!(group_of(&command), Group::Teaching);
        if let Command::NoesisDeleteSession {
            graph: DeleteSessionGraph::DeleteNodes,
            ..
        } = &command
        {
            return Err(ErrorReply::unimplemented(
                "deleting a session's graph nodes (the Noesis graph service)",
            ));
        }
        let _call = CallGuard::enter(self);
        let ticket = self
            .ticket()
            .await
            .ok_or_else(ErrorReply::noesis_disabled)?;
        let mut desk = ticket.desk.lock().await;
        let Some(noesis) = &desk.noesis else {
            return Err(ErrorReply::noesis_disabled());
        };
        refuse_context_while_off(&command, noesis)?;
        if let Some(receipt) = runtime.noesis_receipt(submission).map_err(map_err)? {
            return serde_json::from_value(receipt)
                .map_err(|e| ErrorReply::internal(format!("decode the teaching receipt: {e}")));
        }
        let (done, output) =
            self.with_teaching(&ticket, &mut desk, runtime, |t, _| run_command(t, command))?;
        let reply = done.reply.map_err(reply_error);
        let receipt = reply.as_ref().ok().zip(submission);
        publish(&desk, runtime, output, &by, receipt)?;
        drop(desk);
        if let Some(session) = done.advance {
            if done.bundled {
                self.engineering_after_create(runtime, &session).await;
            }
            self.spawn_advance(runtime, session);
        }
        reply
    }

    /// Queue the new session's engineering extraction when it is automatic;
    /// a failure leaves the session as created and is logged, not returned.
    async fn engineering_after_create(&self, runtime: &Arc<Runtime>, session: &str) {
        debug_assert!(session.starts_with("ns_"), "a created session");
        if let Err(error) = self.after_bundle_stored(runtime, session).await {
            tracing::warn!(%session, %error.message, "queueing the engineering extraction failed");
        }
    }

    /// The running service's teaching resources; `None` once it stops.
    async fn ticket(&self) -> Option<Ticket> {
        let state = self.state.lock().await;
        let State::Running(live) = &*state else {
            return None;
        };
        Some(Ticket {
            desk: live.desk.clone(),
            budget: live.budget.clone(),
            cancel: live.cancel.clone(),
        })
    }

    /// Run `work` on a teaching loop over the open store, keeping its
    /// undelivered inputs for the next call.
    fn with_teaching<T>(
        &self,
        ticket: &Ticket,
        desk: &mut Desk,
        runtime: &Arc<Runtime>,
        work: impl FnOnce(&mut Teaching<'_, AetherEngine<'_>>, &Noesis) -> T,
    ) -> Result<(T, Output), ErrorReply> {
        let run = match desk.run {
            Some(run) => run,
            None => *desk
                .run
                .insert(noesis_run(runtime).map_err(ErrorReply::internal)?),
        };
        let Some(noesis) = &desk.noesis else {
            return Err(ErrorReply::noesis_disabled());
        };
        let host = EngineHost {
            runtime: runtime.clone(),
            models: self.models.clone(),
            budget: ticket.budget.clone(),
            client: self.model_client.clone(),
            cancel: ticket.cancel.clone(),
            prompt_overrides: prompt_overrides(noesis),
            run,
        };
        let mut teaching = Teaching::new(noesis, AetherEngine::new(&host));
        teaching.restore_undelivered(std::mem::take(&mut desk.undelivered));
        let result = work(&mut teaching, noesis);
        let output = Output {
            events: teaching.take_events(),
            evidence: teaching.take_evidence(),
        };
        desk.undelivered = teaching.take_undelivered();
        Ok((result, output))
    }

    /// Run the session's turn in its own task; it waits for the desk.
    fn spawn_advance(self: &Arc<Self>, runtime: &Arc<Runtime>, session: String) {
        let call = CallGuard::enter(self);
        let service = self.clone();
        let runtime = runtime.clone();
        tokio::spawn(async move {
            let _call = call;
            if let Err(error) = service.advance(&runtime, &session).await {
                tracing::warn!(%session, %error.message, "a Noesis teaching turn failed");
            }
        });
    }

    /// Run the session's turn, unless the service stopped or is draining:
    /// no new turn starts after a disable.
    async fn advance(&self, runtime: &Arc<Runtime>, session: &str) -> Result<(), ErrorReply> {
        let Some(ticket) = self.ticket().await else {
            return Ok(());
        };
        let mut desk = ticket.desk.lock().await;
        if desk.noesis.is_none() {
            return Ok(());
        }
        let (ran, output) =
            self.with_teaching(&ticket, &mut desk, runtime, |t, _| t.advance(session))?;
        publish(&desk, runtime, output, &Actor::System, None)?;
        // The hook takes the desk itself; holding it here would deadlock
        // against a stop, which holds the state lock while it waits for the desk.
        drop(desk);
        // Whatever prose the turn committed, even one that failed later.
        let queued = self.after_teaching_turn(runtime, session).await;
        ran.map_err(reply_error)?;
        queued.map(|_| ())
    }

    /// Start the turn of every session owed an answer, as after a restart.
    /// Only the turns count as calls in flight; the short scan does not, so
    /// a settings change right after an enable is not refused for it.
    pub(crate) fn resume_teaching(self: &Arc<Self>, runtime: &Arc<Runtime>) {
        let service = self.clone();
        let runtime = runtime.clone();
        tokio::spawn(async move {
            if let Err(error) = service.resume_owed(&runtime).await {
                tracing::error!(%error.message, "resuming Noesis sessions owed an answer failed");
            }
        });
    }

    async fn resume_owed(self: &Arc<Self>, runtime: &Arc<Runtime>) -> Result<(), ErrorReply> {
        let Some(ticket) = self.ticket().await else {
            return Ok(());
        };
        let sessions = {
            let mut desk = ticket.desk.lock().await;
            if desk.noesis.is_none() {
                return Ok(());
            }
            let (owed, _) = self.with_teaching(&ticket, &mut desk, runtime, |t, _| {
                t.sessions_owing_answers()
            })?;
            owed.map_err(reply_error)?
        };
        for session in sessions {
            self.spawn_advance(runtime, session);
        }
        Ok(())
    }
}

/// Append the evidence and record the events (and a receipt) in one
/// commit. Events are recorded even when the evidence append failed.
fn publish(
    desk: &Desk,
    runtime: &Runtime,
    output: Output,
    by: &Actor,
    receipt: Option<(&Reply, &Submission)>,
) -> Result<(), ErrorReply> {
    let Some(noesis) = &desk.noesis else {
        return Err(ErrorReply::internal("publishing after the service stopped"));
    };
    let appended = append_evidence(noesis, &output.evidence);
    let mut commit = Commit::new();
    for event in &output.events {
        commit.push_event(EventDraft::new(by.clone(), event_kind(event)?));
    }
    if let Ok(concepts) = &appended
        && !concepts.is_empty()
    {
        let concepts = concepts.clone();
        commit.push_event(EventDraft::new(
            by.clone(),
            EventKind::NoesisLearnerUpdated { concepts },
        ));
    }
    if let Some((reply, submission)) = receipt {
        record_noesis_receipt(&mut commit, Some(submission), reply).map_err(map_err)?;
    }
    if !commit.is_empty() {
        runtime.apply(commit).map_err(map_err)?;
    }
    appended.map(|_| ())
}

/// Answer one teaching command on `t`.
fn run_command(t: &mut Teaching<'_, AetherEngine<'_>>, command: Command) -> Done {
    let replied = |reply: Result<Reply, TeachError>| Done {
        reply,
        advance: None,
        bundled: false,
    };
    match command {
        Command::NoesisCreateSession {
            goal,
            mode,
            context,
            practice_targets,
            ..
        } => create_session(
            t,
            &goal,
            mode,
            context.as_ref(),
            &practice_targets.unwrap_or_default(),
        ),
        Command::NoesisSend { session, text, .. } => {
            // A refused message adds nothing for the agent to answer.
            let sent = t.send(&session, &text);
            Done {
                advance: sent.is_ok().then_some(session),
                bundled: false,
                reply: sent.map(|()| Reply::Ok),
            }
        }
        Command::NoesisAnswer {
            session,
            question,
            answer,
            ..
        } => {
            let answered = t.answer(&session, &question, &answer_of(answer));
            // Recorded unless refused before the commit; a failed delivery
            // stays owed and the turn delivers it first.
            let recorded = answered.is_ok() || matches!(answered, Err(TeachError::Engine(_)));
            Done {
                reply: answered.map(|_| Reply::Ok),
                advance: recorded.then_some(session),
                bundled: false,
            }
        }
        Command::NoesisGetSession { session } => replied(
            t.get_session(&session)
                .and_then(|view| session_reply(&view)),
        ),
        Command::NoesisListSessions { after, limit } => {
            let limit = usize::try_from(limit.unwrap_or(DEFAULT_PAGE)).unwrap_or(usize::MAX);
            replied(list_sessions(t, after.as_deref(), limit))
        }
        Command::NoesisDeleteSession { session, .. } => replied(delete_session(t, &session)),
        other => replied(Err(TeachError::Invalid(format!(
            "`{}` is not a teaching command",
            other.name()
        )))),
    }
}

fn create_session(
    t: &mut Teaching<'_, AetherEngine<'_>>,
    goal: &str,
    mode: SessionMode,
    context: Option<&EvidenceBundle>,
    targets: &[String],
) -> Done {
    let mode = match mode {
        SessionMode::Teach => Mode::Teach,
        SessionMode::Practice => Mode::Practice,
    };
    match t.create_session_from(goal, mode, targets, context) {
        Ok(view) => Done {
            reply: session_reply(&view),
            advance: Some(view.id),
            bundled: context.is_some(),
        },
        Err(error) => Done {
            reply: Err(error),
            advance: None,
            bundled: false,
        },
    }
}

/// A bundle is engineering context: refused, before anything is stored or
/// sent, unless `noesis.engineering_context` is in effect.
fn refuse_context_while_off(command: &Command, noesis: &Noesis) -> Result<(), ErrorReply> {
    let Command::NoesisCreateSession {
        context: Some(_), ..
    } = command
    else {
        return Ok(());
    };
    if aether_noesis::evidence::in_effect(&noesis.effective()) {
        return Ok(());
    }
    debug_assert!(noesis.settings().enabled, "a running service is enabled");
    Err(ErrorReply {
        reason: Some(ENGINEERING_CONTEXT_DISABLED.into()),
        ..ErrorReply::new(
            aether_protocol::ErrorCode::Unavailable,
            "engineering context is off; turn on noesis.engineering_context to learn from it",
        )
    })
}

fn list_sessions(
    t: &Teaching<'_, AetherEngine<'_>>,
    after: Option<&str>,
    limit: usize,
) -> Result<Reply, TeachError> {
    let page = t.list_sessions(after, limit)?;
    let sessions = page
        .sessions
        .iter()
        .map(session_wire)
        .collect::<Result<Vec<_>, _>>()?;
    debug_assert!(sessions.len() <= limit);
    Ok(Reply::NoesisSessions {
        sessions,
        next_after: page.next_after,
    })
}

/// The session's Noesis rows and its evidence bundle (N3, by the schema's
/// cascade) go; its agent and the agent's Aether transcript stay, kept and
/// expired by Aether's retention.
fn delete_session(
    t: &mut Teaching<'_, AetherEngine<'_>>,
    session: &str,
) -> Result<Reply, TeachError> {
    let agent = t.delete_session(session)?;
    debug_assert!(!agent.is_empty(), "a session names its agent");
    Ok(Reply::Ok)
}

fn session_reply(view: &SessionView) -> Result<Reply, TeachError> {
    session_wire(view).map(Reply::NoesisSession)
}

/// F2's session view in its frozen wire shape, which it serializes to.
fn session_wire(view: &SessionView) -> Result<NoesisSession, TeachError> {
    let value = serde_json::to_value(view).map_err(|e| TeachError::Corrupt(e.to_string()))?;
    serde_json::from_value(value)
        .map_err(|e| TeachError::Corrupt(format!("session wire shape: {e}")))
}

/// F2's event as the log's typed event, by its frozen name and payload.
fn event_kind(event: &NoesisEvent) -> Result<EventKind, ErrorReply> {
    let mut value = serde_json::to_value(event).map_err(ErrorReply::internal)?;
    let Some(fields) = value.as_object_mut() else {
        return Err(ErrorReply::internal("a Noesis event payload is an object"));
    };
    fields.insert("type".into(), event.name().into());
    serde_json::from_value(value)
        .map_err(|e| ErrorReply::internal(format!("`{}` wire shape: {e}", event.name())))
}

fn answer_of(input: AnswerInput) -> Answer {
    Answer {
        choices: input.choices.unwrap_or_default(),
        text: input.text,
        dont_know: input.dont_know,
        confidence: input.confidence,
        thinking: input.thinking,
    }
}

/// Append the drafts to the learner model; the concepts they touched.
fn append_evidence(
    noesis: &Noesis,
    drafts: &[EvidenceDraft],
) -> Result<Vec<ConceptId>, ErrorReply> {
    if drafts.is_empty() {
        return Ok(Vec::new());
    }
    let conn = noesis.store().connection();
    let now = chrono::Utc::now();
    let ids = aether_noesis::learner::append_drafts(conn, drafts, now)
        .map_err(|e| ErrorReply::internal(format!("append learner evidence: {e}")))?;
    let mut concepts = Vec::with_capacity(ids.len());
    for id in &ids {
        let id = i64::try_from(*id).map_err(ErrorReply::internal)?;
        let concept: String = conn
            .query_row("SELECT concept FROM evidence WHERE id = ?1", [id], |row| {
                row.get(0)
            })
            .map_err(|e| ErrorReply::internal(format!("read appended evidence: {e}")))?;
        concepts.push(concept);
    }
    concepts.sort();
    concepts.dedup();
    debug_assert!(ids.is_empty() || !concepts.is_empty());
    Ok(concepts)
}

/// `<data dir>/noesis/prompts`, read only with the overrides switch on.
fn prompt_overrides(noesis: &Noesis) -> Option<PathBuf> {
    if !noesis.settings().prompt_overrides {
        return None;
    }
    let dir = noesis.store().path().parent()?;
    Some(dir.join("noesis").join("prompts"))
}

fn reply_error(error: TeachError) -> ErrorReply {
    match error {
        TeachError::Invalid(why) => ErrorReply::invalid(why),
        TeachError::NotFound { what, id } => ErrorReply::not_found(format!("{what} `{id}`")),
        TeachError::Conflict(why) => ErrorReply::conflict(why),
        TeachError::Model(refusal) => ErrorReply::invalid(refusal.to_string()),
        error @ (TeachError::Budget(_) | TeachError::Engine(_)) => {
            ErrorReply::new(aether_protocol::ErrorCode::Unavailable, error.to_string())
        }
        other => ErrorReply::internal(other),
    }
}
