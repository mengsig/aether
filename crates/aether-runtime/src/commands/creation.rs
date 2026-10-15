use super::{CreateAgentSpec, Preflight, PreparedAgent, validation::check_narrows_parent};
use crate::submission::{created_work_gone, wrong_shape};
use crate::{CommitHold, Result, Runtime, RuntimeError};
use aether_core::agent::{Agent, AgentStats, AgentStatus, ApprovalPolicy, Role};
use aether_core::event::{Actor, EventDraft, EventKind};
use aether_core::id::{AgentId, TaskId};
use aether_core::idempotency::{Submission, SubmissionOutcome};
use aether_core::task::{Task, TaskStatus};
use aether_core::time::now;
use aether_store::{Commit, Mutation};

impl Runtime {
    /// Create an agent, optionally with an initial task, and start it.
    pub fn create_agent(&self, spec: CreateAgentSpec, by: Actor) -> Result<(Agent, Option<Task>)> {
        self.create_agent_with_preflight(spec, by, Preflight::default(), None)
    }

    /// Creation a client can retry: under `submission`'s key, a second
    /// identical request returns the agent the first one created rather than
    /// a second agent. The agent is read back, so a retry sees it as it is
    /// now and not as a copy of how it looked when it was created.
    pub fn create_agent_keyed(
        &self,
        spec: CreateAgentSpec,
        by: Actor,
        submission: Option<Submission>,
    ) -> Result<(Agent, Option<Task>)> {
        self.create_agent_with_preflight(spec, by, Preflight::default(), submission)
    }

    pub(crate) fn create_agent_with_preflight(
        &self,
        spec: CreateAgentSpec,
        by: Actor,
        preflight: Preflight<'_>,
        submission: Option<Submission>,
    ) -> Result<(Agent, Option<Task>)> {
        // Answered before anything state-dependent is checked, and before the
        // hold: by the time a retry arrives the parent it named can have
        // finished, and refusing it then would fail a request that already
        // succeeded. Deriving this agent's repository id also runs `git`, and
        // the commit lock is no place to wait on a subprocess. Unheld, so the
        // check under the hold below is still the guarantee.
        if let Some(outcome) = self.recorded_submission(submission.as_ref())? {
            return self.replayed_agent(outcome, by);
        }
        match self.create_agent_validated(spec, by.clone(), preflight, submission.as_ref()) {
            Ok(created) => Ok(created),
            // The other half of "a replay is answered before anything
            // state-dependent is checked": a retry that lost the race was
            // validated against state the winner had already moved, and its
            // commit can also have been refused by the record's primary key.
            // Either way the record exists now, so the answer is the original.
            Err(refused) => {
                let Some(hold) = self.hold_for(submission.as_ref()) else {
                    return Err(refused);
                };
                let Some(outcome) = self.replayed_submission(&hold, submission.as_ref())? else {
                    return Err(refused);
                };
                drop(hold);
                self.replayed_agent(outcome, by)
            }
        }
    }

    /// What a recorded creation returns on a retry. Seeding runs here too: it
    /// is idempotent, and the original's seed is spawned after its commit, so
    /// a crash in between would otherwise leave the repository's notes unread
    /// for good - the retry that used to create a second agent is now the only
    /// caller left to do it.
    fn replayed_agent(
        &self,
        outcome: SubmissionOutcome,
        by: Actor,
    ) -> Result<(Agent, Option<Task>)> {
        if let SubmissionOutcome::Receipt { value } = outcome {
            let (agent, task): (Agent, Option<Task>) = serde_json::from_value(value)
                .map_err(|e| RuntimeError::Internal(format!("decode agent receipt: {e}")))?;
            self.seed_repository_knowledge(&agent, by);
            return Ok((agent, task));
        }
        let SubmissionOutcome::Agent { agent, task } = outcome else {
            return Err(wrong_shape(&outcome, "agent"));
        };
        let (agent, task) = self.created_agent(agent, task)?;
        self.seed_repository_knowledge(&agent, by);
        Ok((agent, task))
    }

