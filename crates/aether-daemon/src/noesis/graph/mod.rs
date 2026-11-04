//! The knowledge-graph commands over the running service's `noesis.db`: the
//! snapshot, deletions, extraction and import jobs, and their cancel.

use std::sync::Arc;

use aether_core::event::{Actor, EventDraft, EventKind};
use aether_core::id::ArtifactId;
use aether_core::idempotency::Submission;
use aether_core::noesis::{Job, JobKind, JobState};
use aether_noesis::extract::{self, ExtractError, jobs as job_rows};
use aether_noesis::graph::ids;
use aether_noesis::graph::store::{self as graph_store, GraphError, SnapshotFilter, SourceRow};
use aether_noesis::store::NoesisStore;
use aether_protocol::noesis::DeleteSourceGraph;
use aether_protocol::{Command, ErrorReply, Reply};
use aether_runtime::Runtime;
use aether_runtime::noesis_guard::record_noesis_receipt;
use aether_store::Commit;

use super::{Live, NoesisService, State};
use crate::connection::map_err;

mod jobs;
#[cfg(test)]
mod tests;

pub(super) use jobs::Extraction;

/// Whether `command` is one [`NoesisService::graph`] answers.
pub(super) fn is_graph(command: &Command) -> bool {
    matches!(
        command,
        Command::NoesisGraph { .. }
            | Command::NoesisDeleteProposition { .. }
            | Command::NoesisDeleteSource { .. }
            | Command::NoesisExtract { .. }
            | Command::NoesisImportSource { .. }
            | Command::NoesisJobs { .. }
            | Command::NoesisCancelJob { .. }
    )
}

impl NoesisService {
    /// Answer a validated graph command. Writes that a client may retry
    /// replay their receipt; a new job is announced and handed to the workers.
    pub(super) async fn graph(
        &self,
        runtime: &Arc<Runtime>,
        command: Command,
        submission: Option<&Submission>,
    ) -> Result<Reply, ErrorReply> {
        debug_assert!(is_graph(&command), "only graph commands reach here");
        let state = self.state.lock().await;
        let State::Running(live) = &*state else {
            return Err(ErrorReply::noesis_disabled());
        };
        let conn = live.noesis.store().connection();
        match command {
            Command::NoesisGraph {
                query,
                session,
                source,
                limit,
            } => {
                let filter = SnapshotFilter {
                    query,
                    session,
                    source,
                    limit,
                };
                let found = graph_store::snapshot(conn, &filter).map_err(graph_error)?;
                Ok(Reply::NoesisGraph {
                    nodes: found.nodes,
                    chains: found.chains,
                    clusters: found.clusters,
                    truncated: found.truncated,
                })
            }
            Command::NoesisJobs { state } => {
                let jobs = job_rows::list(conn, state).map_err(extract_error)?;
                Ok(Reply::NoesisJobs { jobs })
            }
            other => mutate(
                &Writer {
                    runtime,
                    live,
                    submission,
                },
                other,
            ),
        }
    }

    /// The hook a finished teaching turn calls: queue and start an extraction
    /// of `session` when `extraction.sessions = auto` and enough prose is new.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the teaching service calls it once it is wired")
    )]
    pub(in crate::noesis) async fn after_teaching_turn(
        &self,
        runtime: &Arc<Runtime>,
        session: &str,
    ) -> Result<Option<Job>, ErrorReply> {
        let state = self.state.lock().await;
        let State::Running(live) = &*state else {
            return Ok(None);
        };
        let store = live.noesis.store();
        let corpus = jobs::StoreCorpus::new(store, runtime.store().clone());
        let effective = live.noesis.effective();
        let queued = extract::after_teaching_turn(store.connection(), &effective, &corpus, session)
            .map_err(extract_error)?;
        let Some(job) = queued else {
            return Ok(None);
        };
        debug_assert_eq!(job.kind, JobKind::ExtractSession);
        let commit = Commit::new().event(job_event(&job));
        runtime.apply(commit).map_err(map_err)?;
        live.extraction.pump(runtime);
        Ok(Some(job))
    }
}

/// What a write needs: the runtime for events and receipts, the live service.
struct Writer<'a> {
    runtime: &'a Arc<Runtime>,
    live: &'a Live,
    submission: Option<&'a Submission>,
}

