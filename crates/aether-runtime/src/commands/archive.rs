use crate::{Result, Runtime, RuntimeError};
use aether_core::agent::{Agent, ArchiveTarget};
use aether_core::event::{Actor, EventDraft, EventKind};
use aether_core::id::AgentId;
use aether_core::time::now;
use aether_store::{AgentFilter, Commit, Mutation};

impl Runtime {
    /// Clear finished agents out of the live view, or bring them back. The
    /// mark is durable and nothing leaves the store: reports and replay read
    /// the same records they always did. Returns the agents this
    /// call actually moved.
    pub fn archive_agents(
        &self,
        target: ArchiveTarget,
        archived: bool,
        by: Actor,
    ) -> Result<Vec<Agent>> {
        let single_agent = matches!(target, ArchiveTarget::Agent(_));
        let eligible = |agent: &Agent| agent.archived != archived && !(archived && agent.is_live());
        let candidates = match target {
            ArchiveTarget::Agent(id) => vec![id],
            ArchiveTarget::Run(run) => self
                .store
                .agents(&AgentFilter {
                    run: Some(run),
                    ..Default::default()
                })?
                .into_iter()
                .filter(eligible)
                .map(|agent| agent.id)
                .collect(),
            ArchiveTarget::AllFinished => self
                .store
                .agents(&AgentFilter::default())?
                .into_iter()
                .filter(eligible)
                .map(|agent| agent.id)
                .collect(),
        };
        // `AllFinished` can cover a daemon's whole history; one commit per
        // this many keeps each transaction (and the commit lock's hold)
        // bounded instead of one unbounded commit over everything a long-
        // running daemon has ever archived.
        const SWEEP_CHUNK: usize = 500;
        let mut changed = Vec::new();
        let verb = if archived { "archived" } else { "restored" };
        for candidates in candidates.chunks(SWEEP_CHUNK) {
            // Earlier chunks stay applied if a later read or write fails.
            let moved = self
                .archive_chunk(candidates, archived, single_agent, &by)
                .map_err(|error| {
                    if changed.is_empty() {
                        error
                    } else {
                        RuntimeError::Internal(format!(
                            "{} agent(s) already {verb} before this failure: {error}",
                            changed.len()
                        ))
                    }
                })?;
            changed.extend(moved);
        }
        Ok(changed)
    }

    pub fn archive_agents_keyed(
        &self,
        target: ArchiveTarget,
        archived: bool,
        by: Actor,
        submission: Option<aether_core::idempotency::Submission>,
    ) -> Result<Vec<Agent>> {
        if submission.is_none() {
            return self.archive_agents(target, archived, by);
        }
        let hold = self.hold();
        if let Some(receipt) = self.replay_receipt(&hold, submission.as_ref())? {
            return Ok(receipt);
        }
        let single = matches!(target, ArchiveTarget::Agent(_));
        let candidates = match target {
            ArchiveTarget::Agent(id) => vec![id],
            other => self
                .store
                .agents(&AgentFilter {
                    run: match other {
                        ArchiveTarget::Run(run) => Some(run),
                        _ => None,
                    },
                    ..Default::default()
                })?
                .into_iter()
                .map(|agent| agent.id)
                .collect(),
        };
        let (commit, moved) = self.prepare_archive(&candidates, archived, single, &by)?;
        self.commit_receipt(hold, commit, submission.as_ref(), &moved)?;
        Ok(moved)
    }

    fn archive_chunk(
        &self,
        candidates: &[AgentId],
        archived: bool,
        single_agent: bool,
        by: &Actor,
    ) -> Result<Vec<Agent>> {
        let hold = self.hold();
        let (commit, moved) = self.prepare_archive(candidates, archived, single_agent, by)?;
        self.apply_held(hold, commit)?;
        Ok(moved)
    }

    fn prepare_archive(
        &self,
        candidates: &[AgentId],
        archived: bool,
        single_agent: bool,
        by: &Actor,
    ) -> Result<(Commit, Vec<Agent>)> {
        let at = now();
        let mut commit = Commit::new();
        let mut moved = Vec::new();
        for &id in candidates {
            // Retention may remove a finished run after candidate discovery.
            let Some(mut agent) = self.store.agent(id)? else {
                if single_agent {
                    return Err(RuntimeError::NotFound(id.to_string()));
                }
                continue;
            };
            // Archiving a live agent would hide work still happening.
            if archived && agent.is_live() {
                if single_agent {
                    return Err(RuntimeError::Invalid(format!(
                        "agent {} is {}; only finished work can be cleared from the view",
                        agent.id,
                        agent.status.kind()
                    )));
                }
                // A sweep passes over live work rather than refusing.
                continue;
            }
            if agent.archived == archived {
                continue;
            }
            agent.archived = archived;
            agent.updated_at = at;
            commit.push_event(
                EventDraft::new(by.clone(), EventKind::AgentArchived { archived })
                    .agent(agent.id)
                    .run(agent.run),
            );
            commit.push_mutation(Mutation::UpsertAgent(agent.clone()));
            moved.push(agent);
        }
        Ok((commit, moved))
    }
}
