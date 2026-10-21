//! Native engine bridge.
use aether_core::shell_network::gated_class;

use super::{
    ApprovalDecision, ApprovalId, ApprovalRequest, ApprovalStatus, Arc, BusMessage,
    CancellationToken, Commit, EngineRequest, EventKind, Gate, Mutation, ResourceKey, Result,
    Runtime, SideEffectClass, TURN_IS_OVER, ToolCallId, ToolContext, ToolEffects, ToolExecution,
    TurnCtx, Value, apply_action, approval, json, now, side_effect_of, tool_context,
};

/// What the turn did with a bridge call, once its end was already decided.
pub(super) enum Served {
    /// A completed reply, except permission prompts and interrupted executions.
    Ran(Option<(String, bool)>),
    /// The call would have decided the turn's end a second time, or asked a
    /// person to authorize work in a turn that is over.
    Refused,
    NativeWait(super::native_wait::NativeWait),
}

/// Run one harness call for the engine.
///
/// `parked` means a harness action already decided how the turn ends. The
/// call still runs - engines emit a parallel batch as separate bridge calls,
/// so refusing every sibling of the parking one loses real work - but it may
/// not decide the end again or cancel descendants, and nothing in it may wait
/// on a person.
#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_request(
    runtime: &Arc<Runtime>,
    ctx: &mut TurnCtx,
    tool_ctx: &Arc<ToolContext>,
    req: EngineRequest,
    effects: &mut ToolEffects,
    cancel: &CancellationToken,
    parked: bool,
    native_jobs: bool,
) -> Result<Served> {
    let mut completed = req.name != "permission";
    if parked && req.name == "permission" {
        // Nothing more may run in the turn, so the engine's own prompt is
        // denied without an approval and without blocking the wind-down.
        let _ = req.reply.send(denied_prompt(TURN_IS_OVER));
        return Ok(Served::Refused);
    }
    if parked && effects.terminal() {
        // Terminal wind-down only collects the native result/usage receipt.
        // Unlike a wait's parallel siblings, no new work belongs to this task.
        let _ = req.reply.send((TURN_IS_OVER.to_owned(), true));
        return Ok(Served::Refused);
    }
    let reply = if req.name == "permission" {
        decide_permission(runtime, ctx, &req.input, cancel).await?
    } else {
        // Resolve through exactly the list the bridge advertised, so a tool
        // the bridge never offered cannot be reached by naming it.
        match runtime
            .engine_tool_specs(&ctx.agent)
            .iter()
            .any(|s| s.name == req.name)
            .then(|| runtime.tools.get(&req.name))
            .flatten()
        {
            None => (unavailable_to_engine(runtime, ctx, &req.name), true),
            Some(tool) => {
                // A tool Aether runs on the engine's behalf is gated like any
                // other: an MCP server's mutation needs the same answer a
                // built-in one would, matched against the same resources and
                // argument a remembered rule would see on the API path.
                let declared = tool.descriptor().side_effect;
                let gate = GateSubject {
                    native_call: None,
                    provider_call_id: None,
                    tool: &req.name,
                    input: &req.input,
                    declared,
                    class: gated_class(declared, &req.input),
                    argument: tool.rule_argument(&req.input),
                    resources: tool.resources(&req.input, tool_ctx),
                    resources_known: tool.resources_known(&req.input),
                };
                let verdict = permission_verdict(runtime, ctx, gate, cancel, !parked).await?;
                if !verdict.allow {
                    let message = verdict
                        .message
                        .unwrap_or_else(|| "denied by the operator".to_owned());
                    let reply = (message, true);
                    let _ = req.reply.send(reply.clone());
                    return Ok(if parked {
                        Served::Refused
                    } else {
                        Served::Ran(Some(reply))
                    });
                }
                match tool
                    .execute(req.input.clone(), tool_ctx, cancel.child_token())
                    .await
                {
                    Ok(ToolExecution::Outcome(o)) => (o.content, o.is_error),
                    Ok(ToolExecution::Action(action)) => {
                        if native_jobs
                            && !parked
                            && let aether_tools::HarnessAction::WaitFor { condition } = action
                        {
                            if let Some(error) =
                                crate::turn::validate_wait(runtime, ctx, &condition, true)?
                            {
                                let reply = (error.content, true);
                                let _ = req.reply.send(reply.clone());
                                return Ok(Served::Ran(Some(reply)));
                            }
                            runtime.apply(Commit::new().event(ctx.draft(EventKind::Custom {
                                name: "engine.native_wait_started".into(),
                                payload: json!({"condition": condition}),
                            })))?;
                            return Ok(Served::NativeWait(super::native_wait::NativeWait::new(
                                condition, ctx, req.reply, req.input,
                            )));
                        }
                        if parked
                            && !effects.admits_after_park(&action, ctx.agent.config.long_lived)
                        {
                            let refusal = effects
                                .refusal_for_asked(&action)
                                .unwrap_or_else(|| TURN_IS_OVER.to_owned());
                            let _ = req.reply.send((refusal, true));
                            return Ok(Served::Refused);
                        }
                        let outcome = apply_action(runtime, ctx, action, effects).await?;
                        (outcome.content, outcome.is_error)
                    }
                    Err(e) => {
                        completed = !matches!(e, aether_tools::ToolError::Cancelled);
                        (e.to_string(), true)
                    }
                }
            }
        }
    };
    // Served during the wind-down: the result stands, and the stop leads it
    // so the engine does not read it as an invitation to continue. Prepended,
    // because a tool's structured payload is the reply's last line.
    let reply = if parked {
        (format!("{TURN_IS_OVER}\n\n{}", reply.0), reply.1)
    } else {
        reply
    };
    // A dropped receiver means the bridge went away; nothing to do.
    let _ = req.reply.send(reply.clone());
    Ok(Served::Ran(completed.then_some(reply)))
}

