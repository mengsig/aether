//! background tools scenarios and fixtures.
use super::*;

#[tokio::test]
async fn a_background_process_is_polled_and_dies_with_its_agent() {
    let marker_name = "bg.pid";
    let h = harness(vec![
        // Start something long-lived that announces its pid.
        ScriptedClient::tool_call(
            "bash",
            json!({
                "command": format!("echo $$ > {marker_name}.tmp && mv {marker_name}.tmp {marker_name} && echo up && sleep 120"),
                "background": true
            }),
        ),
        ScriptedClient::tool_call("shell_processes", json!({})),
        ScriptedClient::text("it is running in the background"),
    ])
    .await;
    let mut s = spec("start a server");
    s.config = Some(AgentConfig {
        approval: ApprovalPolicy::none(),
        long_lived: true,
        ..Default::default()
    });
    let (agent, _) = h.runtime.create_agent(s, Actor::user()).expect("create");
    h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;

    let outcomes: Vec<String> = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(agent.id))
        .expect("events")
        .into_iter()
        .filter_map(|e| match e.kind {
            EventKind::ToolSucceeded { outcome, .. } => Some(outcome.content),
            _ => None,
        })
        .collect();
    assert!(
        outcomes.iter().any(|o| o.contains("in the background")),
        "bash must return a handle: {outcomes:?}"
    );
    assert!(
        outcomes.iter().any(|o| o.contains("running")),
        "shell_processes must list it running: {outcomes:?}"
    );

    // It outlives the turn that started it.
    let pid = wait_for_marker(&h.workspace_root, marker_name).await;
    // Only Linux can prove the positive half; elsewhere the negative
    // assertion below still holds the important guarantee.
    #[cfg(target_os = "linux")]
    assert!(process_alive(pid), "the process must survive its own turn");

    // Cancelling the agent takes its processes with it.
    h.runtime
        .cancel_agent(agent.id, Some("done with it".into()), Actor::user())
        .expect("cancel");
    h.wait_until(agent.id, |s| s.is_terminal()).await;
    // A liveness flip has no partial-progress signal to watch, so this is a
    // ceiling-only wait (stall window == ceiling): tolerant of a slow reap
    // under contention, but still bounded.
    wait_for_with_budget(
        h.wait_ceiling,
        h.wait_ceiling,
        || (!process_alive(pid)).then_some(()),
        || (),
        || format!("background process {pid} did not exit after its agent was cancelled"),
    )
    .await;
}

/// A process an engine of the agent detached (`setsid`) outlives every turn
/// and the engine's group, but not the agent: its end reaps it and says so.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn an_engine_descendant_that_left_its_group_dies_with_its_agent() {
    let h = harness(vec![ScriptedClient::text("waiting")]).await;
    let mut s = spec("wait");
    s.config = Some(AgentConfig {
        long_lived: true,
        ..Default::default()
    });
    let (agent, _) = h.runtime.create_agent(s, Actor::user()).expect("create");
    h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;
    // What an engine's descendants inherit: the engine's agent tag.
    let output = std::process::Command::new("sh")
        .args(["-c", "setsid sleep 43 </dev/null >/dev/null 2>&1 & echo $!"])
        .env("AETHER_ENGINE_AGENT", agent.id.to_string())
        .output()
        .expect("start the detached process");
    let pid: i32 = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .expect("its pid");
    assert!(process_alive(pid), "the detached process runs");

    h.runtime
        .cancel_agent(agent.id, Some("done".into()), Actor::user())
        .expect("cancel");
    h.wait_until(agent.id, |s| s.is_terminal()).await;
    let reaped = || {
        h.store
            .events(0, 10_000, &EventFilter::for_agent(agent.id))
            .expect("events")
            .into_iter()
            .find_map(|e| match e.kind {
                EventKind::Custom { name, payload } if name == "agent.processes_reaped" => {
                    Some(payload)
                }
                _ => None,
            })
    };
    let payload = wait_for_with_budget(
        h.wait_ceiling,
        h.wait_ceiling,
        reaped,
        || (),
        || format!("no agent.processes_reaped for detached process {pid}"),
    )
    .await;

    assert!(!process_alive(pid), "the detached process is gone");
    assert!(
        payload["pids"]
            .as_array()
            .is_some_and(|pids| pids.contains(&json!(pid))),
        "{payload}"
    );
    assert_eq!(
        payload["killed"],
        json!(0),
        "sleep honours SIGTERM: {payload}"
    );
}

/// Whether `pid` is a live (non-zombie) process. `/proc` only exists on
/// Linux, so elsewhere this reports "not alive" and only the negative
/// assertions below mean anything.
fn process_alive(pid: i32) -> bool {
    if !cfg!(target_os = "linux") {
        return false;
    }
    std::path::Path::new(&format!("/proc/{pid}")).exists()
        && std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .map(|s| !s.contains(") Z "))
            .unwrap_or(false)
}

