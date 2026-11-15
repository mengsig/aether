use super::*;
use aether_core::shell_network::gated_class;

/// What to tell a model that named a tool it cannot use. A tool the registry
/// has but this agent's exposure or scope withholds says which, so a denied
/// network or a container backend is legible rather than looking like a typo.
/// Report alternatives under the requested child policy, never widen that policy
/// merely to offer a model. Credentials and engine readiness are checked once
/// per provider because they are shared by its catalog entries.
///
/// When the requested model id is also served by a different, available
/// provider, lead with that instead of the full list: it is the one edit that
/// turns the refusal into a working delegate call. With more than one such
/// provider, prefer the one on the same engine protocol as the parent's own
/// model - the family already proven to run for this lineage - over
/// iteration order.
pub(super) async fn delegation_refusal(
    runtime: &Runtime,
    requested: &ModelRef,
    network: aether_core::agent::NetworkPolicy,
    parent_model: &ModelRef,
    reason: impl std::fmt::Display,
) -> Result<ToolOutcome> {
    let mut providers = BTreeMap::new();
    let mut alternatives = BTreeSet::new();
    let mut same_id: Vec<(ModelRef, aether_model::Protocol)> = Vec::new();
    for info in runtime.models.list() {
        let (available, protocol) = match providers.get(&info.model.provider) {
            Some(cached) => *cached,
            None => {
                let resolved = runtime.models.resolve(&info.model)?;
                let protocol = resolved.provider.protocol;
                let engine_ready = match crate::turn_engine::engine_kind(protocol) {
                    None => true,
                    Some(kind) => runtime
                        .engines
                        .ready()
                        .await
                        .get(&kind)
                        .is_some_and(|engine| {
                            engine
                                .validate_network_policy(
                                    network == aether_core::agent::NetworkPolicy::Allowed,
                                )
                                .is_ok()
                        }),
                };
                let configured = runtime
                    .models
                    .auth_status(&info.model.provider)
                    .await
                    .configured;
                let available = engine_ready && configured;
                providers.insert(info.model.provider.clone(), (available, protocol));
                (available, protocol)
            }
        };
        if !available {
            continue;
        }
        alternatives.insert(info.model.to_string());
        if info.model.model == requested.model && info.model.provider != requested.provider {
            same_id.push((info.model.clone(), protocol));
        }
    }
    if !same_id.is_empty() {
        // Deterministic base order before the engine-family preference is applied.
        same_id.sort_by(|(a, _), (b, _)| a.cmp(b));
        let parent_protocol = runtime
            .models
            .resolve(parent_model)
            .ok()
            .map(|resolved| resolved.provider.protocol);
        let chosen = same_id
            .iter()
            .find(|(_, protocol)| Some(*protocol) == parent_protocol)
            .unwrap_or(&same_id[0]);
        return Ok(ToolOutcome::error(format!(
            "Cannot delegate to model `{requested}`: {reason}. The same model is available as `{}`.",
            chosen.0
        )));
    }
    let alternatives = if alternatives.is_empty() {
        "none under the requested network policy; use list_models to inspect provider readiness"
            .into()
    } else {
        alternatives.into_iter().collect::<Vec<_>>().join(", ")
    };
    Ok(ToolOutcome::error(format!(
        "Cannot delegate to model `{requested}`: {reason}. Locally runnable models: {alternatives}"
    )))
}

pub(super) fn unavailable(
    runtime: &Arc<Runtime>,
    agent: &aether_core::agent::Agent,
    name: &str,
) -> String {
    if agent.config.leaf && name == "delegate" {
        return "Leaf agents cannot delegate children: do the work yourself, or ask your parent to staff it".into();
    }
    if let Some(reason) = crate::withheld_by_position(agent, name) {
        return format!("`{name}` is withheld from {reason}");
    }
    match runtime
        .tools
        .withheld_reason(name, &agent.config.tools, &agent.scope)
    {
        Some(reason) => format!("`{name}` is not available to this agent: {reason}"),
        None => format!("unknown tool `{name}`"),
    }
}