    fn create_agent_validated(
        &self,
        spec: CreateAgentSpec,
        by: Actor,
        preflight: Preflight<'_>,
        submission: Option<&Submission>,
    ) -> Result<(Agent, Option<Task>)> {
        let PreparedAgent {
            mut agent,
            task,
            mut commit,
            parent,
        } = self.prepare_agent(spec, &by, preflight)?;
        let run = agent.run;
        let hold = self.hold();
        // The authoritative replay check: only a retry that raced the original
        // gets this far, and this is where it is caught. Ahead of the budget
        // check, so a retry of a create that already succeeded is answered
        // rather than refused for spend the original did not face.
        if let Some(outcome) = self.replayed_submission(&hold, submission)? {
            drop(hold);
            return self.replayed_agent(outcome, by);
        }
        // Under the hold the create commits under, so the verdict is
        // linearized against concurrent usage commits rather than reading a
        // spend mid-write. It does not serialize delegations: creating a child
        // adds no spend, so K under-budget creates at once all still pass.
        // Ahead of the commit, so a refusal leaves no agent and no task behind.
        if preflight.run_budget
            && let Some(refusal) = self.run_budget_refusal(run)?
        {
            return Err(RuntimeError::Invalid(refusal));
        }
        if let Some(parent) = &parent {
            self.inherit_from_parent(&hold, parent, &mut agent)?;
            crate::cancellation::check_parent(self, &hold, parent.id)?;
        }
        commit.push_mutation(Mutation::UpsertAgent(agent.clone()));
        crate::submission::record_receipt(&mut commit, submission, &(agent.clone(), task.clone()))?;
        self.apply_held(hold, commit)?;
        self.seed_repository_knowledge(&agent, by);
        Ok((agent, task))
    }