impl Writer<'_> {
    fn store(&self) -> &NoesisStore {
        self.live.noesis.store()
    }

    /// The reply an earlier request under the same key recorded.
    fn replayed(&self) -> Result<Option<Reply>, ErrorReply> {
        let Some(receipt) = self
            .runtime
            .noesis_receipt(self.submission)
            .map_err(map_err)?
        else {
            return Ok(None);
        };
        serde_json::from_value(receipt)
            .map(Some)
            .map_err(|e| ErrorReply::internal(format!("decode the graph receipt: {e}")))
    }

    /// Record `events` with the request's receipt `reply`, as one commit.
    fn record(&self, events: Vec<EventDraft>, reply: &Reply) -> Result<(), ErrorReply> {
        let mut commit = Commit::new();
        for event in events {
            commit.push_event(event);
        }
        record_noesis_receipt(&mut commit, self.submission, reply).map_err(map_err)?;
        if commit.is_empty() {
            return Ok(());
        }
        self.runtime.apply(commit).map_err(map_err).map(|_| ())
    }
}

fn mutate(writer: &Writer<'_>, command: Command) -> Result<Reply, ErrorReply> {
    if let Some(reply) = writer.replayed()? {
        return Ok(reply);
    }
    let (reply, events) = match command {
        Command::NoesisDeleteProposition { proposition, .. } => {
            let conn = writer.store().connection();
            let deleted =
                graph_store::in_write(conn, |tx| graph_store::delete_proposition(tx, &proposition))
                    .map_err(graph_error)?;
            if !deleted {
                return Err(ErrorReply::not_found(format!(
                    "proposition `{proposition}`"
                )));
            }
            (Reply::Ok, Vec::new())
        }
        Command::NoesisDeleteSource { source, graph, .. } => {
            delete_source(writer.store(), &source, graph)?;
            (Reply::Ok, Vec::new())
        }
        Command::NoesisExtract {
            session, source, ..
        } => extract_target(writer, session, source)?,
        Command::NoesisImportSource { name, artifact, .. } => import(writer, &name, artifact)?,
        Command::NoesisCancelJob { job, .. } => cancel(writer, &job)?,
        other => {
            return Err(ErrorReply::internal(format!(
                "`{}` is not a graph write",
                other.name()
            )));
        }
    };
    writer.record(events, &reply)?;
    Ok(reply)
}

/// Refused while a job reads the source: its merges would race the delete.
fn delete_source(
    store: &NoesisStore,
    source: &str,
    graph: DeleteSourceGraph,
) -> Result<(), ErrorReply> {
    let conn = store.connection();
    if let Some(job) = job_rows::live_on(conn, source).map_err(extract_error)? {
        return Err(ErrorReply::conflict(format!(
            "job `{}` is extracting source `{source}`; cancel it or wait for it to end",
            job.id
        )));
    }
    let delete_unsupported = graph == DeleteSourceGraph::DeleteUnsupported;
    let deleted = graph_store::in_write(conn, |tx| {
        graph_store::delete_source(tx, source, delete_unsupported)
    })
    .map_err(graph_error)?;
    if !deleted {
        return Err(ErrorReply::not_found(format!("source `{source}`")));
    }
    Ok(())
}

/// `noesis_extract`: the job already live on the target, or a new one.
fn extract_target(
    writer: &Writer<'_>,
    session: Option<String>,
    source: Option<String>,
) -> Result<(Reply, Vec<EventDraft>), ErrorReply> {
    let conn = writer.store().connection();
    let (kind, target) = match (session, source) {
        (Some(session), None) => {
            check_session(writer.store(), &session)?;
            (JobKind::ExtractSession, session)
        }
        (None, Some(source)) => {
            let known = graph_store::source(conn, &source).map_err(graph_error)?;
            if known.is_none() {
                return Err(ErrorReply::not_found(format!("source `{source}`")));
            }
            (JobKind::ExtractSource, source)
        }
        _ => {
            return Err(ErrorReply::invalid(
                "name exactly one of `session` and `source`",
            ));
        }
    };
    if let Some(job) = job_rows::live_on(conn, &target).map_err(extract_error)? {
        return Ok((Reply::NoesisJob { job }, Vec::new()));
    }
    let job = queue(writer, kind, &target)?;
    let event = job_event(&job);
    Ok((Reply::NoesisJob { job }, vec![event]))
}