/// A permission prompt answered "no", in the shape the engine's
/// `--permission-prompt-tool` expects.
fn denied_prompt(message: &str) -> (String, bool) {
    (
        json!({ "behavior": "deny", "message": message }).to_string(),
        false,
    )
}

/// Why a tool the engine named is not there for it: the exposure or scope
/// reason when the registry has one, else that the bridge never offered it.
fn unavailable_to_engine(runtime: &Arc<Runtime>, ctx: &TurnCtx, name: &str) -> String {
    if ctx.agent.config.leaf && name == "delegate" {
        return "Leaf agents cannot delegate children: do the work yourself, or ask your parent to staff it".into();
    }
    if let Some(reason) = crate::withheld_by_position(&ctx.agent, name) {
        return format!("`{name}` is withheld from {reason}");
    }
    let withheld = runtime
        .tools
        .withheld_reason(name, &ctx.agent.config.tools, &ctx.agent.scope);
    match (withheld, runtime.tools.get(name)) {
        (Some(reason), _) => format!("`{name}` is not available to this agent: {reason}"),
        (None, Some(_)) => format!("`{name}` is not offered through the engine bridge"),
        (None, None) => format!("unknown tool `{name}`"),
    }
}

/// Answer a Claude Code permission prompt (through the MCP bridge) in the
/// shape its `--permission-prompt-tool` expects.
async fn decide_permission(
    runtime: &Arc<Runtime>,
    ctx: &TurnCtx,
    input: &Value,
    cancel: &CancellationToken,
) -> Result<(String, bool)> {
    // Only reached while the turn is live; a parked turn denies before here.
    let tool_name = input
        .get("tool_name")
        .and_then(Value::as_str)
        .unwrap_or("tool")
        .to_owned();
    let tool_input = input.get("input").cloned().unwrap_or(json!({}));
    let verdict = permission_verdict(
        runtime,
        ctx,
        GateSubject::from_prompt(
            runtime,
            ctx,
            &tool_name,
            &tool_input,
            side_effect_of(&tool_name),
            input["tool_use_id"].as_str(),
        )?,
        cancel,
        true,
    )
    .await?;
    if !verdict.allow {
        return Ok(denied_prompt(
            verdict
                .message
                .as_deref()
                .unwrap_or("denied by the operator"),
        ));
    }
    Ok((
        json!({ "behavior": "allow", "updatedInput": tool_input }).to_string(),
        false,
    ))
}

/// What the runtime decided about an engine's permission prompt.
pub(super) struct Verdict {
    pub(super) allow: bool,
    pub(super) message: Option<String>,
}

/// One call put to the gate. A tool Aether owns fills in its own argument and
/// resources so a remembered rule matches the same way it does on the API
/// path; a call the engine ran itself can only offer what the prompt carried.
pub(super) struct GateSubject<'a> {
    pub(super) native_call: Option<ToolCallId>,
    provider_call_id: Option<&'a str>,
    pub(super) tool: &'a str,
    pub(super) input: &'a Value,
    /// The tool's own class; `class` is it raised by what the input does.
    pub(super) declared: SideEffectClass,
    pub(super) class: SideEffectClass,
    pub(super) argument: Option<String>,
    pub(super) resources: Vec<ResourceKey>,
    /// Whether `resources` is the whole truth; false fails the gate closed.
    pub(super) resources_known: bool,
}