    /// Validate `spec` and build the agent, its task and their events,
    /// without taking the hold. Nothing is written: the caller adds
    /// `UpsertAgent` and applies `commit` under its own hold.
    pub(crate) fn prepare_agent(
        &self,
        spec: CreateAgentSpec,
        by: &Actor,
        preflight: Preflight<'_>,
    ) -> Result<PreparedAgent> {
        if spec.parent.is_some() && spec.config.as_ref().and_then(|c| c.effort).is_none() {
            return Err(RuntimeError::Invalid(
                "a sub-agent needs an explicit effort: low, medium, high, xhigh or max".into(),
            ));
        }
        let checked = preflight.checked;
        let parent = match spec.parent {
            Some(id) => Some(self.live_agent(id, "parent")?),
            None => None,
        };
        if parent.as_ref().is_some_and(|parent| parent.config.leaf) {
            return Err(RuntimeError::Invalid(
                "Leaf agents cannot delegate children: do the work yourself, or ask your parent to staff it".into(),
            ));
        }
        let asked_for_model = spec.model.is_some();
        let model = spec
            .model
            .or_else(|| parent.as_ref().map(|p| p.model.clone()))
            .or_else(|| self.default_model())
            .ok_or_else(|| {
                RuntimeError::Invalid("no model configured; sign in from your client".into())
            })?;
        if checked.is_some_and(|(checked_model, _)| *checked_model != model) {
            return Err(RuntimeError::Invalid(
                "Inherited model changed during delegation checks; retry delegation".into(),
            ));
        }
        self.models.resolve(&model)?;
        // Inherited only alongside an inherited primary: a child given its own
        // model must not silently keep its parent's fallbacks.
        let fallbacks = match (asked_for_model, spec.fallbacks.is_empty()) {
            (_, false) => spec.fallbacks,
            (false, true) => parent
                .as_ref()
                .map(|p| p.fallbacks.clone())
                .unwrap_or_default(),
            (true, true) => Vec::new(),
        };
        for link in &fallbacks {
            self.models.resolve(link)?;
        }
        self.check_billing_class(&model, &fallbacks)?;
        let requested =
            self.requested_scope(spec.scope, spec.repository_root.as_deref(), parent.as_ref())?;
        let mut scope =
            self.child_scope(requested, parent.as_ref(), preflight.isolated_workspace)?;
        if checked.is_some_and(|(_, network)| network != scope.network) {
            return Err(RuntimeError::Invalid(
                "Inherited network policy changed during delegation checks; retry delegation"
                    .into(),
            ));
        }
        if scope.repository.is_none() {
            scope.repository = self.unnamed_repository(&scope, parent.as_ref())?;
        }
        self.check_backend_supported(&model, &fallbacks, &scope)?;
        let mut config = spec.config.unwrap_or_default();
        if let Some(p) = &parent
            && config.instructions.is_none()
        {
            config.instructions.clone_from(&p.config.instructions);
        }
        // §3.2 for approvals: a child inherits its parent's policy and may
        // only gate more than it, never less. A root with nothing asked for
        // gets the daemon's configured default.
        // `spec.approval` is the one channel that can say "gate me" without
        // ambiguity; a bare `config.approval` is honoured too when it is not
        // the default, for a caller that sets it directly instead. The
        // wire-protocol version this replaced `config.approval` as the only
        // channel under (when its default value doubled as an unambiguous
        // ask, because the daemon's own default was the same gated policy)
        // can no longer reach this code at all: PROTOCOL_VERSION 4 refuses
        // that client's hello before any agent is created.
        let asked = spec
            .approval
            .clone()
            .or_else(|| Some(config.approval.clone()).filter(|p| *p != ApprovalPolicy::default()));
        config.approval = match (&asked, &parent) {
            (Some(asked), Some(p)) => {
                check_narrows_parent(asked, p)?;
                asked.clone()
            }
            (Some(asked), None) => asked.clone(),
            (None, Some(p)) => p.config.approval.clone(),
            (None, None) => self.config.approval_default.policy(),
        };
        if !self.config.yolo
            && matches!(
                by,
                Actor::User {
                    origin: Some(aether_core::event::OperatorOrigin::Device { .. }),
                    ..
                }
            )
            && !config.approval.is_yolo()
            && std::iter::once(&model)
                .chain(&fallbacks)
                .any(|model| !self.engine_can_gate(model))
        {
            return Err(RuntimeError::Invalid(
                "remote creation cannot select an engine that drops required approvals".into(),
            ));
        }
        if !self.config.yolo && !config.approval.is_yolo() && !self.engine_can_gate(&model) {
            // set_approvals refuses this same end state loudly (an engine
            // that cannot gate); creation dropped it silently. Emit it here
            // too - the daemon-wide yolo case above needs no note, since
            // that switch is already visible daemon-wide.
            tracing::warn!(
                model = %model,
                "agent's approval policy dropped: this model cannot ask before acting"
            );
        }
        if self.config.yolo || !self.engine_can_gate(&model) {
            // An engine that cannot ask before acting runs ungated (see
            // ARCHITECTURE on Codex exec mode).
            config.approval = ApprovalPolicy::none();
        }
        // NaN and infinity pass every comparison the enforcement makes, so a
        // budget that can never fire would be stored as if it were a limit.
        if config.token_budget == Some(0)
            || config
                .cost_budget
                .is_some_and(|b| !b.is_finite() || b <= 0.0)
        {
            return Err(RuntimeError::Invalid(
                "a budget must be a positive, finite amount; leave it unset for no limit".into(),
            ));
        }
        self.check_cost_budget(&model, &fallbacks, config.cost_budget)?;
        let run = spec
            .run
            .or_else(|| parent.as_ref().map(|p| p.run))
            .unwrap_or_default();
        // Cheap reject before the write gate: the authoritative check below
        // holds the daemon's whole write path across this same scan.
        if preflight.run_budget
            && let Some(refusal) = self.run_budget_refusal(run)?
        {
            return Err(RuntimeError::Invalid(refusal));
        }
        let at = now();
        let role = Role::new(spec.role.trim().to_owned());
        let name = spec.name.unwrap_or_else(|| role.as_str().to_owned());
        let mut agent = Agent {
            id: AgentId::new(),
            parent: parent.as_ref().map(|p| p.id),
            role: role.clone(),
            name: name.clone(),
            scope: scope.clone(),
            model: model.clone(),
            fallbacks: fallbacks.clone(),
            status: AgentStatus::Created,
            current_task: None,
            archived: false,
            inbox_cursor: 0,
            checkpoint: None,
            engine_session: None,
            engine_usage: None,
            engine_sent: None,
            status_since: Some(at),
            // Inherited from the parent under the creator's hold.
            paused: false,
            paused_via: None,
            resume_wait: None,
            restarted_from: None,
            restarted_as: None,
            config,
            stats: AgentStats::default(),
            run,
            created_at: at,
            updated_at: at,
        };
        let mut commit = Commit::new();
        let (aid, parent_id) = (agent.id, agent.parent);
        let base = |kind: EventKind| {
            EventDraft::new(by.clone(), kind)
                .agent(aid)
                .parent(parent_id)
                .run(run)
        };
        commit.push_event(base(EventKind::AgentCreated {
            role,
            name,
            parent: agent.parent,
            model,
            fallbacks,
            scope,
        }));

        let task = match spec.objective.filter(|o| !o.trim().is_empty()) {
            Some(objective) => {
                let mut task = Task::new(objective.trim().to_owned(), by.clone(), run, at);
                task.owner = Some(agent.id);
                task.priority = agent.config.priority;
                task.transition(TaskStatus::Assigned, at)?;
                agent.current_task = Some(task.id);
                commit.push_event(
                    base(EventKind::TaskCreated {
                        objective: task.objective.clone(),
                        parent_task: None,
                        dependencies: Vec::new(),
                        priority: task.priority,
                    })
                    .task(task.id),
                );
                commit.push_event(base(EventKind::TaskAssigned { owner: agent.id }).task(task.id));
                commit.push_mutation(Mutation::UpsertTask(task.clone()));
                Some(task)
            }
            None => None,
        };
        if spec.start {
            agent.transition(AgentStatus::Ready, at)?;
            commit.push_event(base(EventKind::AgentReady));
        }
        Ok(PreparedAgent {
            agent,
            task,
            commit,
            parent,
        })
    }

