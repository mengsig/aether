use super::*;
use crate::workspace::WorkspaceManifest;
use aether_core::agent::{NetworkPolicy, Scope};
use aether_core::idempotency::Submission;
use aether_core::task::Task;
use std::path::{Path, PathBuf};

struct DelegateRequest {
    role: String,
    name: Option<String>,
    objective: String,
    instructions: Option<String>,
    model: ModelRef,
    effort: aether_core::model::Effort,
    scope: Option<Scope>,
    workspace_mode: aether_tools::WorkspaceMode,
    wait: bool,
    leaf: bool,
    host_context: Option<aether_core::agent::HostContext>,
    idempotency: Option<aether_tools::DelegateKey>,
}

/// A refusal is a normal tool outcome; only infrastructure faults are `Err`.
type Checked<T> = Result<std::result::Result<T, ToolOutcome>>;

struct IsolatedSource {
    canonical_source: PathBuf,
    managed_root: PathBuf,
    relative_allowed_paths: Vec<PathBuf>,
}

const ISOLATED_INSTRUCTIONS: &str = "You are working in a managed isolated Git worktree copied from your parent's current workspace. Keep edits in this workspace. Your parent will deliberately integrate your changes after you finish; completion or failure never copies them back automatically. If you delegate isolated children, integrate only the results you accept; deferred or rejected results remain in their worktrees.";

/// `integrate_workspace`: the operator's checks, then the caller's own: the
/// child must be its direct child and the source still within its scope.
pub(super) async fn integrate_child_workspace(
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
    Ok(integration_reply(child, &report))
}

fn integration_reply(child: AgentId, report: &crate::workspace::IntegrationReport) -> ToolOutcome {
    let changed_paths: Vec<_> = report
        .changed_paths
        .iter()
        .map(|path| workspace_path_for_reply(path))
        .collect();
    ToolOutcome::ok(
        serde_json::json!({
            "child_agent_id": child,
            "changed_paths": changed_paths,
            "already_integrated": report.already_integrated,
        })
        .to_string(),
    )
}

async fn delegate(
    runtime: &Arc<Runtime>,
    ctx: &TurnCtx,
    effects: &mut ToolEffects,
    request: DelegateRequest,
) -> Result<ToolOutcome> {
    let parent = match load_delegating_parent(runtime, ctx)? {
        Ok(parent) => parent,
        Err(refusal) => return Ok(refusal),
    };
    let submission = match delegate_submission(&parent, &request) {
        Ok(submission) => submission,
        Err(refusal) => return Ok(refusal),
    };
    if let (Some(submission), Some(keyed)) = (&submission, &request.idempotency) {
        match runtime.replay_created_agent(submission, ctx.base_actor.clone()) {
            Ok(Some((child, created))) => {
                // The receipt is the task as created; whether to wait needs it as it is now.
                let task = match created {
                    Some(task) => runtime.store.task(task.id)?,
                    None => None,
                };
                return Ok(replayed_outcome(
                    effects,
                    &child,
                    task,
                    request.wait,
                    &keyed.key,
                ));
            }
            Ok(None) => {}
            // A key reused for a different request: the caller's mistake.
            Err(RuntimeError::Invalid(conflict)) => {
                return Ok(ToolOutcome::error(format!(
                    "delegation refused: {conflict}"
                )));
            }
            Err(fault) => return Err(fault),
        }
    }
    let network = request
        .scope
        .as_ref()
        .map_or(parent.scope.network, |scope| scope.network);
    let child_on_engine =
        match check_delegation_target(runtime, &request.model, network, &parent).await? {
            Ok(on_engine) => on_engine,
            Err(refusal) => return Ok(refusal),
        };
    let (child_scope, isolated) = match prepare_child_scope(runtime, &parent, &request).await? {
        Ok(prepared) => prepared,
        Err(refusal) => return Ok(refusal),
    };
    let wait = request.wait;
    let key = request.idempotency.as_ref().map(|keyed| keyed.key.clone());
    let requested = request.model.clone();
    let spec = child_spec(ctx, request, child_scope, isolated.is_some());
    let created = runtime.create_agent_with_preflight(
        spec,
        ctx.base_actor.clone(),
        crate::commands::Preflight {
            checked: Some((&requested, network)),
            run_budget: true,
            isolated_workspace: isolated
                .as_ref()
                .map(|manifest| manifest.workspace.as_path()),
        },
        submission,
    );
    match created {
        Ok((child, task)) => {
            // Advisory (VISION §3.6 limit 3): never `?`, so a read failure
            // cannot fail the turn on a delegation that already committed.
            let status = cap_status(runtime, &ctx.agent, child_on_engine);
            Ok(delegated_outcome(
                effects,
                &child,
                Delegated {
                    task_id: task.as_ref().map(|t| t.id),
                    wait,
                    key: key.as_deref(),
                },
                isolated.as_ref(),
                &status,
            ))
        }
        Err(e) => Ok(refuse_delegation(runtime, isolated, e)?),
    }
}

