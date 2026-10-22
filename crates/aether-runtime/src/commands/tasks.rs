use super::CreateTaskSpec;
use crate::submission::{created_work_gone, wrong_shape};
use crate::{CommitHold, Result, Runtime, RuntimeError};
use aether_core::agent::Agent;
use aether_core::event::{Actor, EventDraft, EventKind};
use aether_core::id::TaskId;
use aether_core::idempotency::{Submission, SubmissionOutcome};
use aether_core::inbox::InboxKind;
use aether_core::task::{Task, TaskStatus, TaskStatusKind};
use aether_core::time::now;
use aether_store::{Commit, Mutation};

impl Runtime {
    pub fn create_task(&self, spec: CreateTaskSpec, by: Actor) -> Result<Task> {
        self.create_task_keyed(spec, by, None)
    }

    /// Creation a client can retry: under `submission`'s key, a second
    /// identical request returns the task the first one created.
    pub fn create_task_keyed(
        &self,
        spec: CreateTaskSpec,
        by: Actor,
        submission: Option<Submission>,
    ) -> Result<Task> {
        if spec.objective.trim().is_empty() {
            return Err(RuntimeError::Invalid("task objective is empty".into()));
        }
        super::messaging::check_text_size("task objective", spec.objective.len())?;
        let at = now();
        // Held across the owner and dependency reads, keyed or not: the owner
        // row is written back whole, and its `current_task` must not be lost.
        let hold = self.hold();
        if let Some(outcome) = self.replayed_submission(&hold, submission.as_ref())? {
            if let SubmissionOutcome::Receipt { value } = outcome {
                return serde_json::from_value(value)
                    .map_err(|e| RuntimeError::Internal(format!("decode task receipt: {e}")));
            }
            let SubmissionOutcome::Task { task } = outcome else {
                return Err(wrong_shape(&outcome, "task"));
            };
            return self
                .store
                .task(task)?
                .ok_or_else(|| created_work_gone("task", &task));
        }
        let owner = match spec.owner {
            Some(id) => Some(self.live_agent(id, "task owner")?),
            None => None,
        };
        let run = owner.as_ref().map(|o| o.run).unwrap_or_default();
        let mut unmet = false;
        for id in &spec.dependencies {
            let dependency = self
                .store
                .task(*id)?
                .ok_or_else(|| RuntimeError::NotFound(format!("dependency {id}")))?;
            // The gate would release the task at once, as if the work it waits
            // for had been done: refuse instead of gating on nothing.
            if matches!(
                dependency.status.kind(),
                TaskStatusKind::Cancelled | TaskStatusKind::Failed
            ) {
                return Err(RuntimeError::Invalid(format!(
                    "dependency {id} is {} and will never produce a result to wait for",
                    dependency.status.kind()
                )));
            }
            unmet |= !dependency.status.is_terminal()
                || crate::wait::task_turn_running(self, dependency.id);
        }
        let mut task = Task::new(spec.objective.trim().to_owned(), by.clone(), run, at);
        task.parent_task = spec.parent_task;
        task.dependencies = spec.dependencies;
        task.priority = spec.priority;
        let mut commit = Commit::new();
        let mut created = EventDraft::new(
            by.clone(),
            EventKind::TaskCreated {
                objective: task.objective.clone(),
                parent_task: task.parent_task,
                dependencies: task.dependencies.clone(),
                priority: task.priority,
            },
        )
        .task(task.id)
        .run(run);
        created.agent_id = owner.as_ref().map(|o| o.id);
        commit.push_event(created);
        if let Some(mut owner) = owner {
            if let Some(held) = owner.current_task {
                return Err(RuntimeError::Invalid(format!(
                    "agent {} already owns a task ({held}) and an agent carries one \
                     task at a time: give this one to another agent, or create it \
                     without an owner",
                    owner.id
                )));
            }
            task.owner = Some(owner.id);
            owner.current_task = Some(task.id);
            // Unmet dependencies leave the task Queued; the scheduler assigns
            // it and wakes the owner once they are all terminal (§3.2).
            if !unmet {
                task.transition(TaskStatus::Assigned, at)?;
                commit.push_event(
                    EventDraft::new(by.clone(), EventKind::TaskAssigned { owner: owner.id })
                        .agent(owner.id)
                        .task(task.id)
                        .run(run),
                );
                self.announce_assignment(&hold, &mut commit, &owner, task.id, by)?;
            }
            commit.push_mutation(Mutation::UpsertAgent(owner));
        }
        commit.push_mutation(Mutation::UpsertTask(task.clone()));
        crate::submission::record_receipt(&mut commit, submission.as_ref(), &task)?;
        self.apply_held(hold, commit)?;
        Ok(task)
    }

    /// Tell the owner through its inbox, as `assign_and_wake` does: that wakes a
    /// `Waiting { Inbox }` owner and gives a Ready one work; others read it next turn.
    fn announce_assignment(
        &self,
        hold: &CommitHold<'_>,
        commit: &mut Commit,
        owner: &Agent,
        task: TaskId,
        by: Actor,
    ) -> Result<()> {
        let reason = format!("task {task} was assigned to you");
        self.push_inbox(hold, commit, owner, InboxKind::Wake { reason }, by)?;
        Ok(())
    }

    /// Deliver a task outcome to whoever created the task, if it is a live agent.
    pub(crate) fn notify_task_creator(
        &self,
        hold: &CommitHold<'_>,
        commit: &mut Commit,
        task: &Task,
        kind: InboxKind,
        by: Actor,
    ) -> Result<()> {
        if let Actor::Agent { id } = task.creator
            && let Some(creator) = self.store.agent(id)?
            && !creator.status.is_terminal()
        {
            self.push_inbox(hold, commit, &creator, kind, by)?;
        }
        Ok(())
    }
}