    /// Re-check `agent` against its parent's current row, under the creation hold:
    /// a pause or narrowing committed since `derived_from` was read never saw the child.
    fn inherit_from_parent(
        &self,
        _hold: &CommitHold<'_>,
        derived_from: &Agent,
        agent: &mut Agent,
    ) -> Result<()> {
        let parent = self.live_agent(derived_from.id, "parent")?;
        // Derived from the old row, so possibly wider than the parent now.
        if parent.scope != derived_from.scope
            || parent.config.approval != derived_from.config.approval
        {
            return Err(RuntimeError::Invalid(format!(
                "parent {}'s scope or approval policy changed while this agent was being \
                 created; retry",
                parent.id
            )));
        }
        // §3.8: a subtree pause holds what the held supervisor's own
        // in-flight turn goes on to delegate, and the same subtree resume
        // lifts it. A plain pause holds one agent, so a child born under
        // it runs: nothing would ever lift a hold nobody asked for.
        agent.paused = parent.paused_via.is_some();
        agent.paused_via = parent.paused_via;
        Ok(())
    }

    /// The pair a recorded creation names, read back, so the answer describes
    /// the agent and task as they are now.
    fn created_agent(&self, id: AgentId, task: Option<TaskId>) -> Result<(Agent, Option<Task>)> {
        let agent = self
            .store
            .agent(id)?
            .ok_or_else(|| created_work_gone("agent", &id))?;
        let task = match task {
            Some(id) => Some(
                self.store
                    .task(id)?
                    .ok_or_else(|| created_work_gone("task", &id))?,
            ),
            None => None,
        };
        Ok((agent, task))
    }
}