#[derive(Default)]
pub(crate) struct ToolEffects {
    pub(super) pending_approvals: Vec<ApprovalId>,
    pub(crate) next: Option<Next>,
    /// A question recorded this turn promises the agent parks on its answer,
    /// so nothing later in the turn may end the agent or ask again.
    pub(super) asked: Option<String>,
}

impl ToolEffects {
    /// This turn's question was answered inside it: it no longer blocks a
    /// settle or a further question.
    pub(crate) fn question_answered(&mut self) {
        debug_assert!(self.asked.is_some(), "no question was asked");
        self.asked = None;
    }

    pub(crate) fn terminal(&self) -> bool {
        matches!(self.next, Some(Next::Completed(_) | Next::Failed(_)))
    }

    /// A harness action has decided how the turn ends. The API path returns
    /// straight after the batch; an engine drives its own loop and has to be
    /// stopped (`turn_engine::drive`).
    pub(crate) fn parked(&self) -> bool {
        matches!(
            self.next,
            Some(Next::Waiting(_) | Next::Completed(_) | Next::Failed(_))
        )
    }

    /// Whether an action may still run once the turn's end is decided.
    ///
    /// Engines emit a parallel tool batch as separate bridge calls, so the
    /// siblings of the parking call arrive after it. Read-only inspection
    /// still answers; anything that starts work or delivers an effect, a
    /// second decision and descendant cancellation are refused; a further
    /// wait merges into the one already recorded.
    ///
    /// `long_lived`: a settle for such an agent only parks it on
    /// `Waiting(Inbox)` rather than ending it, so it is legitimate after an
    /// earlier park (e.g. `ask_user`) - mirrors `!ctx.agent.config.long_lived`
    /// in `apply_action`'s own settle guard.
    pub(crate) fn admits_after_park(&self, action: &HarnessAction, long_lived: bool) -> bool {
        // Cancelling awaited work can immediately satisfy the wait and wake
        // this agent before its deadline. It belongs to the resumed turn,
        // not to the native process's reporting/wind-down window.
        if matches!(action, HarnessAction::CancelAgent { .. }) {
            return false;
        }
        // A redundant wind-down wait must not disable a timer with an Inbox
        // conjunct, or extend it by adding another deadline. Untimed explicit
        // task waits and delegation siblings retain their merge behavior.
        if let HarnessAction::WaitFor { condition } = action {
            let already_timed = matches!(&self.next, Some(Next::Waiting(wait))
                if wait.earliest_deadline().is_some());
            if *condition == WaitCondition::Inbox || already_timed {
                return false;
            }
        }
        match decides(action) {
            Decides::Nothing => true,
            Decides::Effect => false,
            Decides::MergingWait => matches!(self.next, Some(Next::Waiting(_))),
            // Self-guarding like the arm above: a long_lived settle only
            // ever records `Waiting`, never `Completed`/`Failed`, but this
            // does not lean on that invariant holding elsewhere.
            Decides::Settles => long_lived && matches!(self.next, Some(Next::Waiting(_))),
            // The turn's *first* question is a merging wait too - it folds
            // into whatever wait already parked the turn (an earlier
            // `WaitFor`/delegate-wait, or a long_lived settle's `Inbox`)
            // exactly like `apply_action`'s own `AskUser` arm does on the
            // batch path. Only a second question, a pending approval, or a
            // genuinely terminal settle refuse it.
            Decides::Asks => {
                self.asked.is_none()
                    && self.pending_approvals.is_empty()
                    && !matches!(self.next, Some(Next::Completed(_) | Next::Failed(_)))
            }
        }
    }

    /// A refusal naming the question the turn is parked on, for a caller whose
    /// own message is generic. `None`: nothing was asked.
    pub(crate) fn refusal_for_asked(&self, action: &HarnessAction) -> Option<String> {
        let question = self.asked.as_deref()?;
        match action {
            HarnessAction::CompleteTask { .. } => {
                Some(still_asking("complete", "completed", question))
            }
            HarnessAction::FailTask { .. } => Some(still_asking("fail", "failed", question)),
            _ => None,
        }
    }
}

