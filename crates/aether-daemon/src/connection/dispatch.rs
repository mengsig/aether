use crate::Daemon;
use crate::authz::AuthContext;
use aether_core::event::Actor;
use aether_core::idempotency::Submission;
use aether_core::model::ContentBlock;
use aether_core::schedule::ScheduleTarget;
use aether_protocol::{Command, DaemonStatus, ErrorCode, ErrorReply, Reply};
use aether_runtime::{AgentUpdate, CreateAgentSpec, CreateTaskSpec, Remember};

use super::{
    host_services, map_err, messages, providers, push, remote_admin, resolution, store_err,
};
use base64::Engine as _;

/// Where to cut `bytes` at about `max` so that text stays text. Slicing at a
/// byte offset can land inside a UTF-8 sequence, and a client decoding the
/// result then reports a text file as binary. Only an *incomplete* sequence
/// at the tail is backed off; genuinely invalid bytes are left alone, so
/// binary content is still cut at exactly `max`.
pub(super) fn text_safe_end(bytes: &[u8], max: usize) -> usize {
    match std::str::from_utf8(&bytes[..max]) {
        Ok(_) => max,
        // `error_len() == None` means "ran out of input", i.e. our own cut.
        Err(e) if e.error_len().is_none() => e.valid_up_to(),
        Err(_) => max,
    }
}

/// The most raw artifact bytes one `ReadArtifact` reply carries. Base64 adds a
/// third, and the reply must stay under the client's 64 MiB frame cap.
pub(super) const MAX_ARTIFACT_READ_BYTES: usize = 32 * 1024 * 1024;

/// Up to `max_bytes` (itself capped at `clamp`) of the artifact, cut at a
/// text boundary, and whether more follows. Reads one byte past the cap so
/// that "exactly at the cap" is not reported as truncated.
pub(super) fn read_artifact_slice(
    store: &dyn aether_store::Store,
    id: aether_core::id::ArtifactId,
    max_bytes: usize,
    clamp: usize,
) -> aether_store::Result<(Vec<u8>, bool)> {
    let cap = max_bytes.min(clamp);
    let mut bytes = store
        .read_artifact_range(id, 0, cap.saturating_add(1))?
        .unwrap_or_default();
    assert!(bytes.len() <= cap.saturating_add(1), "the store over-read");
    let truncated = bytes.len() > cap;
    if truncated {
        bytes.truncate(text_safe_end(&bytes, cap));
    }
    Ok((bytes, truncated))
}

/// Run `work` on the blocking pool. A panic in it is the connection's
/// internal error, not a lost reply.
pub(super) async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, ErrorReply> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| ErrorReply::internal(format!("the task did not finish: {e}")))
}

/// The client's idempotency key on a command that creates work, with the
/// digest of the request it arrived on. `None` when the command carries no
/// key, or is not one that creates work.
///
/// The digest is taken over the *decoded* command's named fingerprint fields
/// ([`Command::keyed_request`]), not over the client's raw line. Two
/// consequences worth knowing: a retry that spells an optional field the other
/// way - omitted where the first sent the default, `5` where it sent `5.0` -
/// fingerprints equal, because both decode to the same typed command; and a
/// field this daemon learns about later cannot change a digest an earlier one
/// wrote. Untyped values (a `trigger` or schedule `payload`) are the exception:
/// they are digested as spelled.
fn submission_of(command: &Command, auth: &AuthContext) -> Result<Option<Submission>, ErrorReply> {
    auth.submission(command)
}

pub(super) async fn dispatch_version(
    daemon: &Daemon,
    command: Command,
    by: Actor,
    auth: &AuthContext,
) -> Result<Reply, ErrorReply> {
    auth.authorize(&command)?;
    crate::authz::guard(daemon, auth, &command)?;
    if let Command::ResolveSubmission { command, horizon } = command {
        return resolution::resolve(daemon, *command, horizon, by, auth).await;
    }
    let submission = submission_of(&command, auth)?;
    dispatch_authorized(daemon, command, by, auth, submission).await
}