impl<'a> GateSubject<'a> {
    /// What is known about a call Aether did not run: what the prompt's
    /// payload names, and nothing more.
    pub(super) fn from_prompt(
        runtime: &Arc<Runtime>,
        ctx: &TurnCtx,
        tool: &'a str,
        input: &'a Value,
        declared: SideEffectClass,
        provider_call_id: Option<&'a str>,
    ) -> Result<Self> {
        // Derived through the very functions the built-in tools use, so a
        // rule matches the same call on either path. `None` means nothing
        // path-shaped and not a shell: unknown, so the gate fails closed.
        let resources = permission_resources(&tool_context(runtime, ctx), tool, input);
        Ok(Self {
            native_call: crate::tool_interruption::native_call_id(
                runtime,
                ctx.agent.id,
                provider_call_id,
            )?,
            provider_call_id,
            tool,
            input,
            declared,
            class: gated_class(declared, input),
            argument: permission_argument(input),
            resources_known: resources.is_some(),
            resources: resources.unwrap_or_default(),
        })
    }
}

/// The class to record for a call the engine ran itself. Aether's own tools
/// reach the engine through the bridge under an `mcp__aether__` prefix, so
/// their real descriptor is what the log must carry; only a tool Aether does
/// not know falls back to inferring from the name.
pub(super) fn engine_call_class(
    runtime: &Arc<Runtime>,
    name: &str,
    input: &Value,
) -> SideEffectClass {
    // The contract tells the model both forms (`aether-engine`'s
    // `contract.rs`), so the dotted one has to be stripped too or the class
    // the gate used and the class the log records disagree.
    let bare = bridge_tool_name(name);
    let declared = match runtime.tools.get(bare) {
        Some(tool) => tool.descriptor().side_effect,
        None => side_effect_of(name),
    };
    gated_class(declared, input)
}

pub(super) fn bridge_tool_name(name: &str) -> &str {
    name.strip_prefix("mcp__aether__")
        .or_else(|| name.strip_prefix("mcp__aether."))
        .or_else(|| name.strip_prefix("aether__"))
        .or_else(|| name.strip_prefix("aether."))
        .unwrap_or(name)
}

