use super::*;

mod inspection;

use crate::workspace::review::path_text as workspace_path_for_reply;

/// `integrate_workspace`: the operator's checks, then the caller's own: the
/// child must be its direct child and the source still within its scope.
async fn integrate_child_workspace(
    runtime: &Arc<Runtime>,
    ctx: &TurnCtx,
    child: AgentId,
) -> Result<ToolOutcome> {
    let Some(target) = runtime.store.agent(child)? else {
        return Ok(ToolOutcome::error(format!(
            "child agent {child} was not found"
        )));
    };
    if target.parent != Some(ctx.agent.id) {
        return Ok(ToolOutcome::error(format!(
            "agent {child} is not your direct child"
        )));
    }
    let managed = match crate::workspace_review::integration_preflight(runtime, &target)? {
        Ok(managed) => managed,
        Err(refusal) => return Ok(ToolOutcome::error(refusal)),
    };
    debug_assert_eq!(managed.manifest.parent, ctx.agent.id);
    let current_parent = runtime.store.agent(ctx.agent.id)?.ok_or_else(|| {
        crate::RuntimeError::Invalid("integrating parent no longer exists".into())
    })?;
    if let Some(field) =
        runtime.scope_widened_for_existing(&target.scope, &target, &current_parent)?
    {
        return Ok(ToolOutcome::error(format!(
            "integration refused: selected source is outside your current `{field}` scope"
        )));
    }
    let outcome = tokio::task::spawn_blocking(move || {
        crate::workspace::integrate(&managed.managed_root, &managed.manifest)
    })
    .await
    .map_err(|e| {
        crate::RuntimeError::Internal(format!("workspace integration task failed: {e}"))
    })?;
    let report = match outcome {
        Ok(crate::workspace::IntegrationOutcome::Landed(report)) => report,
        Ok(crate::workspace::IntegrationOutcome::Conflicts(paths)) => {
            let paths: Vec<_> = paths.iter().map(|p| workspace_path_for_reply(p)).collect();
            return Ok(ToolOutcome::error(format!(
                "integration refused: parent changed {}; resolve them before integrating the child workspace",
                paths.join(", ")
            )));
        }
        Err(error) => {
            return Ok(ToolOutcome::error(format!("integration refused: {error}")));
        }
    };
    let changed_paths: Vec<_> = report
        .changed_paths
        .iter()
        .map(|path| workspace_path_for_reply(path))
        .collect();
    Ok(ToolOutcome::ok(
        serde_json::json!({
            "child_agent_id": child,
            "changed_paths": changed_paths,
            "already_integrated": report.already_integrated,
        })
        .to_string(),
    ))
}