/// Fails with a retryable error a fixed number of times, then succeeds.
struct Flaky {
    remaining_failures: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl aether_tools::Tool for Flaky {
    fn descriptor(&self) -> aether_core::tool::ToolDescriptor {
        aether_core::tool::ToolDescriptor {
            spec: aether_core::model::ToolSpec {
                name: "flaky_read".into(),
                description: "fails a few times, then works".into(),
                input_schema: json!({"type": "object", "properties": {}}),
                strict: false,
            },
            side_effect: SideEffectClass::ReadOnly,
            needs_network: false,
        }
    }

    fn resources(
        &self,
        _input: &serde_json::Value,
        _ctx: &aether_tools::ToolContext,
    ) -> Vec<aether_core::tool::ResourceKey> {
        Vec::new()
    }

    async fn execute(
        &self,
        _input: serde_json::Value,
        _ctx: &aether_tools::ToolContext,
        _cancel: tokio_util::sync::CancellationToken,
    ) -> Result<aether_tools::ToolExecution, aether_tools::ToolError> {
        use std::sync::atomic::Ordering;
        if self
            .remaining_failures
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok()
        {
            return Err(aether_tools::ToolError::Io(std::io::Error::other(
                "transient",
            )));
        }
        Ok(aether_tools::ToolExecution::Outcome(
            aether_core::tool::ToolOutcome::ok("settled"),
        ))
    }
}

#[tokio::test]
async fn a_retryable_failure_is_recorded_as_tool_retried() {
    // ARCHITECTURE 12 says the turn records retries as `tool.retried`, and
    // nothing produced that event anywhere.
    let h = harness_with_tools(
        vec![
            ScriptedClient::tool_call("flaky_read", json!({})),
            ScriptedClient::tool_call("complete_task", json!({"summary": "read it"})),
        ],
        |_| {},
        |_| {},
        |tools| {
            tools.register(Arc::new(Flaky {
                remaining_failures: std::sync::atomic::AtomicUsize::new(2),
            }));
        },
    )
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(spec("read something flaky"), Actor::user())
        .expect("create");
    let status = h.wait_until(agent.id, |s| s.is_terminal()).await;
    assert_eq!(
        status.kind(),
        AgentStatusKind::Completed,
        "{:?}",
        types(h.store.as_ref(), agent.id)
    );
    let retried: Vec<_> = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(agent.id))
        .expect("events")
        .into_iter()
        .filter_map(|e| match e.kind {
            EventKind::ToolRetried { attempt, of, .. } => Some((attempt, of)),
            _ => None,
        })
        .collect();
    assert_eq!(
        retried,
        vec![(1, 3), (2, 3)],
        "two retries, each naming the attempt and what the class allows"
    );
}

/// Where phase 1's budget stop meets phase 2's background processes: the
/// scheduler ends the agent itself, outside the turn, and that path must
/// reap what the agent started or a detached command outlives its owner.
#[tokio::test]
async fn a_budget_stop_takes_the_agent_s_background_processes_with_it() {
    let marker_name = "budget-bg.pid";
    let h = harness(vec![
        ScriptedClient::tool_call(
            "bash",
            json!({
                "command": format!("echo $$ > {marker_name}.tmp && mv {marker_name}.tmp {marker_name} && sleep 120"),
                "background": true
            }),
        ),
        ScriptedClient::text("it is running in the background"),
    ])
    .await;
    let mut s = spec("start a server, then idle");
    s.config = Some(AgentConfig {
        approval: ApprovalPolicy::none(),
        long_lived: true,
        cost_budget: Some(0.01),
        ..Default::default()
    });
    let (agent, _) = h.runtime.create_agent(s, Actor::user()).expect("create");
    h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;

    let pid = wait_for_marker(&h.workspace_root, marker_name).await;
    #[cfg(target_os = "linux")]
    assert!(process_alive(pid), "the process must survive its own turn");

    // Spend past the budget by hand: the scripted model reports no cost.
    let mut over = h.store.agent(agent.id).expect("a").expect("e");
    over.stats.usage.cost_usd = 0.02;
    h.store
        .commit(Commit::new().mutate(Mutation::UpsertAgent(over)))
        .expect("commit");

    let status = h.wait_until(agent.id, |s| s.is_terminal()).await;
    let AgentStatus::Failed { error } = &status else {
        panic!("expected Failed, got {status:?}");
    };
    assert!(error.contains("cost budget"), "{error}");
    // A liveness flip has no partial-progress signal to watch, so this is a
    // ceiling-only wait (stall window == ceiling): tolerant of a slow reap
    // under contention, but still bounded.
    wait_for_with_budget(
        h.wait_ceiling,
        h.wait_ceiling,
        || (!process_alive(pid)).then_some(()),
        || (),
        || format!("background process {pid} did not exit after its budget stop"),
    )
    .await;
}
