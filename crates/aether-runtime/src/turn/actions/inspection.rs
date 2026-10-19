use super::*;

fn inspect_state(runtime: &Arc<Runtime>, ctx: &TurnCtx) -> Result<ToolOutcome> {
    let resolved = runtime.models.resolve(&ctx.agent.model)?;
    let agent = runtime
        .store
        .agent(ctx.agent.id)?
        .unwrap_or_else(|| ctx.agent.clone());
    let window = resolved.model.capabilities.context_window;
    // The link actually carrying this turn, not the configured
    // primary: a chain that crosses the engine boundary would
    // otherwise disagree with `engine_turns_running` beside it.
    let (engine_turns_running, engine_backed) = {
        let sched = runtime.sched.lock();
        let in_flight = sched.running.get(&ctx.agent.id).map(|r| r.engine);
        (
            sched.on_engine() as u32,
            in_flight.unwrap_or_else(|| {
                crate::turn_engine::engine_kind(resolved.provider.protocol).is_some()
            }),
        )
    };
    let run_spend = aether_core::budget::run_usage(&runtime.store.agents(&AgentFilter {
        run: Some(agent.run),
        ..Default::default()
    })?);
    let mut subtree_spend = aether_core::model::Usage::default();
    for id in runtime.store.subtree_ids(agent.id)? {
        if let Some(member) = runtime.store.agent(id)? {
            subtree_spend += member.stats.usage;
        }
    }
    let used_percent = agent
        .stats
        .context_tokens
        .filter(|_| window > 0)
        .map(|used| used as f64 / f64::from(window) * 100.0);
    let data = serde_json::json!({
        "agent": agent.id,
        "parent": agent.parent,
        "model": agent.model,
        "effort": agent.config.effort,
        "task": ctx.task,
        "context": {"tokens": agent.stats.context_tokens, "window": window, "used_percent": used_percent, "at_compaction_threshold": used_percent.map(|percent| percent >= 75.0)},
        "remaining_token_budget": agent.remaining_budget(),
        "cost_budget": agent.config.cost_budget,
        "subtree_cost_spent_usd": subtree_spend.cost_usd,
        "subtree_cost_is_a_floor": !subtree_spend.cost_complete,
        "max_turns": agent.config.max_turns,
        "turns_taken": agent.stats.turns,
        "deadline": agent.config.deadline.map(|at| at.to_rfc3339()),
        "concurrency": {
            "global_max_concurrent_turns": runtime.config.max_concurrent_turns,
            "own_subtree_max_concurrent_turns": agent.config.concurrency,
            "max_concurrent_engine_turns": runtime.config.max_concurrent_engine_turns,
            "engine_turns_running": engine_turns_running,
            "own_model_is_an_engine": engine_backed,
            "run_budget_usd": runtime.config.run_budget_usd,
            "run_spent_usd": run_spend.cost_usd,
            "run_spend_is_a_floor": !run_spend.cost_complete
        },
        "compaction": {
            "model": runtime.models.compactor_for(&agent.model),
            "effort": runtime.models.config().model.compactor_effort.unwrap_or(aether_core::model::Effort::Medium),
            "pressure_threshold_percent": 75,
            "manual_minimum": "Engine: any nonempty recorded transcript. API: an older complete turn beyond the six-entry retained tail.",
            "task_boundary": "Long-lived agents after at least six new transcript entries.",
            "checkpoint": agent.checkpoint
        }
    });
    Ok(ToolOutcome {
        content: data.to_string(),
        structured: Some(data),
        ..Default::default()
    })
}