/// What an action decides about the turn's end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Decides {
    Nothing,
    /// A wait folded into whatever wait the turn already recorded.
    MergingWait,
    /// Settles the task, but only parks a long_lived agent on `Inbox` rather
    /// than ending it - legitimate after an earlier park for that agent only.
    Settles,
    /// Starts work or delivers an effect (a spawn, a message, a memory, a
    /// schedule, a merge): refused once the turn is over, so a batched
    /// sibling cannot act after the model was told to stop.
    Effect,
    /// A question: its own guards (asked-once, no pending approval, no
    /// genuine terminal settle) decide whether it merges like a wait.
    Asks,
}

/// Exhaustive on purpose: a new harness action has to say whether it decides
/// how the turn ends, or the engine path will keep serving it after the park.
pub(super) fn decides(action: &HarnessAction) -> Decides {
    match action {
        HarnessAction::WaitFor { .. } => Decides::MergingWait,
        HarnessAction::Delegate { wait, .. } => {
            if *wait {
                Decides::MergingWait
            } else {
                Decides::Effect
            }
        }
        HarnessAction::IntegrateWorkspace { .. }
        | HarnessAction::CreateSchedule { .. }
        | HarnessAction::UpdateSchedule { .. }
        | HarnessAction::SendMessage { .. }
        | HarnessAction::Remember { .. }
        | HarnessAction::Notify { .. }
        | HarnessAction::ResolveNotification { .. } => Decides::Effect,
        HarnessAction::CompleteTask { .. } | HarnessAction::FailTask { .. } => Decides::Settles,
        HarnessAction::AskUser { .. } => Decides::Asks,
        HarnessAction::ListSchedules { .. }
        | HarnessAction::InspectState
        | HarnessAction::ListAgents { .. }
        | HarnessAction::ListPeers { .. }
        | HarnessAction::ListTasks { .. }
        | HarnessAction::ReadTask { .. }
        | HarnessAction::ReadArtifact { .. }
        | HarnessAction::RefreshProviderUsage { .. }
        | HarnessAction::ProviderUsage
        | HarnessAction::ListModels { .. }
        | HarnessAction::CancelAgent { .. }
        | HarnessAction::RequestCheckpoint { .. } => Decides::Nothing,
    }
}