/// The submission a keyed delegate call is recorded under: scoped to the
/// calling agent, fingerprinted over the arguments the caller sent.
fn delegate_submission(
    parent: &Agent,
    request: &DelegateRequest,
) -> std::result::Result<Option<Submission>, ToolOutcome> {
    let Some(keyed) = &request.idempotency else {
        return Ok(None);
    };
    let scoped = format!("delegate:{}:{}", parent.id, keyed.key);
    Submission::new(&scoped, &keyed.request)
        .map(Some)
        .map_err(|e| ToolOutcome::error(e.to_string()))
}

/// The reply to a repeat of a delegate call that already created its child.
fn replayed_outcome(
    effects: &mut ToolEffects,
    child: &Agent,
    task: Option<Task>,
    wait: bool,
    key: &str,
) -> ToolOutcome {
    let task_id = task.as_ref().map(|t| t.id);
    // Waits even on a terminal row: its turn may still be landing its final
    // receipt, and the wait oracle releases a quiescent task at once.
    let waits = wait && task_id.is_some();
    if waits && let Some(id) = task_id {
        merge_wait(effects, WaitCondition::Task { id });
    }
    let structured = serde_json::json!({
        "agent_id": child.id,
        "task_id": task_id,
        "replayed": true,
        "idempotency_key": key,
        "task_status": task.as_ref().map(|t| t.status.kind()),
        "wait_for_args": {"tasks": task_id.into_iter().collect::<Vec<_>>()},
    });
    let mut outcome = ToolOutcome::ok(format!(
        "Already delegated under this idempotency_key: agent {} ({}) with task {}; nothing new was created.{}\n{structured}",
        child.id,
        child.name,
        task_id.map(|t| t.to_string()).unwrap_or_default(),
        if waits {
            format!(" {TURN_ENDS_HERE} {CHILD_WAKES}")
        } else {
            String::new()
        },
    ));
    outcome.structured = Some(structured);
    outcome
}

fn load_delegating_parent(runtime: &Arc<Runtime>, ctx: &TurnCtx) -> Checked<Agent> {
    let parent = runtime
        .store
        .agent(ctx.agent.id)?
        .ok_or_else(|| crate::RuntimeError::Invalid("Delegating parent no longer exists".into()))?;
    if parent.config.leaf {
        return Ok(Err(ToolOutcome::error(
            "Leaf agents cannot delegate children: do the work yourself, or ask your parent to staff it",
        )));
    }
    Ok(Ok(parent))
}

/// Model, engine and provider-auth checks; `Ok` carries whether the child
/// runs on an engine.
async fn check_delegation_target(
    runtime: &Arc<Runtime>,
    requested: &ModelRef,
    network: NetworkPolicy,
    parent: &Agent,
) -> Checked<bool> {
    let resolved = match runtime.models.resolve(requested) {
        Ok(resolved) => resolved,
        Err(error) => {
            return delegation_refusal(runtime, requested, network, &parent.model, error)
                .await
                .map(Err);
        }
    };
    let engine_kind = crate::turn_engine::engine_kind(resolved.provider.protocol);
    if let Some(kind) = engine_kind {
        let Some(engine) = runtime.engines.get(&kind) else {
            return delegation_refusal(
                runtime,
                requested,
                network,
                &parent.model,
                "its engine is not available in this daemon",
            )
            .await
            .map(Err);
        };
        if let Err(error) = engine.validate_network_policy(network == NetworkPolicy::Allowed) {
            return delegation_refusal(runtime, requested, network, &parent.model, error)
                .await
                .map(Err);
        }
    }
    if runtime.client_override.is_none() {
        let status = runtime.models.auth_status(&requested.provider).await;
        if !status.configured {
            return delegation_refusal(
                runtime,
                requested,
                network,
                &parent.model,
                format!(
                    "provider authentication is not configured. {}",
                    status.error.unwrap_or_default()
                ),
            )
            .await
            .map(Err);
        }
    }
    Ok(Ok(engine_kind.is_some()))
}