/// A session a graph may come from: one that exists and is not practice.
fn check_session(store: &NoesisStore, session: &str) -> Result<(), ErrorReply> {
    let origin = extract::session_origin(store.connection(), session).map_err(extract_error)?;
    match origin {
        None => Err(ErrorReply::not_found(format!("session `{session}`"))),
        Some(origin) if origin.practice => Err(ErrorReply::invalid(
            "a practice session never writes to the graph",
        )),
        Some(_) => Ok(()),
    }
}

/// A new queued job, started at once if a background slot is free. With
/// `noesis.concurrency.background = 0` no job could ever run, so none is made.
fn queue(writer: &Writer<'_>, kind: JobKind, target: &str) -> Result<Job, ErrorReply> {
    if writer.live.noesis.settings().concurrency_background == 0 {
        return Err(ErrorReply::invalid(
            "noesis.concurrency.background is 0, so no background job can run",
        ));
    }
    let job = job_rows::create(writer.store().connection(), kind, target).map_err(extract_error)?;
    debug_assert_eq!(job.state, JobState::Queued);
    writer.live.extraction.pump(writer.runtime);
    Ok(job)
}

/// `noesis_import_source`: record the artifact's text as a source (the same
/// text again finds the same source) and queue its extraction.
fn import(
    writer: &Writer<'_>,
    name: &str,
    artifact: ArtifactId,
) -> Result<(Reply, Vec<EventDraft>), ErrorReply> {
    let text = jobs::read_source(writer.runtime.store().as_ref(), artifact)?;
    if text.trim().is_empty() {
        return Err(ErrorReply::invalid("the source has no text to extract"));
    }
    let row = SourceRow {
        id: ids::source_id(&text),
        name: name.trim().to_string(),
        artifact: artifact.to_string(),
        imported_at: graph_store::now_text(),
    };
    let stored =
        graph_store::insert_source(writer.store().connection(), &row).map_err(graph_error)?;
    debug_assert_eq!(stored.id, row.id);
    let live = job_rows::live_on(writer.store().connection(), &stored.id).map_err(extract_error)?;
    let (job, events) = match live {
        Some(job) => (job, Vec::new()),
        None => {
            let job = queue(writer, JobKind::ExtractSource, &stored.id)?;
            let event = job_event(&job);
            (job, vec![event])
        }
    };
    Ok((
        Reply::NoesisImport {
            source: stored.id,
            job,
        },
        events,
    ))
}

/// A queued job ends here; a running one stops at its next model-call
/// boundary, keeping its last committed merge. An ended job is returned as is.
fn cancel(writer: &Writer<'_>, job: &str) -> Result<(Reply, Vec<EventDraft>), ErrorReply> {
    let before = job_rows::get(writer.store().connection(), job).map_err(extract_error)?;
    if before.state.is_terminal() {
        return Ok((Reply::NoesisJob { job: before }, Vec::new()));
    }
    if before.state == JobState::Queued {
        let cancelled =
            job_rows::cancel_queued(writer.store().connection(), job).map_err(extract_error)?;
        if cancelled.state == JobState::Cancelled {
            let event = job_event(&cancelled);
            return Ok((Reply::NoesisJob { job: cancelled }, vec![event]));
        }
        return Ok((Reply::NoesisJob { job: cancelled }, Vec::new()));
    }
    writer.live.extraction.cancel(job);
    Ok((Reply::NoesisJob { job: before }, Vec::new()))
}

fn job_event(job: &Job) -> EventDraft {
    EventDraft::new(
        Actor::System,
        EventKind::NoesisJobUpdated { job: job.clone() },
    )
}

fn graph_error(error: GraphError) -> ErrorReply {
    ErrorReply::internal(error.to_string())
}

fn extract_error(error: ExtractError) -> ErrorReply {
    match error {
        ExtractError::UnknownJob(id) => ErrorReply::not_found(format!("job `{id}`")),
        ExtractError::Conflict(why) => ErrorReply::conflict(why),
        ExtractError::Invalid(why) => ErrorReply::invalid(why),
        other => ErrorReply::internal(other.to_string()),
    }
}