/// Run a command its connection is already cleared to send. `submission` is
/// the command's own, or its replay-only twin when a resolution reads back.
pub(super) async fn dispatch_authorized(
    daemon: &Daemon,
    command: Command,
    by: Actor,
    auth: &AuthContext,
    submission: Option<Submission>,
) -> Result<Reply, ErrorReply> {
    let rt = &daemon.runtime;
    let store = rt.store().as_ref();
    match command {
        Command::Hello(_) => Err(ErrorReply::invalid("already greeted")),
        // Nothing but a frame in: the point is to hold a quiet connection
        // open under the idle timeout.
        Command::Ping => Ok(Reply::Ok),
        Command::StorageStatus | Command::StorageGc { .. } => {
            let (dry_run, prune_build_output) = match command {
                Command::StorageGc {
                    dry_run,
                    prune_build_output,
                } => (dry_run, prune_build_output),
                _ => (true, false),
            };
            let reading = rt.clone();
            let report = blocking(move || {
                reading
                    .storage_gc_with(dry_run, prune_build_output)
                    .map_err(map_err)
            })
            .await??;
            Ok(Reply::StorageReport(report))
        }
        Command::PreviewRetention => {
            let reading = rt.clone();
            let preview = blocking(move || reading.preview_retention().map_err(map_err)).await??;
            Ok(Reply::RetentionPreview(preview))
        }
        Command::Status => {
            // Reads every agent and takes `sched`: on a slow disk that must not park an async worker.
            let reading = rt.clone();
            let (s, last_seq) = blocking(move || {
                let status = reading.status().map_err(map_err)?;
                let last_seq = reading.store().last_seq().map_err(store_err)?;
                Ok::<_, ErrorReply>((status, last_seq))
            })
            .await??;
            Ok(Reply::Status(DaemonStatus {
                instance_id: daemon.instance_id.clone(),
                version: daemon.version.clone(),
                pid: std::process::id(),
                started_at: rt.started_at(),
                last_seq,
                agents_live: s.agents_live,
                agents_running: s.agents_running,
                agents_waiting: s.agents_waiting,
                agents_blocked: s.agents_blocked,
                agents_suspended: s.agents_suspended,
                agents_paused: s.agents_paused,
                tasks_active: s.tasks_active,
                pending_approvals: s.pending_approvals,
                default_model: rt.default_model(),
                usage_total: s.usage_total,
                yolo: s.yolo,
                engines: s.engines,
                providers: s.providers,
                resource_leases: s.resource_leases,
                runs: s.runs,
                approval_default: s.approval_default,
                network_default: s.network_default,
                max_concurrent_turns: Some(s.max_concurrent_turns),
                max_concurrent_engine_turns: s.max_concurrent_engine_turns,
                engine_turns_running: s.engine_turns_running,
                run_budget_usd: s.run_budget_usd,
                mcp: s.mcp,
                low_disk: s.low_disk,
                bridge: rt
                    .config()
                    .bridge
                    .as_ref()
                    .map(|bridge| bridge.command.display().to_string()),
            }))
        }
        Command::Shutdown | Command::Subscribe { .. } | Command::Unsubscribe { .. } => {
            Err(ErrorReply::internal("handled by the connection loop"))
        }
        Command::ResolveSubmission { .. } => Err(ErrorReply::internal(
            "a resolution is answered before dispatch",
        )),
        Command::CreateAgent(req) => {
            if rt.default_model().is_none() && req.model.is_none() {
                rt.set_default_model(daemon.models.default_model().await);
            }
            let spec = CreateAgentSpec {
                role: req.role,
                name: req.name,
                objective: req.objective,
                model: req.model,
                fallbacks: req.fallbacks,
                scope: req.scope,
                repository_root: req.repository_root,
                parent: req.parent,
                config: req.config,
                approval: req.approval,
                run: req.run,
                start: req.start,
            };
            let (agent, _) = rt
                .create_agent_keyed(spec, by, submission)
                .map_err(map_err)?;
            Ok(Reply::Agent(agent))
        }
        Command::GetAgent { id } => store
            .agent(id)
            .map_err(store_err)?
            .map(Reply::Agent)
            .ok_or_else(|| ErrorReply::not_found(id)),
        Command::ListAgents { query } => Ok(Reply::Agents(
            store.agents(&query.into()).map_err(store_err)?,
        )),
        Command::ListRuns { limit } => Ok(Reply::Runs(
            crate::projections::list_runs(store, limit.clamp(1, 5000)).map_err(store_err)?,
        )),
        Command::GetRunReport { run } => Ok(Reply::RunReport(Box::new(
            crate::projections::build_report(store, run).map_err(store_err)?,
        ))),
        // The daemon reads and writes these paths, not the caller: a run can
        // be far larger than a socket frame. Reaching the socket already
        // means being able to create agents that run commands here, so this
        // is not a wider reach than the caller had.
        Command::Replay { agent } => Ok(Reply::Replay(
            crate::projections::replay(store, agent).map_err(store_err)?,
        )),
        // Off the async worker: a transfer holds the store lock for as long
        // as the run is large, and half a gigabyte of import would otherwise
        // stall every other client's command behind it.
        Command::ExportRun { run, path } => {
            let store = rt.store().clone();
            let exported =
                blocking(move || aether_runtime::transfer::export_run(store.as_ref(), run, &path))
                    .await?;
            Ok(Reply::RunExported(exported.map_err(map_err)?))
        }
        Command::ImportRun { path } => {
            let rt = rt.clone();
            let imported =
                blocking(move || aether_runtime::transfer::import_run(&rt, &path)).await?;
            Ok(Reply::RunImported(imported.map_err(map_err)?))
        }
        Command::GetTree {
            run,
            include_archived,
        } => Ok(Reply::Tree(
            crate::projections::build_tree(store, run, include_archived).map_err(store_err)?,
        )),
        Command::Archive {
            target, archived, ..
        } => Ok(Reply::Agents(
            rt.archive_agents_keyed(target, archived, by, submission)
                .map_err(map_err)?,
        )),
        Command::UpdateAgent { id, patch, .. } => {
            let update = AgentUpdate {
                name: patch.name,
                model: patch.model,
                fallbacks: patch.fallbacks,
                effort: patch.effort,
                instructions: patch.instructions,
                max_turns: patch.max_turns,
                token_budget: patch.token_budget,
                cost_budget: patch.cost_budget,
                tools: patch.tools,
                approval: patch.approval,
                priority: patch.priority,
                concurrency: patch.concurrency,
                deadline: patch.deadline,
                long_lived: patch.long_lived,
                network: patch.network,
                backend: patch.backend,
            };
            Ok(Reply::Agent(
                rt.update_agent_keyed(id, update, by, submission)
                    .map_err(map_err)?,
            ))
        }
        Command::ReplyToQuestion {
            to,
            question_seq,
            content,
            ..
        } => {
            let message = rt
                .reply_to_question(
                    to,
                    question_seq,
                    vec![ContentBlock::text(content)],
                    by,
                    submission,
                )
                .map_err(map_err)?;
            Ok(Reply::MessageSent { message })
        }
        command @ Command::SendMessage { .. } => {
            messages::dispatch(daemon, command, by, submission).await
        }
        Command::AppendNote {
            agent,
            text,
            links,
            context,
            level,
        } => {
            let note = aether_core::note::Note {
                text,
                links,
                context,
                level,
            };
            let seq = rt.append_note(agent, &note, by).map_err(map_err)?;
            Ok(Reply::NoteAppended { seq })
        }
        Command::Cancel { agent, reason, .. } => {
            rt.cancel_agent_keyed(agent, reason, by, submission)
                .map_err(map_err)?;
            Ok(Reply::Ok)
        }
        Command::Interrupt { agent, .. } => {
            let was_running = rt
                .interrupt_agent_keyed(agent, by, submission)
                .map_err(map_err)?;
            Ok(Reply::Interrupted { was_running })
        }
        Command::GetGithubWatcher => Ok(Reply::GithubWatcher(
            rt.github_watcher_status().map_err(map_err)?,
        )),
        Command::ConfigureGithubWatcher {
            config,
            expected_revision,
        } => {
            if !matches!(by, Actor::User { .. }) {
                return Err(ErrorReply::new(
                    ErrorCode::Unauthorized,
                    "only an operator may configure the GitHub watcher",
                ));
            }
            Ok(Reply::GithubWatcher(
                rt.configure_github_watcher(config, expected_revision)
                    .map_err(map_err)?,
            ))
        }
        Command::CreateSchedule { spec, .. } => {
            // A spawn template without a model is frozen to the default now,
            // so resolve it lazily the way `create_agent` does.
            if rt.default_model().is_none()
                && matches!(&spec.target, ScheduleTarget::Spawn(template) if template.model.is_none())
            {
                rt.set_default_model(daemon.models.default_model().await);
            }
            Ok(Reply::Schedule(
                rt.create_schedule_keyed(spec, by, submission)
                    .map_err(map_err)?,
            ))
        }
        Command::GetSchedule { id } => Ok(Reply::Schedule(
            rt.store()
                .schedule(id)
                .map_err(store_err)?
                .ok_or_else(|| map_err(aether_runtime::RuntimeError::NotFound(id.to_string())))?,
        )),
        Command::ListSchedules { target } => Ok(Reply::Schedules(
            rt.store().schedules(target).map_err(store_err)?,
        )),
        Command::UpdateSchedule { id, action, .. } => Ok(Reply::Schedule(
            rt.update_schedule_keyed(id, action, by, submission)
                .map_err(map_err)?,
        )),
        Command::ScheduleHistory { id, limit } => Ok(Reply::ScheduleHistory(
            rt.store()
                .schedule_history(id, limit.min(1000))
                .map_err(store_err)?,
        )),
        Command::Trigger {
            agent,
            source,
            payload,
            ..
        } => {
            rt.trigger_agent_keyed(agent, source, payload, by, submission)
                .map_err(map_err)?;
            Ok(Reply::Ok)
        }
        Command::Restart { agent, .. } => Ok(Reply::Agent(
            rt.restart_agent_keyed(agent, by, submission)
                .map_err(map_err)?,
        )),
        Command::Wake { agent } => Ok(Reply::Agent(rt.wake_agent(agent, by).map_err(map_err)?)),
        Command::Pause { agent, subtree, .. } => {
            let outcome = rt
                .pause_agent_keyed(agent, subtree, by, submission)
                .map_err(map_err)?;
            Ok(Reply::Paused {
                agents: outcome.changed,
                mid_turn: outcome.mid_turn,
            })
        }
        Command::Resume { agent, subtree, .. } => {
            let agents = rt
                .resume_agent_keyed(agent, subtree, by, submission)
                .map_err(map_err)?;
            Ok(Reply::Agents(agents))
        }
        Command::SetApprovals {
            agent,
            policy,
            subtree,
            ..
        } => Ok(Reply::Agents(
            rt.set_approvals_keyed(agent, policy, subtree, by, submission)
                .map_err(map_err)?,
        )),
        Command::Compact { agent } => {
            rt.request_compaction(agent, by).map_err(map_err)?;
            Ok(Reply::Ok)
        }
        Command::WorkspaceDiff { agent } => {
            let reading = rt.clone();
            let diff = blocking(move || reading.workspace_diff(agent).map_err(map_err)).await??;
            Ok(Reply::WorkspaceDiff(Box::new(diff)))
        }
        Command::WorkspaceIntegrate { agent } => {
            let writing = rt.clone();
            let integration =
                blocking(move || writing.workspace_integrate(agent, by).map_err(map_err)).await??;
            Ok(Reply::WorkspaceIntegration(integration))
        }
        Command::WorkspaceDiscard { agent, force } => {
            let writing = rt.clone();
            let discard =
                blocking(move || writing.workspace_discard(agent, force, by).map_err(map_err))
                    .await??;
            Ok(Reply::WorkspaceDiscarded(discard))
        }
        Command::EngineTool {
            agent,
            token,
            name,
            input,
        } => {
            let (content, is_error) = rt
                .engine_tool(agent, &token, name, input)
                .await
                .map_err(map_err)?;
            Ok(Reply::EngineTool { content, is_error })
        }
        Command::CreateTask(req) => {
            let spec = CreateTaskSpec {
                objective: req.objective,
                owner: req.owner,
                parent_task: req.parent_task,
                dependencies: req.dependencies,
                priority: req.priority,
            };
            Ok(Reply::Task(
                rt.create_task_keyed(spec, by, submission)
                    .map_err(map_err)?,
            ))
        }
        Command::GetTask { id } => store
            .task(id)
            .map_err(store_err)?
            .map(Reply::Task)
            .ok_or_else(|| ErrorReply::not_found(id)),
        Command::ListTasks { query } => {
            Ok(Reply::Tasks(store.tasks(&query.into()).map_err(store_err)?))
        }
        Command::ListApprovals => Ok(Reply::Approvals(
            store.pending_approvals().map_err(store_err)?,
        )),
        Command::ResolveApproval {
            id,
            decision,
            note,
            remember,
            ..
        } => {
            let remember = remember.map(|r| Remember {
                scope: r.scope,
                tool_wide: r.tool_wide,
                note: r.note,
            });
            rt.resolve_approval_keyed(id, decision, by, note, remember, submission)
                .map_err(map_err)?;
            Ok(Reply::Ok)
        }
        Command::PublishArtifact(req) => {
            if req.name.trim().is_empty()
                || req.name.len() > 256
                || req.summary.len() > 4096
                || req.content_type.len() > 256
                || req.text.len() > 1024 * 1024
            {
                return Err(ErrorReply::new(
                    ErrorCode::InvalidArgument,
                    "artifact requires a name (up to 256 bytes), summary up to 4096 bytes, content type up to 256 bytes and payload up to 1 MiB",
                ));
            }
            let task = store
                .task(req.task)
                .map_err(store_err)?
                .ok_or_else(|| ErrorReply::not_found(req.task))?;
            let agent = task.owner.ok_or_else(|| {
                ErrorReply::new(
                    ErrorCode::InvalidArgument,
                    "artifact task must have an owner",
                )
            })?;
            if store.agent(agent).map_err(store_err)?.is_none() {
                return Err(ErrorReply::not_found(agent));
            }
            let artifact = rt
                .publish_artifact(
                    aether_runtime::artifacts::ArtifactPublication {
                        kind: aether_core::artifact::ArtifactKind::Result,
                        name: req.name,
                        summary: req.summary,
                        content_type: Some(req.content_type),
                        bytes: req.text.into_bytes(),
                        agent,
                        task: Some(task.id),
                    },
                    by,
                )
                .await
                .map_err(|error| ErrorReply::new(ErrorCode::Internal, error.to_string()))?;
            Ok(Reply::Artifact(artifact))
        }
        Command::ListArtifacts { query } => Ok(Reply::Artifacts(
            store.artifacts(&query.into()).map_err(store_err)?,
        )),
        Command::EngineTools { agent, token } => Ok(Reply::EngineTools {
            tools: rt.engine_tools(agent, &token).map_err(map_err)?,
        }),
        Command::McpServers { refresh, server } => {
            let Some(mcp) = rt.tools().mcp().cloned() else {
                return Ok(Reply::McpServers(Vec::new()));
            };
            if refresh {
                match &server {
                    // A named server that cannot be reached is the answer the
                    // caller asked for, so its health is returned, not an error.
                    Some(name) => {
                        if let Err(e) = mcp.discover(name).await
                            && matches!(e, aether_tools::McpError::UnknownServer(_))
                        {
                            return Err(ErrorReply::new(ErrorCode::NotFound, e.to_string()));
                        }
                    }
                    None => mcp.discover_all().await,
                }
            }
            let health = mcp.health();
            Ok(Reply::McpServers(match server {
                Some(name) => health.into_iter().filter(|h| h.name == name).collect(),
                None => health,
            }))
        }
        Command::ListApprovalRules { include_revoked } => Ok(Reply::ApprovalRules(
            rt.approval_rules(include_revoked).map_err(map_err)?,
        )),
        Command::AddApprovalRule(req) => Ok(Reply::ApprovalRule(
            rt.add_approval_rule_keyed(
                req.decision,
                req.matcher,
                req.scope,
                by,
                req.note,
                submission,
            )
            .map_err(map_err)?,
        )),
        Command::RevokeApprovalRule { id, reason } => Ok(Reply::ApprovalRule(
            rt.revoke_approval_rule(id, by, reason).map_err(map_err)?,
        )),
        Command::GetEvents {
            after,
            limit,
            query,
        } => {
            let before = query.before;
            if before.is_some() && after != 0 {
                return Err(ErrorReply::invalid(
                    "before cannot be combined with a nonzero after",
                ));
            }
            let filter = query.into();
            let limit = limit.clamp(1, 5000);
            let events = match before {
                Some(before) => store.events_before(before, limit, &filter),
                None => store.events(after, limit, &filter),
            };
            Ok(Reply::Events(events.map_err(store_err)?))
        }
        Command::GetTranscript { agent, after } => Ok(Reply::Transcript(
            store
                .transcript(agent, after)
                .map_err(store_err)?
                .into_iter()
                .map(Into::into)
                .collect(),
        )),
        Command::GetInbox { agent, after } => {
            Ok(Reply::Inbox(store.inbox(agent, after).map_err(store_err)?))
        }
        Command::GetSnapshot { id } => store
            .snapshot(id)
            .map_err(store_err)?
            .map(Reply::Snapshot)
            .ok_or_else(|| ErrorReply::not_found(id)),
        Command::GetInvocation { id } => store
            .invocation(id)
            .map_err(store_err)?
            .map(Reply::Invocation)
            .ok_or_else(|| ErrorReply::not_found(id)),
        Command::ListInvocations { agent } => Ok(Reply::Invocations(
            store.invocations_for_agent(agent).map_err(store_err)?,
        )),
        Command::GetCheckpoint { id } => {
            Ok(Reply::Checkpoint(store.checkpoint(id).map_err(store_err)?))
        }
        Command::LatestCheckpoint { agent } => Ok(Reply::Checkpoint(
            store.latest_checkpoint(agent).map_err(store_err)?,
        )),
        Command::GetArtifact { id } => store
            .artifact(id)
            .map_err(store_err)?
            .map(Reply::Artifact)
            .ok_or_else(|| ErrorReply::not_found(id)),
        Command::ReadArtifact { id, max_bytes } => {
            let artifact = store
                .artifact(id)
                .map_err(store_err)?
                .ok_or_else(|| ErrorReply::not_found(id))?;
            let store = rt.store().clone();
            let read = blocking(move || {
                read_artifact_slice(store.as_ref(), id, max_bytes, MAX_ARTIFACT_READ_BYTES)
            })
            .await?;
            let (slice, truncated) = read.map_err(store_err)?;
            Ok(Reply::ArtifactBytes {
                artifact,
                base64: base64::engine::general_purpose::STANDARD.encode(slice),
                truncated,
            })
        }
        Command::Memories { query } => {
            Ok(Reply::Memories(rt.memories(query.into()).map_err(map_err)?))
        }
        Command::CreateMemory { spec, .. } => {
            let rt = rt.clone();
            let result = blocking(move || rt.create_memory_keyed(spec, by, submission)).await?;
            Ok(Reply::Memory(result.map_err(map_err)?))
        }
        Command::AcceptMemory { id, reason } => Ok(Reply::Memory(
            rt.accept_memory(id, reason, by).map_err(map_err)?,
        )),
        Command::RejectMemory { id, reason } => Ok(Reply::Memory(
            rt.reject_memory(id, reason, by).map_err(map_err)?,
        )),
        Command::RecordMemoryFeedback {
            memory,
            verdict,
            evidence,
            task,
            invocation,
            reason,
            ..
        } => {
            let rt = rt.clone();
            let spec = aether_core::memory_feedback::MemoryFeedbackSpec {
                memory,
                verdict,
                evidence,
                task,
                invocation,
                reason,
            };
            let result =
                blocking(move || rt.record_memory_feedback_keyed(spec, by, submission)).await?;
            Ok(Reply::MemoryFeedback(result.map_err(map_err)?))
        }
        Command::RetractMemoryFeedback { feedback, reason } => {
            let rt = rt.clone();
            let result = blocking(move || rt.retract_memory_feedback(feedback, reason, by)).await?;
            Ok(Reply::MemoryFeedback(result.map_err(map_err)?))
        }
        Command::MemoryFeedback { memory, limit } => {
            let rt = rt.clone();
            let result = blocking(move || rt.memory_feedback(memory, limit, &by)).await?;
            Ok(Reply::MemoryFeedbackList(result.map_err(map_err)?))
        }
        Command::ForgetMemory { id, reason } => Ok(Reply::Memory(
            rt.forget_memory(id, reason, by).map_err(map_err)?,
        )),
        Command::RestoreMemory { id } => {
            Ok(Reply::Memory(rt.restore_memory(id, by).map_err(map_err)?))
        }
        Command::PinMemory { id, pinned } => Ok(Reply::Memory(
            rt.pin_memory(id, pinned, by).map_err(map_err)?,
        )),
        // Off the async worker: this scans the tree with a git subprocess
        // per note, then sweeps with two more per file - the create path
        // treats the identical work as blocking for the same reason.
        Command::SyncRepositoryKnowledge { root } => {
            let rt = rt.clone();
            let synced = blocking(move || rt.sync_repository_knowledge(root, by)).await?;
            let (repository, report) = synced.map_err(map_err)?;
            Ok(Reply::MemorySeeded { repository, report })
        }
        // Off the async worker: a root runs git, and the move rewrites every
        // memory filed under the alias in one transaction.
        Command::AliasRepository {
            alias,
            root,
            repository,
            dry_run,
        } => {
            let rt = rt.clone();
            let aliased =
                blocking(move || rt.alias_repository(&alias, root, repository, dry_run, by))
                    .await?;
            Ok(Reply::RepositoryAliased(aliased.map_err(map_err)?))
        }
        Command::ListRepositoryAliases => Ok(Reply::RepositoryAliases(
            rt.repository_aliases().map_err(map_err)?,
        )),
        Command::MemoriesInContext { task, invocation } => Ok(Reply::MemoriesInContext(
            rt.memories_in_context(task, invocation, &by)
                .map_err(map_err)?,
        )),
        Command::ListModels => providers::list_models(daemon).await,
        Command::AuthStatus => providers::auth_status(daemon).await,

        Command::Providers => providers::providers(daemon).await,
        Command::RefreshProviderUsage { provider } => Ok(Reply::ProviderUsage(
            daemon
                .runtime
                .refresh_provider_usage(provider.as_deref())
                .await
                .map_err(map_err)?,
        )),

        command @ (Command::GetAttention
        | Command::CompletePath { .. }
        | Command::ListWorkspaces { .. }
        | Command::ValidateWorkspace { .. }
        | Command::GetGitSummary { .. }) => host_services::dispatch(daemon, command).await,
        command @ (Command::GetRemote
        | Command::ConfigureRemote { .. }
        | Command::StartPairing { .. }
        | Command::GetPairing { .. }
        | Command::ConfirmPairing { .. }
        | Command::CancelPairing { .. }
        | Command::ListDevices { .. }
        | Command::RevokeDevice { .. }
        | Command::UpdateDevice { .. }) => remote_admin::dispatch(daemon, command, by, auth).await,
        command @ (Command::GetPushConfig
        | Command::SubscribePush { .. }
        | Command::UnsubscribePush
        | Command::TestPush) => push::dispatch(daemon, command, auth).await,
    }
}

#[cfg(test)]
pub(super) async fn dispatch(
    daemon: &Daemon,
    command: Command,
    by: Actor,
) -> Result<Reply, ErrorReply> {
    dispatch_version(daemon, command, by, &AuthContext::Local).await
}