/// The child's scope, and the snapshot manifest when the workspace is isolated.
async fn prepare_child_scope(
    runtime: &Arc<Runtime>,
    parent: &Agent,
    request: &DelegateRequest,
) -> Checked<(Option<Scope>, Option<WorkspaceManifest>)> {
    if request.workspace_mode != aether_tools::WorkspaceMode::Isolated {
        return Ok(Ok((request.scope.clone(), None)));
    }
    let requested = request
        .scope
        .clone()
        .unwrap_or_else(|| parent.scope.clone());
    let source = match isolated_source(runtime, parent, &requested)? {
        Ok(source) => source,
        Err(refusal) => return Ok(Err(refusal)),
    };
    let (managed_root, canonical_source, parent_id) =
        (source.managed_root, source.canonical_source, parent.id);
    let manifest = match tokio::task::spawn_blocking(move || {
        crate::workspace::create(&managed_root, &canonical_source, parent_id)
    })
    .await
    .map_err(|e| crate::RuntimeError::Internal(format!("workspace snapshot task failed: {e}")))?
    {
        Ok(manifest) => manifest,
        Err(error) => {
            return Ok(Err(ToolOutcome::error(format!(
                "delegation refused: {error}"
            ))));
        }
    };
    let mut isolated_scope = requested;
    isolated_scope.allowed_paths = source
        .relative_allowed_paths
        .iter()
        .map(|path| manifest.workspace.join(path))
        .collect();
    isolated_scope.workspace_root = Some(manifest.workspace.clone());
    Ok(Ok((Some(isolated_scope), Some(manifest))))
}

/// Validates the requested source against the parent's workspace and the
/// daemon's managed directory, before anything is created on disk.
fn isolated_source(
    runtime: &Arc<Runtime>,
    parent: &Agent,
    requested: &Scope,
) -> Checked<IsolatedSource> {
    if requested.backend != aether_core::agent::ExecutionBackend::Host {
        return Ok(Err(ToolOutcome::error(
            "isolated delegation requires the host execution backend: Git worktree metadata lives outside a container's workspace mount",
        )));
    }
    if let Some(field) = runtime.widened_field_resolved(requested, &parent.scope)? {
        return Ok(Err(ToolOutcome::error(format!(
            "delegation refused: child scope widens `{field}`"
        ))));
    }
    let Some(source) = requested.workspace_root.as_ref() else {
        return Ok(Err(ToolOutcome::error(
            "isolated delegation requires a Git workspace root; set workspace_root to its repository root",
        )));
    };
    let Some(parent_root) = parent.scope.workspace_root.as_ref() else {
        return Ok(Err(ToolOutcome::error(
            "isolated delegation requires a parent workspace root",
        )));
    };
    let canonical_source = match canonical_source_within(source, parent_root) {
        Ok(path) => path,
        Err(refusal) => return Ok(Err(refusal)),
    };
    let Some(managed_root) = runtime.config.delegated_workspace_root.as_ref() else {
        return Ok(Err(ToolOutcome::error(
            "isolated delegation is unavailable: the daemon has no managed workspace directory configured",
        )));
    };
    let relative_allowed_paths = match relative_allowed_paths(requested, source) {
        Ok(paths) => paths,
        Err(refusal) => return Ok(Err(refusal)),
    };
    Ok(Ok(IsolatedSource {
        canonical_source,
        managed_root: managed_root.clone(),
        relative_allowed_paths,
    }))
}

fn canonical_source_within(
    source: &Path,
    parent_root: &Path,
) -> std::result::Result<PathBuf, ToolOutcome> {
    let (canonical_source, canonical_parent) = match (
        std::fs::canonicalize(source),
        std::fs::canonicalize(parent_root),
    ) {
        (Ok(source), Ok(parent)) => (source, parent),
        (Err(error), _) => {
            return Err(ToolOutcome::error(format!(
                "isolated workspace source {} is unavailable: {error}",
                source.display()
            )));
        }
        (_, Err(error)) => {
            return Err(ToolOutcome::error(format!(
                "parent workspace {} is unavailable: {error}",
                parent_root.display()
            )));
        }
    };
    if !canonical_source.starts_with(&canonical_parent) {
        return Err(ToolOutcome::error(format!(
            "isolated workspace source {} resolves outside parent workspace {}",
            source.display(),
            parent_root.display()
        )));
    }
    Ok(canonical_source)
}

fn relative_allowed_paths(
    requested: &Scope,
    source: &Path,
) -> std::result::Result<Vec<PathBuf>, ToolOutcome> {
    let mut relative_allowed_paths = Vec::new();
    for path in &requested.allowed_paths {
        let Ok(relative) = path.strip_prefix(source) else {
            return Err(ToolOutcome::error(format!(
                "isolated delegation cannot inherit allowed path {} outside the selected Git root {}; narrow allowed_paths first",
                path.display(),
                source.display()
            )));
        };
        relative_allowed_paths.push(relative.to_path_buf());
    }
    Ok(relative_allowed_paths)
}

