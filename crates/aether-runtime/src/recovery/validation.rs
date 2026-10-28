//! Validate references needed by live agents without touching pending cancellation.

use super::*;
use aether_core::agent::Agent;
use aether_core::id::ArtifactId;

pub(super) fn validate_live(runtime: &Arc<Runtime>) -> Result<()> {
    let cancelling = crate::cancellation::pending_agents(runtime)?;
    for mut agent in runtime.store.agents(&AgentFilter {
        live_only: true,
        ..Default::default()
    })? {
        if cancelling.contains(&agent.id)
            || matches!(
                agent.status,
                AgentStatus::Blocked {
                    reason: BlockReason::Inspection { .. }
                }
            )
        {
            continue;
        }
        let Err(error) = references(runtime, &agent) else {
            continue;
        };
        let detail = format!(
            "Agent {} needs attention: {error}. Run `aether doctor`, then `aether resume {}` after restoring the missing state.",
            agent.id, agent.id
        );
        if let Err(error) = block_invalid_reference(runtime, &mut agent, detail) {
            quarantine_failed_recovery(runtime, agent.id, &error)?;
        }
    }
    Ok(())
}

fn block_invalid_reference(runtime: &Runtime, agent: &mut Agent, detail: String) -> Result<()> {
    let keys = unreleased_keys(runtime, agent.id)?;
    agent.transition(
        AgentStatus::Blocked {
            reason: BlockReason::Inspection {
                detail: detail.clone(),
            },
        },
        now(),
    )?;
    runtime.apply(
        Commit::new()
            .event(
                EventDraft::new(
                    Actor::System,
                    EventKind::AgentBlocked {
                        reason: detail,
                        dependency: None,
                    },
                )
                .agent(agent.id)
                .run(agent.run),
            )
            .mutate(Mutation::UpsertAgent(agent.clone())),
    )?;
    runtime.resources.quarantine(agent.id, keys);
    Ok(())
}

fn references(runtime: &Runtime, agent: &Agent) -> Result<()> {
    if let Some(root) = &agent.scope.workspace_root {
        let metadata = std::fs::metadata(root).map_err(|error| {
            crate::RuntimeError::Invalid(format!(
                "workspace {} cannot be opened: {error}",
                root.display()
            ))
        })?;
        if !metadata.is_dir() {
            return Err(crate::RuntimeError::Invalid(format!(
                "workspace {} is not a directory",
                root.display()
            )));
        }
    }
    let Some(checkpoint) = runtime.store.latest_checkpoint(agent.id)? else {
        return Ok(());
    };
    if checkpoint.agent_id != agent.id {
        return Err(crate::RuntimeError::Invalid(format!(
            "checkpoint {} belongs to another agent",
            checkpoint.id
        )));
    }
    for note in checkpoint.draft.artifacts {
        let Some(reference) = note.reference else {
            continue;
        };
        let Ok(id) = reference.parse::<ArtifactId>() else {
            continue;
        };
        let artifact = runtime.store.artifact(id)?;
        if artifact.is_none() {
            return Err(crate::RuntimeError::Invalid(format!(
                "checkpoint {} refers to missing artifact {id}",
                checkpoint.id
            )));
        }
        if artifact
            .is_some_and(|artifact| artifact.digest.is_some() && artifact.evicted_at.is_none())
            && !runtime.store.artifact_bytes_exist(id)?
        {
            return Err(crate::RuntimeError::Invalid(format!(
                "checkpoint {} refers to artifact {id} whose stored payload is missing",
                checkpoint.id
            )));
        }
    }
    Ok(())
}

pub(super) fn quarantine_failed_recovery(
    runtime: &Runtime,
    id: AgentId,
    error: &crate::RuntimeError,
) -> Result<()> {
    let detail = format!(
        "Agent {id} recovery is quarantined: {error}. Run `aether doctor` and `aether diagnose`; inspect surviving tool processes before `aether resume {id}`."
    );
    runtime.report_problem(format!("recovery.{id}"), detail.clone());
    let agent = runtime.store.agent(id);
    let keys = match unreleased_keys(runtime, id) {
        Ok(keys) => keys,
        Err(error) => {
            tracing::error!(agent = %id, %error, "resource history is unreadable; conservatively fencing the workspace");
            let mut keys = vec![ResourceKey::dir(
                agent
                    .as_ref()
                    .ok()
                    .and_then(|a| a.as_ref())
                    .and_then(|a| a.scope.workspace_root.as_deref())
                    .unwrap_or(std::path::Path::new("/")),
            )];
            if let Ok(Some(agent)) = &agent {
                keys.extend(agent.scope.allowed_paths.iter().map(ResourceKey::dir));
            }
            keys
        }
    };
    runtime.resources.quarantine(id, keys);
    let mut agent = match agent {
        Ok(Some(agent)) if !agent.status.is_terminal() => agent,
        Ok(_) => return Ok(()),
        Err(error) => {
            tracing::error!(agent = %id, %error, "agent projection remains quarantined; run `aether doctor`");
            return Ok(());
        }
    };
    agent.transition(
        AgentStatus::Blocked {
            reason: BlockReason::Inspection {
                detail: detail.clone(),
            },
        },
        now(),
    )?;
    let hold = runtime.hold();
    let mut commit = Commit::new().event(
        EventDraft::new(
            Actor::System,
            EventKind::AgentBlocked {
                reason: detail,
                dependency: None,
            },
        )
        .agent(id)
        .run(agent.run),
    );
    // The turn was lost however recovery failed: an approval an engine waited
    // on inside it is expired, not left for a grant nothing can act on.
    let lost = approvals_of_unread_turn(runtime, &agent)?;
    expire_lost_approvals(runtime, &mut commit, &agent, &lost)?;
    commit.push_mutation(Mutation::UpsertAgent(agent));
    runtime.apply_held(hold, commit)?;
    Ok(())
}
