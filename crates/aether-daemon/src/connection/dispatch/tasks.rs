//! Tasks, approvals and approval rules, and published artifacts.

use crate::Daemon;
use aether_core::event::Actor;
use aether_core::idempotency::Submission;
use aether_protocol::{Command, ErrorCode, ErrorReply, Reply};
use aether_runtime::{CreateTaskSpec, Remember};

use crate::connection::{map_err, store_err};

pub(super) fn owns(command: &Command) -> bool {
    matches!(
        command,
        Command::CreateTask(_)
            | Command::GetTask { .. }
            | Command::ListTasks { .. }
            | Command::ListApprovals
            | Command::ResolveApproval { .. }
            | Command::PublishArtifact(_)
            | Command::ListArtifacts { .. }
            | Command::ListApprovalRules { .. }
            | Command::AddApprovalRule(_)
            | Command::RevokeApprovalRule { .. }
    )
}

pub(super) async fn dispatch(
    daemon: &Daemon,
    command: Command,
    by: Actor,
    submission: Option<Submission>,
) -> Result<Reply, ErrorReply> {
    let rt = &daemon.runtime;
    let store = rt.store().as_ref();
    match command {
        Command::CreateTask(req) => create_task(daemon, req, by, submission),
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
        } => resolve_approval(daemon, id, decision, note, remember, by, submission),
        Command::PublishArtifact(req) => publish_artifact(daemon, req, by).await,
        Command::ListArtifacts { query } => Ok(Reply::Artifacts(
            store.artifacts(&query.into()).map_err(store_err)?,
        )),
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
        _ => unreachable!("routed to the wrong command family"),
    }
}

async fn publish_artifact(
    daemon: &Daemon,
    req: aether_protocol::PublishArtifact,
    by: Actor,
) -> Result<Reply, ErrorReply> {
    let rt = &daemon.runtime;
    let store = rt.store().as_ref();
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

fn create_task(
    daemon: &Daemon,
    req: aether_protocol::CreateTask,
    by: Actor,
    submission: Option<Submission>,
) -> Result<Reply, ErrorReply> {
    let spec = CreateTaskSpec {
        objective: req.objective,
        owner: req.owner,
        parent_task: req.parent_task,
        dependencies: req.dependencies,
        priority: req.priority,
    };
    Ok(Reply::Task(
        daemon
            .runtime
            .create_task_keyed(spec, by, submission)
            .map_err(map_err)?,
    ))
}

fn resolve_approval(
    daemon: &Daemon,
    id: aether_core::id::ApprovalId,
    decision: aether_core::tool::ApprovalDecision,
    note: Option<String>,
    remember: Option<aether_protocol::RememberSpec>,
    by: Actor,
    submission: Option<Submission>,
) -> Result<Reply, ErrorReply> {
    let remember = remember.map(|r| Remember {
        scope: r.scope,
        tool_wide: r.tool_wide,
        note: r.note,
    });
    daemon
        .runtime
        .resolve_approval_keyed(id, decision, by, note, remember, submission)
        .map_err(map_err)?;
    Ok(Reply::Ok)
}