fn child_spec(
    ctx: &TurnCtx,
    request: DelegateRequest,
    child_scope: Option<Scope>,
    isolated: bool,
) -> CreateAgentSpec {
    let mut config = ctx.agent.config.clone();
    // None at create_agent means inherit. Delegation instructions
    // belong to this task; inheriting "spawn a child" recursively
    // makes each leaf repeat its parent's orchestration work.
    let mut child_instructions = request.instructions.unwrap_or_default();
    if isolated {
        if !child_instructions.is_empty() {
            child_instructions.push_str("\n\n");
        }
        child_instructions.push_str(ISOLATED_INSTRUCTIONS);
    }
    config.instructions = Some(child_instructions);
    config.effort = Some(request.effort);
    config.long_lived = false;
    config.leaf = request.leaf;
    config.max_turns = None;
    if let Some(host_context) = request.host_context {
        config.host_context = host_context;
    }
    CreateAgentSpec {
        role: request.role,
        name: request.name,
        objective: Some(request.objective),
        model: Some(request.model),
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
    }
}

/// The advisory cap note and its structured form for the delegate reply.
fn cap_note(status: &CapStatus) -> (String, serde_json::Value) {
    match status {
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
    }
}

/// What the delegate reply reports about the call itself.
struct Delegated<'a> {
    task_id: Option<aether_core::id::TaskId>,
    wait: bool,
    /// Echoed so the caller can repeat the call to learn its outcome.
    key: Option<&'a str>,
}

fn delegated_outcome(
    effects: &mut ToolEffects,
    child: &Agent,
    delegated: Delegated<'_>,
    isolated: Option<&WorkspaceManifest>,
    status: &CapStatus,
) -> ToolOutcome {
    let Delegated { task_id, wait, key } = delegated;
    if wait && let Some(id) = task_id {
        merge_wait(effects, WaitCondition::Task { id });
    }
    let (cap_note, structured) = cap_note(status);
    let mut outcome = ToolOutcome::ok(format!(
        "Delegated to agent {} ({}) with task {}.{}{}",
        child.id,
        child.name,
        task_id.map(|t| t.to_string()).unwrap_or_default(),
        if wait {
            format!(" {TURN_ENDS_HERE} {CHILD_WAKES}")
        } else {
            String::new()
        },
        cap_note,
    ));
    if let Some(manifest) = isolated {
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
        s["wait_for_args"] = serde_json::json!({"tasks": task_id.into_iter().collect::<Vec<_>>()});
        if let Some(key) = key {
            s["idempotency_key"] = serde_json::json!(key);
        }
        // Engine replies carry content, not the structured field.
        // Keep the documented IDs and copyable wait arguments visible.
        outcome.content.push('\n');
        outcome.content.push_str(&s.to_string());
        s
    });
    outcome
}

/// A refused creation discards the snapshot that was made for it.
fn refuse_delegation(
    runtime: &Arc<Runtime>,
    isolated: Option<WorkspaceManifest>,
    e: RuntimeError,
) -> Result<ToolOutcome> {
    let Some(manifest) = isolated else {
        return Ok(ToolOutcome::error(format!("delegation refused: {e}")));
    };
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
}

pub(super) fn cancel_agent(
    runtime: &Arc<Runtime>,
    ctx: &TurnCtx,
    agent: AgentId,
    reason: String,
) -> Result<ToolOutcome> {
    if agent == ctx.agent.id || !runtime.store.is_descendant(ctx.agent.id, agent)? {
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

pub(super) fn send_message(
    runtime: &Arc<Runtime>,
    ctx: &TurnCtx,
    to: AgentId,
    content: String,
    subtree: bool,
) -> Result<ToolOutcome> {
    if let Some(refusal) = crate::peers::message_reach_refusal(runtime, &ctx.agent, to, subtree)? {
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

/// Delegation and agent-to-agent actions.
pub(super) async fn apply(
    runtime: &Arc<Runtime>,
    ctx: &TurnCtx,
    effects: &mut ToolEffects,
    action: HarnessAction,
) -> Result<ToolOutcome> {
    match action {
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
            idempotency,
        } => {
            let request = DelegateRequest {
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
                idempotency,
            };
            delegate(runtime, ctx, effects, request).await
        }
        HarnessAction::IntegrateWorkspace { child } => {
            integrate_child_workspace(runtime, ctx, child).await
        }
        HarnessAction::CancelAgent { agent, reason } => cancel_agent(runtime, ctx, agent, reason),
        HarnessAction::SendMessage {
            to,
            content,
            subtree,
        } => send_message(runtime, ctx, to, content, subtree),
        other => unreachable!("not a delegation action: {other:?}"),
    }
}