/// Allow silently when the agent's policy does not require approval for the
/// class; otherwise raise an approval and wait for a person.
pub(super) async fn permission_verdict(
    runtime: &Arc<Runtime>,
    ctx: &TurnCtx,
    gate: GateSubject<'_>,
    cancel: &CancellationToken,
    may_ask: bool,
) -> Result<Verdict> {
    let GateSubject {
        native_call,
        provider_call_id,
        tool: tool_name,
        input: tool_input,
        declared,
        class,
        argument,
        resources,
        resources_known,
    } = gate;
    let allow = || Verdict {
        allow: true,
        message: None,
    };
    let refuse = |msg: &str| Verdict {
        allow: false,
        message: Some(msg.to_owned()),
    };
    let deny = |msg: &str| -> Result<Verdict> {
        if let Some(call) = native_call {
            runtime.apply(Commit::new().event(ctx.draft(EventKind::ToolDenied {
                call,
                reason: msg.to_owned(),
            })))?;
        } else if let Some(provider_call_id) = provider_call_id {
            // Claude's bridge can beat its stdout tool announcement. Keep the
            // exact identity until that announcement can close the denied call.
            runtime.apply(Commit::new().event(ctx.draft(EventKind::Custom {
                name: crate::tool_interruption::NATIVE_DENIED.into(),
                payload: json!({"provider_call_id": provider_call_id, "reason": msg}),
            })))?;
        }
        Ok(refuse(msg))
    };
    if !ctx.agent.config.approval.requires_raised(declared, class) {
        return Ok(allow());
    }
    let approval = ApprovalRequest {
        id: ApprovalId::new(),
        agent_id: ctx.agent.id,
        task_id: ctx.task.as_ref().map(|t| t.id),
        call_id: native_call.unwrap_or_else(ToolCallId::new),
        tool: tool_name.to_owned(),
        description: describe_permission(tool_name, tool_input),
        argument: argument.clone(),
        resources: resources.clone(),
        side_effect: class,
        requested_at: now(),
        status: ApprovalStatus::Pending,
    };
    let subject = approval::subject_of(tool_name, argument, &resources, resources_known);
    let rule_ctx = runtime.rule_context(ctx.agent.id)?;
    if let Gate::Remembered {
        rule,
        decision,
        note,
    } = runtime.gate_call(true, &subject, &rule_ctx)?
    {
        let mut commit = Commit::new();
        approval::remembered_commit(
            &mut commit,
            |kind| ctx.draft(kind),
            &approval,
            rule,
            decision,
            note.clone(),
        );
        runtime.apply(commit)?;
        return match decision {
            ApprovalDecision::Approved => Ok(allow()),
            ApprovalDecision::Denied => deny(&approval::describe_denial(rule, note.as_deref())),
        };
    }
    // A turn whose end is already recorded may not authorize new work in
    // itself, and an approval wait is unbounded - awaiting one here would
    // suspend the wind-down for as long as nobody answers.
    if !may_ask {
        return Ok(refuse(TURN_IS_OVER));
    }
    // Subscribe before committing so the resolution cannot slip past us.
    let mut rx = runtime.subscribe();
    runtime.apply(
        Commit::new()
            .event(ctx.draft(EventKind::ApprovalRequested {
                approval: approval.id,
                call: approval.call_id,
                tool: tool_name.to_owned(),
                description: approval.description.clone(),
                argument: approval.argument.clone(),
                side_effect: class,
            }))
            .mutate(Mutation::PutApproval(approval.clone())),
    )?;
    loop {
        tokio::select! {
            msg = rx.recv() => match msg {
                Ok(BusMessage::Event(e)) => {
                    if let EventKind::ApprovalResolved { approval: id, decision, note, .. } = &e.kind
                        && *id == approval.id
                    {
                        return match decision {
                            ApprovalDecision::Approved => Ok(allow()),
                            ApprovalDecision::Denied => deny(note.as_deref().unwrap_or("denied by the operator")),
                        };
                    }
                }
                Ok(BusMessage::Delta { .. }) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    if let Some(a) = runtime.store.approval(approval.id)?
                        && let ApprovalStatus::Resolved { decision, note, .. } = a.status
                    {
                        return match decision {
                            ApprovalDecision::Approved => Ok(allow()),
                            ApprovalDecision::Denied => deny(note.as_deref().unwrap_or("denied")),
                        };
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return Ok(refuse("runtime shutting down")),
            },
            _ = cancel.cancelled() => {
                if let Some(a) = runtime.store.approval(approval.id)?
                    && let ApprovalStatus::Resolved { decision: ApprovalDecision::Denied, note, .. } = a.status
                {
                    return deny(note.as_deref().unwrap_or("denied by the operator"));
                }
                return Ok(refuse("cancelled"));
            },
        }
    }
}

/// What the person sees on the approval card: the command itself when
/// there is one, and the engine's reason for asking.
/// The argument a person would write a rule about: a shell's command, the
/// path a file tool names, else what the call is keyed on (url, pattern,
/// query). `None` leaves any argument-scoped deny unevaluable, so the gate
/// asks rather than letting a broad allow decide.
pub(super) fn permission_argument(input: &Value) -> Option<String> {
    [
        "command",
        "file_path",
        "path",
        "notebook_path",
        "url",
        "pattern",
        "query",
    ]
    .iter()
    .find_map(|field| input.get(*field).and_then(Value::as_str))
    .map(str::to_owned)
}

/// What an engine's call touches, derived through the very functions the
/// built-in tools use so a rule matches the same call on either path.
/// `None` means unknown - the payload said nothing we could key on - and a
/// subject built from it fails closed.
pub(super) fn permission_resources(
    tools: &ToolContext,
    tool_name: &str,
    input: &Value,
) -> Option<Vec<ResourceKey>> {
    let paths: Vec<ResourceKey> = ["file_path", "path", "notebook_path"]
        .iter()
        .filter_map(|field| input.get(*field).and_then(Value::as_str))
        .flat_map(|p| aether_tools::fs::resource_paths(tools, p))
        .map(ResourceKey::file)
        .collect();
    if !paths.is_empty() {
        return Some(paths);
    }
    if side_effect_of(tool_name) == SideEffectClass::Mutation
        && input.get("command").and_then(Value::as_str).is_some()
    {
        let keys = aether_tools::shell_resources(tools);
        // Never a known-empty set: nothing derived means unknown, not "touches nothing".
        return (!keys.is_empty()).then_some(keys);
    }
    None
}

fn describe_permission(tool: &str, input: &Value) -> String {
    let mut text = match input.get("command").and_then(Value::as_str) {
        Some(command) => format!("{tool}: {}", truncate(command, 200)),
        None => format!("{tool} {}", truncate(&input.to_string(), 300)),
    };
    if let Some(reason) = input
        .get("reason")
        .and_then(Value::as_str)
        .filter(|r| !r.trim().is_empty())
    {
        text.push_str(&format!(" · {}", truncate(reason, 160)));
    }
    text
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        format!("{}…", text.chars().take(max).collect::<String>())
    }
}