/// Apply a harness action: validate, transition, record. Returns what the
/// model is told.
pub(crate) async fn apply_action(
    runtime: &Arc<Runtime>,
    ctx: &mut TurnCtx,
    action: HarnessAction,
    effects: &mut ToolEffects,
) -> Result<ToolOutcome> {
    match action {
        HarnessAction::ListAgents { after, limit } => {
            crate::inspection::agents(runtime, ctx.agent.id, after, limit)
        }
        HarnessAction::ListPeers { after, limit } => {
            crate::peers::list(runtime, ctx.agent.id, after, limit)
        }
        HarnessAction::ListTasks { after, limit } => {
            crate::inspection::tasks(runtime, ctx.agent.id, after, limit)
        }
        HarnessAction::ReadArtifact { id, offset, limit } => {
            let runtime = runtime.clone();
            let caller = ctx.agent.id;
            tokio::task::spawn_blocking(move || {
                crate::inspection::artifact(&runtime, caller, id, offset, limit)
            })
            .await
            .map_err(|e| crate::RuntimeError::Internal(format!("artifact read task failed: {e}")))?
        }
        HarnessAction::ReadTask {
            task,
            revision,
            offset,
            max_chars,
        } => crate::inspection::task(
            runtime,
            ctx.agent.id,
            task,
            revision.as_deref(),
            offset,
            max_chars,
        ),
        HarnessAction::RefreshProviderUsage { provider } => {
            let data = runtime.refresh_provider_usage(provider.as_deref()).await?;
            Ok(ToolOutcome {
                content: data.to_string(),
                structured: Some(data),
                ..Default::default()
            })
        }
        HarnessAction::ProviderUsage => {
            let data = runtime.provider_usage();
            Ok(ToolOutcome {
                content: data.to_string(),
                structured: Some(data),
                ..Default::default()
            })
        }
        HarnessAction::InspectState => inspection::inspect_state(runtime, ctx),
        HarnessAction::ListModels { provider } => {
            inspection::list_models(runtime, ctx, provider).await
        }
        HarnessAction::Delegate {
            role,
            name,
            objective,
            instructions,
            model,
            effort,
            scope,
            workspace_mode,
            wait,
            leaf,
            host_context,
        } => {
            let parent = runtime.store.agent(ctx.agent.id)?.ok_or_else(|| {
                crate::RuntimeError::Invalid("Delegating parent no longer exists".into())
            })?;
            if parent.config.leaf {
                return Ok(ToolOutcome::error(
                    "Leaf agents cannot delegate children: do the work yourself, or ask your parent to staff it",
                ));
            }
            let requested = model.clone();
            let network = scope
                .as_ref()
                .map_or(parent.scope.network, |scope| scope.network);
            let resolved = match runtime.models.resolve(&requested) {
                Ok(resolved) => resolved,
                Err(error) => {
                    return delegation_refusal(runtime, &requested, network, &parent.model, error)
                        .await;
                }
            };
            if let Some(kind) = crate::turn_engine::engine_kind(resolved.provider.protocol) {
                let Some(engine) = runtime.engines.get(&kind) else {
                    return delegation_refusal(
                        runtime,
                        &requested,
                        network,
                        &parent.model,
                        "its engine is not available in this daemon",
                    )
                    .await;
                };
                if let Err(error) = engine
                    .validate_network_policy(network == aether_core::agent::NetworkPolicy::Allowed)
                {
                    return delegation_refusal(runtime, &requested, network, &parent.model, error)
                        .await;
                }
            }
            if runtime.client_override.is_none() {
                let status = runtime.models.auth_status(&requested.provider).await;
                if !status.configured {
                    return delegation_refusal(
                        runtime,
                        &requested,
                        network,
                        &parent.model,
                        format!(
                            "provider authentication is not configured. {}",
                            status.error.unwrap_or_default()
                        ),
                    )
                    .await;
                }
            }
            let mut isolated = None;
            let child_scope = if workspace_mode == aether_tools::WorkspaceMode::Isolated {
                let requested = scope.clone().unwrap_or_else(|| parent.scope.clone());
                if requested.backend != aether_core::agent::ExecutionBackend::Host {
                    return Ok(ToolOutcome::error(
                        "isolated delegation requires the host execution backend: Git worktree metadata lives outside a container's workspace mount",
                    ));
                }
                if let Some(field) = runtime.widened_field_resolved(&requested, &parent.scope)? {
                    return Ok(ToolOutcome::error(format!(
                        "delegation refused: child scope widens `{field}`"
                    )));
                }
                let Some(source) = requested.workspace_root.as_ref() else {
                    return Ok(ToolOutcome::error(
                        "isolated delegation requires a Git workspace root; set workspace_root to its repository root",
                    ));
                };
                let Some(parent_root) = parent.scope.workspace_root.as_ref() else {
                    return Ok(ToolOutcome::error(
                        "isolated delegation requires a parent workspace root",
                    ));
                };
                let (canonical_source, canonical_parent) = match (
                    std::fs::canonicalize(source),
                    std::fs::canonicalize(parent_root),
                ) {
                    (Ok(source), Ok(parent)) => (source, parent),
                    (Err(error), _) => {
                        return Ok(ToolOutcome::error(format!(
                            "isolated workspace source {} is unavailable: {error}",
                            source.display()
                        )));
                    }
                    (_, Err(error)) => {
                        return Ok(ToolOutcome::error(format!(
                            "parent workspace {} is unavailable: {error}",
                            parent_root.display()
                        )));
                    }
                };
                if !canonical_source.starts_with(&canonical_parent) {
                    return Ok(ToolOutcome::error(format!(
                        "isolated workspace source {} resolves outside parent workspace {}",
                        source.display(),
                        parent_root.display()
                    )));
                }
                let Some(managed_root) = runtime.config.delegated_workspace_root.as_ref() else {
                    return Ok(ToolOutcome::error(
                        "isolated delegation is unavailable: the daemon has no managed workspace directory configured",
                    ));
                };
                let mut relative_allowed_paths = Vec::new();
                for path in &requested.allowed_paths {
                    let Ok(relative) = path.strip_prefix(source) else {
                        return Ok(ToolOutcome::error(format!(
                            "isolated delegation cannot inherit allowed path {} outside the selected Git root {}; narrow allowed_paths first",
                            path.display(),
                            source.display()
                        )));
                    };
                    relative_allowed_paths.push(relative.to_path_buf());
                }
                let (managed_root, source, parent_id) =
                    (managed_root.clone(), canonical_source, parent.id);
                let manifest = match tokio::task::spawn_blocking(move || {
                    crate::workspace::create(&managed_root, &source, parent_id)
                })
                .await
                .map_err(|e| {
                    crate::RuntimeError::Internal(format!("workspace snapshot task failed: {e}"))
                })? {
                    Ok(manifest) => manifest,
                    Err(error) => {
                        return Ok(ToolOutcome::error(format!("delegation refused: {error}")));
                    }
                };
                let mut isolated_scope = requested;
                isolated_scope.allowed_paths = relative_allowed_paths
                    .iter()
                    .map(|path| manifest.workspace.join(path))
                    .collect();
                isolated_scope.workspace_root = Some(manifest.workspace.clone());
                isolated = Some(manifest);
                Some(isolated_scope)
            } else {
                scope
            };
            let mut config = ctx.agent.config.clone();
            // None at create_agent means inherit. Delegation instructions
            // belong to this task; inheriting "spawn a child" recursively
            // makes each leaf repeat its parent's orchestration work.
            let mut child_instructions = instructions.unwrap_or_default();
            if isolated.is_some() {
                if !child_instructions.is_empty() {
                    child_instructions.push_str("\n\n");
                }
                child_instructions.push_str(
                    "You are working in a managed isolated Git worktree copied from your parent's current workspace. Keep edits in this workspace. Your parent will deliberately integrate your changes after you finish; completion or failure never copies them back automatically. If you delegate isolated children, integrate only the results you accept; deferred or rejected results remain in their worktrees.",
                );
            }
            config.instructions = Some(child_instructions);
            config.effort = Some(effort);
            config.long_lived = false;
            config.leaf = leaf;
            config.max_turns = None;
            if let Some(host_context) = host_context {
                config.host_context = host_context;
            }
            let spec = CreateAgentSpec {
                role,
                name,
                objective: Some(objective),
                model: Some(model),
                // Delegation always names its model; do not inherit the parent's chain.
                fallbacks: Vec::new(),
                scope: child_scope,
                // A delegated child inherits its parent's repository.
                repository_root: None,
                parent: Some(ctx.agent.id),
                config: Some(config),
                // Inherited from the parent by the runtime; a delegating
                // agent does not get to choose its child's gates.
                approval: None,
                run: Some(ctx.agent.run),
                start: true,
            };
            match runtime.create_agent_with_preflight(
                spec,
                ctx.base_actor.clone(),
                crate::commands::Preflight {
                    checked: Some((&requested, network)),
                    run_budget: true,
                    isolated_workspace: isolated
                        .as_ref()
                        .map(|manifest| manifest.workspace.as_path()),
                },
                // A delegating agent submits once per turn; nothing retries
                // this for it.
                None,
            ) {
                Ok((child, task)) => {
                    let task_id = task.as_ref().map(|t| t.id);
                    if wait && let Some(id) = task_id {
                        merge_wait(effects, WaitCondition::Task { id });
                    }
                    // Advisory to the caller: descendants at any depth on
                    // the provider against the effective cap - exactly what
                    // the scheduler enforces (VISION §3.6 limit 3), so the
                    // model is told whether the child it just created starts
                    // now or queues. Never `?`: this read cannot fail the
                    // turn on a delegation that already committed.
                    let status = cap_status(
                        runtime,
                        &ctx.agent,
                        crate::turn_engine::engine_kind(resolved.provider.protocol).is_some(),
                    );
                    let (cap_note, structured) = match &status {
                        CapStatus::Counted {
                            running,
                            cap,
                            source,
                            queued,
                        } => {
                            let note = if *queued {
                                format!(
                                    " {running}/{cap} turns already running against {source}; \
                                     it queues until a slot frees."
                                )
                            } else {
                                format!(" {running}/{cap} turns running against {source}.")
                            };
                            let structured = serde_json::json!({
                                "concurrency_cap": cap,
                                "cap_source": source,
                                "descendants_running": running,
                                "queued": queued,
                            });
                            (note, structured)
                        }
                        CapStatus::Unknown { cap, source, error } => {
                            let note = format!(
                                " {source} is {cap}, but the live count against it could not be \
                                 read ({error})."
                            );
                            let structured = serde_json::json!({
                                "concurrency_cap": cap,
                                "cap_source": source,
                                "descendants_running": null,
                                "queued": null,
                            });
                            (note, structured)
                        }
                    };
                    let mut outcome = ToolOutcome::ok(format!(
                        "Delegated to agent {} ({}) with task {}.{}{}",
                        child.id,
                        child.name,
                        task_id.map(|t| t.to_string()).unwrap_or_default(),
                        if wait {
                            format!(" {TURN_ENDS_HERE} You will be woken when it finishes.")
                        } else {
                            String::new()
                        },
                        cap_note,
                    ));
                    if let Some(manifest) = &isolated {
                        outcome.content.push_str(&format!(
                            " Isolated workspace: {}. Changes remain there until you call integrate_workspace with child_agent_id={}; integration checks conflicts and never runs automatically.",
                            manifest.workspace.display(), child.id
                        ));
                    }
                    outcome.structured = Some({
                        let mut s = structured;
                        s["agent_id"] = serde_json::json!(child.id);
                        s["task_id"] = serde_json::json!(task_id);
                        s["workspace_mode"] = serde_json::json!(if isolated.is_some() {
                            "isolated"
                        } else {
                            "shared"
                        });
                        s["workspace_root"] = serde_json::json!(
                            child
                                .scope
                                .workspace_root
                                .as_deref()
                                .map(workspace_path_for_reply)
                        );
                        s["wait_for_args"] =
                            serde_json::json!({"tasks": task_id.into_iter().collect::<Vec<_>>()});
                        // Engine replies carry content, not the structured field.
                        // Keep the documented IDs and copyable wait arguments visible.
                        outcome.content.push('\n');
                        outcome.content.push_str(&s.to_string());
                        s
                    });
                    Ok(outcome)
                }
                Err(e) => {
                    if let Some(manifest) = isolated {
                        let managed_root = runtime
                            .config
                            .delegated_workspace_root
                            .as_deref()
                            .ok_or_else(|| {
                                crate::RuntimeError::Internal(
                                    "managed workspace root disappeared during delegation".into(),
                                )
                            })?;
                        match crate::workspace::discard(managed_root, &manifest) {
                            Ok(()) => Ok(ToolOutcome::error(format!("delegation refused: {e}"))),
                            Err(cleanup) => Ok(ToolOutcome::error(format!(
                                "delegation refused: {e}; unused workspace {} could not be removed: {cleanup}",
                                manifest.workspace.display()
                            ))),
                        }
                    } else {
                        Ok(ToolOutcome::error(format!("delegation refused: {e}")))
                    }
                }
            }
        }
        HarnessAction::IntegrateWorkspace { child } => {
            integrate_child_workspace(runtime, ctx, child).await
        }
        HarnessAction::CreateSchedule { spec } => {
            match runtime.create_schedule(spec, ctx.base_actor.clone()) {
                Ok(job) => Ok(ToolOutcome::ok(
                    serde_json::json!({"schedule":job}).to_string(),
                )),
                Err(error) => Ok(ToolOutcome::error(format!(
                    "Cannot create schedule: {error}"
                ))),
            }
        }
        HarnessAction::ListSchedules { id } => {
            let visible = |job: &aether_core::schedule::Schedule| {
                job.owner == ctx.base_actor || job.target() == Some(ctx.agent.id)
            };
            if let Some(id) = id {
                let Some(job) = runtime.store.schedule(id)?.filter(visible) else {
                    return Ok(ToolOutcome::error(
                        "Schedule not found among timers you own or that target you.",
                    ));
                };
                let history = runtime.store.schedule_history(id, 100)?;
                Ok(ToolOutcome::ok(
                    serde_json::json!({"schedule":job,"history":history}).to_string(),
                ))
            } else {
                let jobs = runtime
                    .store
                    .schedules(None)?
                    .into_iter()
                    .filter(visible)
                    .collect::<Vec<_>>();
                Ok(ToolOutcome::ok(
                    serde_json::json!({"schedules":jobs}).to_string(),
                ))
            }
        }
        HarnessAction::UpdateSchedule { id, action } => {
            match runtime.update_schedule(id, action, ctx.base_actor.clone()) {
                Ok(job) => Ok(ToolOutcome::ok(
                    serde_json::json!({"schedule":job}).to_string(),
                )),
                Err(error) => Ok(ToolOutcome::error(format!(
                    "Cannot update schedule: {error}"
                ))),
            }
        }
        HarnessAction::CancelAgent { agent, reason } => {
            let fleet = runtime.store.agents(&AgentFilter::default())?;
            if agent == ctx.agent.id
                || !aether_core::agent::subtree_of(&fleet, ctx.agent.id).contains(&agent)
            {
                return Ok(ToolOutcome::error(
                    "You may only cancel your delegated descendants. Use the descendant agent ID returned by delegation; complete_task or fail_task ends your own task.",
                ));
            }
            match runtime.cancel_agent(agent, Some(reason), ctx.base_actor.clone()) {
                Ok(()) => Ok(ToolOutcome::ok(format!(
                    "Cancellation recorded for {agent} and its descendants. Running turns stop cooperatively."
                ))),
                Err(error) => Ok(ToolOutcome::error(format!(
                    "Could not finish cancellation: {error}"
                ))),
            }
        }
        HarnessAction::SendMessage {
            to,
            content,
            subtree,
        } => {
            if let Some(refusal) =
                crate::peers::message_reach_refusal(runtime, &ctx.agent, to, subtree)?
            {
                return Ok(ToolOutcome::error(refusal));
            }
            let blocks = vec![ContentBlock::text(content)];
            if subtree {
                return match runtime.fan_out_message(to, blocks, ctx.base_actor.clone()) {
                    Ok(deliveries) => Ok(ToolOutcome::ok(format!(
                        "Delivered the message to {} live agent(s) in {to}'s subtree.",
                        deliveries.len()
                    ))),
                    Err(e) => Ok(ToolOutcome::error(format!("could not deliver: {e}"))),
                };
            }
            match runtime.send_message(to, blocks, ctx.base_actor.clone()) {
                Ok(id) => Ok(ToolOutcome::ok(format!("Delivered message {id} to {to}."))),
                Err(e) => Ok(ToolOutcome::error(format!("could not deliver: {e}"))),
            }
        }
        HarnessAction::CompleteTask {
            summary,
            artifacts,
            structured,
        } => {
            if let Some(id) = first_artifact_outside_scope(runtime, ctx.agent.id, &artifacts)? {
                return Ok(ToolOutcome::error(format!(
                    "complete_task.artifacts contains artifact `{id}` you did not produce: only \
                     artifacts from you or your descendants can be attached. Use the artifact id \
                     returned when it was stored."
                )));
            }
            let hold = runtime.hold();
            let mut task = match ctx.task.as_ref() {
                // `structured.task_less` lets a client mark this
                // informational row distinctly instead of a bare green
                // "succeeded" (the outcome is genuinely not an error).
                None => {
                    return Ok(ToolOutcome {
                        structured: Some(serde_json::json!({"task_less": true})),
                        ..ToolOutcome::ok(NO_ACTIVE_TASK_TO_COMPLETE)
                    });
                }
                Some(task) => runtime
                    .store
                    .task(task.id)?
                    .ok_or_else(|| RuntimeError::NotFound(task.id.to_string()))?,
            };
            if task.status.is_terminal() {
                return Ok(ToolOutcome::error(format!(
                    "Task {} is already {} and cannot be completed again.",
                    task.id,
                    task.status.kind()
                )));
            }
            // Only the path that really settles the task is refused: the two
            // branches above end neither the task nor the agent. A
            // long_lived agent's settle parks it on Inbox rather than ending
            // it, so the question's own park still holds - nothing to refuse.
            if let Some(question) = &effects.asked
                && !ctx.agent.config.long_lived
            {
                return Ok(ToolOutcome::error(still_asking(
                    "complete",
                    "completed",
                    question,
                )));
            }
            let result = TaskResult {
                summary,
                artifacts,
                structured,
            };
            mark_completed(&mut task, result.clone(), now())?;
            let mut commit = Commit::new()
                .event(ctx.draft(EventKind::TaskCompleted {
                    result: result.clone(),
                }))
                .mutate(Mutation::UpsertTask(task.clone()));
            runtime.notify_task_creator(
                &hold,
                &mut commit,
                &task,
                InboxKind::TaskCompleted {
                    task: task.id,
                    result: result.clone(),
                },
                ctx.base_actor.clone(),
            )?;
            runtime.apply_held(hold, commit)?;
            ctx.task = Some(task.clone());
            if ctx.agent.config.long_lived {
                // Merged, not assigned: an ask_user earlier in this batch may
                // already have recorded the same Inbox wait.
                merge_wait(effects, WaitCondition::Inbox);
            } else {
                effects.next = Some(Next::Completed(Some(result.summary.clone())));
            }
            Ok(ToolOutcome::ok(format!(
                "Task {} completed. {TURN_ENDS_HERE}",
                task.id
            )))
        }
        HarnessAction::FailTask { error, structured } => {
            let hold = runtime.hold();
            let mut task = match ctx.task.as_ref() {
                None => {
                    return Ok(ToolOutcome {
                        structured: Some(serde_json::json!({"task_less": true})),
                        ..ToolOutcome::ok(NO_ACTIVE_TASK_TO_FAIL)
                    });
                }
                Some(task) => runtime
                    .store
                    .task(task.id)?
                    .ok_or_else(|| RuntimeError::NotFound(task.id.to_string()))?,
            };
            if task.status.is_terminal() {
                return Ok(ToolOutcome::error(format!(
                    "Task {} is already {} and cannot be failed again.",
                    task.id,
                    task.status.kind()
                )));
            }
            if let Some(question) = &effects.asked
                && !ctx.agent.config.long_lived
            {
                return Ok(ToolOutcome::error(still_asking("fail", "failed", question)));
            }
            task.transition(
                TaskStatus::Failed {
                    error: error.clone(),
                    structured: structured.clone(),
                },
                now(),
            )?;
            let mut commit = Commit::new()
                .event(ctx.draft(EventKind::TaskFailed {
                    error: error.clone(),
                    structured: structured.clone(),
                }))
                .mutate(Mutation::UpsertTask(task.clone()));
            runtime.notify_task_creator(
                &hold,
                &mut commit,
                &task,
                InboxKind::TaskFailed {
                    task: task.id,
                    error: error.clone(),
                    structured,
                },
                ctx.base_actor.clone(),
            )?;
            runtime.apply_held(hold, commit)?;
            ctx.task = Some(task.clone());
            if ctx.agent.config.long_lived {
                merge_wait(effects, WaitCondition::Inbox);
            } else {
                effects.next = Some(Next::Failed(error));
            }
            Ok(ToolOutcome::ok(format!(
                "Task {} marked failed. {TURN_ENDS_HERE}",
                task.id
            )))
        }
        HarnessAction::WaitFor { condition } => {
            if let Some(error) = validate_wait(runtime, ctx, &condition, false)? {
                return Ok(error);
            }
            merge_wait(effects, condition);
            Ok(ToolOutcome::ok(format!(
                "Waiting. {TURN_ENDS_HERE} You will be woken with the results when the \
                 condition is met."
            )))
        }
        HarnessAction::Remember { mut candidate } => {
            if let MemoryScope::Role { role } = &candidate.scope
                && role == "unknown"
            {
                candidate.scope = MemoryScope::Role {
                    role: ctx.agent.role.as_str().to_owned(),
                };
            }
            candidate.provenance.agent = Some(ctx.agent.id);
            candidate.provenance.task = ctx.task.as_ref().map(|t| t.id);
            let hold = runtime.hold();
            let (admission, commit) = runtime.memory.evaluate(
                &hold,
                candidate,
                ctx.base_actor.clone(),
                Some(ctx.agent.id),
                ctx.task.as_ref().map(|t| t.id),
            )?;
            runtime.apply_held(hold, commit)?;
            Ok(match admission {
                aether_memory::Admission::Accepted { record } => {
                    ToolOutcome::ok(format!("Remembered as {}.", record.id))
                }
                aether_memory::Admission::Superseded { record, old } => {
                    ToolOutcome::ok(format!("Remembered as {} (supersedes {old}).", record.id))
                }
                aether_memory::Admission::Rejected { reason } => {
                    ToolOutcome::ok(format!("Not stored: {reason}."))
                }
            })
        }
        HarnessAction::RequestCheckpoint { reason } => {
            let hold = runtime.hold();
            if matches!(
                compaction_pending(runtime, ctx.agent.id)?,
                Some(CompactionTrigger::Handoff { .. })
            ) {
                return Ok(ToolOutcome::ok(format!(
                    "Checkpoint already requested for the model handoff ({reason})."
                )));
            }
            runtime.apply_held(
                hold,
                Commit::new().event(ctx.draft(EventKind::CompactionRequested {
                    trigger: CompactionTrigger::Explicit {
                        by: ctx.base_actor.clone(),
                    },
                })),
            )?;
            Ok(ToolOutcome::ok(format!(
                "Checkpoint requested ({reason}). It will be built before your next turn."
            )))
        }
        HarnessAction::AskUser { question } => {
            // Honest only while someone waits on the answer: never recorded
            // once the turn ends the agent, and never twice in a turn.
            if let Some(asked) = &effects.asked {
                return Ok(ToolOutcome::error(format!(
                    "You already asked the operator \"{}\" in this turn, and only one \
                     question can be waited on. {TURN_ENDS_HERE} Ask anything further \
                     after they reply.",
                    one_line(asked)
                )));
            }
            if let Some(Next::Completed(_) | Next::Failed(_)) = &effects.next {
                return Ok(ToolOutcome::error(format!(
                    "This turn already ended the agent, so nothing would be left waiting \
                     for an answer and the operator's reply would be refused. {TURN_ENDS_HERE} \
                     Put what they need to know in the summary instead, or ask before you \
                     finish."
                )));
            }
            // An approval queued anywhere in this batch replaces the turn's
            // wait, and an approval wait is never interrupted by the reply.
            if !effects.pending_approvals.is_empty() {
                return Ok(ToolOutcome::error(format!(
                    "Another call in this turn is waiting for the operator to approve it, \
                     and that wait would swallow your question: you would not be woken by \
                     their answer. {TURN_ENDS_HERE} Ask again once the approval is settled."
                )));
            }
            runtime.apply(Commit::new().event(ctx.draft(EventKind::QuestionAsked {
                question: question.clone(),
            })))?;
            effects.asked = Some(question.clone());
            // Merged, not assigned: a wait this turn already recorded (say a
            // `delegate` it is waiting on) still has to be honoured.
            merge_wait(effects, WaitCondition::Inbox);
            Ok(ToolOutcome::ok(format!(
                "Question recorded for the operator to see. {TURN_ENDS_HERE} You will be \
                 woken when they reply."
            )))
        }
    }
}