/// Request, gate, run and record the model's tool calls.
pub(super) async fn run_tool_calls(
    runtime: &Arc<Runtime>,
    ctx: &mut TurnCtx,
    tool_ctx: &Arc<ToolContext>,
    invocation_id: InvocationId,
    tool_uses: Vec<(String, String, serde_json::Value)>,
    cancel: &CancellationToken,
) -> Result<ToolEffects> {
    let mut effects = ToolEffects::default();
    let mut runnable: Vec<(ToolCall, Arc<dyn Tool>)> = Vec::new();
    let mut commit = Commit::new();
    let visible: BTreeSet<String> = runtime
        .agent_tool_specs(&ctx.agent)
        .into_iter()
        .map(|s| s.name)
        .collect();

    let rule_ctx = runtime.rule_context(ctx.agent.id)?;
    for (provider_call_id, name, input) in tool_uses {
        let mut call = ToolCall {
            id: ToolCallId::new(),
            provider_call_id,
            name: name.clone(),
            input,
            agent_id: ctx.agent.id,
            task_id: ctx.task.as_ref().map(|t| t.id),
            invocation_id: Some(invocation_id),
            resources: Vec::new(),
            idempotency_key: None,
            requested_at: now(),
        };
        let Some(tool) = runtime.tools.get(&name).filter(|_| visible.contains(&name)) else {
            commit.push_event(ctx.draft(EventKind::ToolRequested {
                call: call.clone(),
                side_effect: aether_core::tool::SideEffectClass::ReadOnly,
            }));
            commit.push_event(ctx.draft(EventKind::ToolFailed {
                call: call.id,
                error: unavailable(runtime, &ctx.agent, &name),
                retryable: false,
                duration_ms: None,
            }));
            continue;
        };
        let descriptor = tool.descriptor();
        aether_tools::ToolRuntime::prepare(&mut call, tool.as_ref(), tool_ctx);
        let class = gated_class(descriptor.side_effect, &call.input);
        commit.push_event(ctx.draft(EventKind::ToolRequested {
            call: call.clone(),
            side_effect: class,
        }));
        let gated = ctx
            .agent
            .config
            .approval
            .requires_raised(descriptor.side_effect, class);
        if gated {
            let argument = tool.rule_argument(&call.input);
            let approval = ApprovalRequest {
                id: ApprovalId::new(),
                agent_id: ctx.agent.id,
                task_id: ctx.task.as_ref().map(|t| t.id),
                call_id: call.id,
                tool: name.clone(),
                description: describe_call(&call),
                argument: argument.clone(),
                resources: call.resources.clone(),
                side_effect: class,
                requested_at: now(),
                status: ApprovalStatus::Pending,
            };
            let subject = approval::subject_of(
                &name,
                argument,
                &call.resources,
                tool.resources_known(&call.input),
            );
            match runtime.gate_call(true, &subject, &rule_ctx)? {
                Gate::Remembered {
                    rule,
                    decision,
                    note,
                } => {
                    approval::remembered_commit(
                        &mut commit,
                        |kind| ctx.draft(kind),
                        &approval,
                        rule,
                        decision,
                        note.clone(),
                    );
                    match decision {
                        ApprovalDecision::Approved => runnable.push((call, tool)),
                        ApprovalDecision::Denied => {
                            commit.push_event(ctx.draft(EventKind::ToolDenied {
                                call: call.id,
                                reason: approval::describe_denial(rule, note.as_deref()),
                            }));
                        }
                    }
                }
                Gate::Ungated | Gate::AskPerson => {
                    commit.push_event(ctx.draft(EventKind::ApprovalRequested {
                        approval: approval.id,
                        call: call.id,
                        tool: name.clone(),
                        description: approval.description.clone(),
                        argument: approval.argument.clone(),
                        side_effect: class,
                    }));
                    commit.push_mutation(Mutation::PutApproval(approval.clone()));
                    effects.pending_approvals.push(approval.id);
                }
            }
            continue;
        }
        runnable.push((call, tool));
    }
    // An outcome already committed for *this call* while driving *this*
    // invocation answers a re-drive of it (a re-drive replays the recorded
    // call, id and all), so a retried attempt never repeats the effect. A
    // later invocation is a live agent asking again: it runs, and so does a
    // second identical call in the same response - it is a second call.
    let mut replays: Vec<(ToolCall, RecordedOutcome)> = Vec::new();
    let mut to_run = Vec::with_capacity(runnable.len());
    for (call, tool) in runnable {
        match replayable(runtime, &call)? {
            Some(recorded) => replays.push((call, recorded)),
            None => to_run.push((call, tool)),
        }
    }
    let runnable = to_run;
    let replayed_any = !replays.is_empty();
    // The requests land first: an outcome must never precede the call it
    // answers in the log (VISION 3.4).
    runtime.apply(commit)?;
    for (call, recorded) in replays {
        ctx.agent.stats.tool_calls += 1;
        runtime.apply(
            Commit::new()
                .event(ctx.draft(EventKind::ToolReplayed {
                    call: call.id,
                    key: recorded.key.clone(),
                    original: recorded.call_id,
                }))
                .event(ctx.draft(EventKind::ToolSucceeded {
                    call: call.id,
                    outcome: recorded.outcome,
                })),
        )?;
    }

    if !runnable.is_empty() {
        // Limit 5 of VISION §3.6: no two agents mutate overlapping resources.
        let guard = match hold_resources(runtime, ctx, &runnable, cancel).await? {
            Held::Keys(guard) => Some(guard),
            Held::NoKeys => None,
            Held::Stopped => {
                // Every call was announced with tool.requested but never
                // started: settle them, or the transcript carries tool_use
                // blocks with no result and a restart misreads the turn.
                let mut settled = Commit::new();
                for (call, _) in &runnable {
                    settled.push_event(ctx.draft(EventKind::ToolCancelled { call: call.id }));
                }
                runtime.apply(settled)?;
                return Ok(effects);
            }
        };
        let mut started = Commit::new();
        for (call, _) in &runnable {
            started.push_event(ctx.draft(EventKind::ToolStarted { call: call.id }));
        }
        runtime.apply(started)?;
        let results = runtime
            .tool_runtime
            .execute_batch(
                runnable,
                tool_ctx.clone(),
                cancel.clone(),
                ctx.hard_cancel.clone(),
                &runtime.config.execution,
            )
            .await;
        release_resources(runtime, ctx, guard)?;
        for result in results {
            let duration = result.duration_ms();
            let ran = result.ran;
            for (i, error) in result.retried.iter().enumerate() {
                runtime.apply(Commit::new().event(ctx.draft(EventKind::ToolRetried {
                    call: result.call.id,
                    attempt: i as u32 + 1,
                    of: result.attempts_allowed,
                    error: error.clone(),
                })))?;
            }
            match result.result {
                Ok(ToolExecution::Outcome(mut outcome)) => {
                    outcome.duration_ms = duration;
                    ctx.agent.stats.tool_calls += 1;
                    let mut commit = Commit::new().event(ctx.draft(EventKind::ToolSucceeded {
                        call: result.call.id,
                        outcome: outcome.clone(),
                    }));
                    if let Some(recorded) = recording_for(&result.call, &outcome) {
                        commit.push_mutation(Mutation::RecordToolOutcome(recorded));
                    }
                    runtime.apply(commit)?;
                }
                Ok(ToolExecution::Action(action))
                    if effects.parked() && decides(&action) == Decides::Effect =>
                {
                    runtime.apply(Commit::new().event(ctx.draft(EventKind::ToolFailed {
                        call: result.call.id,
                        error: crate::turn_engine::TURN_IS_OVER.to_owned(),
                        retryable: false,
                        duration_ms: None,
                    })))?;
                }
                Ok(ToolExecution::Action(action)) => {
                    ctx.agent.stats.tool_calls += 1;
                    let outcome = apply_action(runtime, ctx, action, &mut effects).await?;
                    runtime.apply(Commit::new().event(ctx.draft(EventKind::ToolSucceeded {
                        call: result.call.id,
                        outcome: outcome.with_duration(duration),
                    })))?;
                }
                Err(ToolError::Cancelled) => {
                    runtime.apply(Commit::new().event(ctx.draft(EventKind::ToolCancelled {
                        call: result.call.id,
                    })))?;
                }
                Err(e) => {
                    // A tool's error can quote what it read, like its output.
                    let error = e.to_string();
                    let error = runtime.tool_runtime.redactor().redact(&error);
                    runtime.apply(Commit::new().event(ctx.draft(EventKind::ToolFailed {
                        call: result.call.id,
                        error: error.into_owned(),
                        retryable: e.is_retryable(),
                        duration_ms: ran.then_some(duration),
                    })))?;
                }
            }
        }
        let hold = runtime.hold();
        let row = agent_row(runtime, ctx, &hold)?;
        runtime.apply_held(hold, Commit::new().mutate(Mutation::UpsertAgent(row)))?;
    } else if replayed_any {
        // A turn where every call replayed still made those calls.
        let hold = runtime.hold();
        let row = agent_row(runtime, ctx, &hold)?;
        runtime.apply_held(hold, Commit::new().mutate(Mutation::UpsertAgent(row)))?;
    }
    Ok(effects)
}