async fn list_models(
    runtime: &Arc<Runtime>,
    ctx: &TurnCtx,
    provider: Option<String>,
) -> Result<ToolOutcome> {
    let network = runtime
        .store
        .agent(ctx.agent.id)?
        .map_or(ctx.agent.scope.network, |agent| agent.scope.network);
    let catalog = runtime.models.list();
    let names: std::collections::BTreeSet<_> = catalog
        .iter()
        .filter(|info| provider.as_ref().is_none_or(|p| *p == info.model.provider))
        .map(|info| info.name.clone())
        .collect();
    let mut statuses = std::collections::BTreeMap::new();
    let mut models = Vec::new();
    let mut alternatives = Vec::new();
    for info in catalog
        .into_iter()
        .filter(|info| names.contains(&info.name))
    {
        if !statuses.contains_key(&info.model.provider) {
            let status = runtime.models.auth_status(&info.model.provider).await;
            let resolved = runtime.models.resolve(&info.model)?;
            let engine_error = crate::turn_engine::engine_kind(resolved.provider.protocol)
                .and_then(|kind| match runtime.engines.get(&kind) {
                    None => Some("Provider engine is not available in this daemon".to_owned()),
                    Some(engine) => engine
                        .validate_network_policy(
                            network == aether_core::agent::NetworkPolicy::Allowed,
                        )
                        .err()
                        .map(|error| error.to_string()),
                });
            let engine_available = engine_error.is_none();
            let error = status.error.or(engine_error);
            statuses.insert(
                info.model.provider.clone(),
                serde_json::json!({"configured": status.configured,
                    "available": status.configured && engine_available, "error": error}),
            );
        }
        let configured = statuses[&info.model.provider]["configured"].as_bool() == Some(true);
        let available = statuses[&info.model.provider]["available"].as_bool() == Some(true);
        let selected = provider.as_ref().is_none_or(|p| *p == info.model.provider);
        let resolved = runtime.models.resolve(&info.model)?;
        let cost = resolved
            .model
            .capabilities
            .cost
            .or_else(|| runtime.models.registry().api_equivalent_price(&resolved));
        let entry = serde_json::json!({"model": info.model, "name": info.name, "provider_name": info.provider_name, "configured": configured, "available": available, "context_window": info.capabilities.context_window, "effort_supported": info.capabilities.effort, "tier": info.tier, "api_equivalent_cost": cost});
        if selected {
            models.push(entry);
        } else if available {
            alternatives.push(entry);
        }
    }
    if models.is_empty() {
        return Ok(ToolOutcome::error(
            "No models matched. Use list_models without a provider to discover enabled providers.",
        ));
    }
    let data = serde_json::json!({"providers": statuses, "models": models, "configured_alternatives": alternatives, "current_model": ctx.agent.model,
        "guidance": "Choose a connection with available=true. configured reports credentials; available also checks the engine and your current network policy. Inspect configured_alternatives before declaring a model family unavailable. Each model has a tier (fast, balanced, strong, frontier; null when unranked), a capability band within its provider; a provider may lack a tier. Prefer capability first, then cost within a suitable tier: frontier or strong for the hardest work, balanced for demanding work, fast for focused, low-cost work. Select by tier, never by model name. Respect explicit model choices and budgets. API-equivalent prices are not subscription charges. Reasoning with tools on an OpenAI model requires the openai-responses provider on the API path."});
    Ok(ToolOutcome {
        content: data.to_string(),
        structured: Some(data),
        ..Default::default()
    })
}

async fn read_artifact(
    runtime: &Arc<Runtime>,
    ctx: &TurnCtx,
    id: ArtifactId,
    offset: u64,
    limit: usize,
) -> Result<ToolOutcome> {
    let runtime = runtime.clone();
    let caller = ctx.agent.id;
    tokio::task::spawn_blocking(move || {
        crate::inspection::artifact(&runtime, caller, id, offset, limit)
    })
    .await
    .map_err(|e| crate::RuntimeError::Internal(format!("artifact read task failed: {e}")))?
}

async fn refresh_provider_usage(
    runtime: &Arc<Runtime>,
    provider: Option<String>,
) -> Result<ToolOutcome> {
    let data = runtime.refresh_provider_usage(provider.as_deref()).await?;
    Ok(usage_outcome(data))
}

fn provider_usage(runtime: &Arc<Runtime>) -> ToolOutcome {
    usage_outcome(runtime.provider_usage())
}

fn usage_outcome(data: serde_json::Value) -> ToolOutcome {
    ToolOutcome {
        content: data.to_string(),
        structured: Some(data),
        ..Default::default()
    }
}

/// Read-only listing and inspection actions.
pub(super) async fn apply(
    runtime: &Arc<Runtime>,
    ctx: &TurnCtx,
    action: HarnessAction,
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
            read_artifact(runtime, ctx, id, offset, limit).await
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
            refresh_provider_usage(runtime, provider).await
        }
        HarnessAction::ProviderUsage => Ok(provider_usage(runtime)),
        HarnessAction::InspectState => inspect_state(runtime, ctx),
        HarnessAction::ListModels { provider } => list_models(runtime, ctx, provider).await,
        other => unreachable!("not an inspection action: {other:?}"),
    }
}
