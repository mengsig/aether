//! A Codex command that outlives its tool call (a lock waiter, a CI run) is a
//! pending exec process. `wait_for` must keep the session, and with it the
//! process, attached until the process exits, in both Codex modes.

use super::*;
use serde_json::json;

/// `codex exec` or `codex app-server` by its first argument. Either way it
/// starts a real lock waiter, reports it as a running command and reports it
/// done only once it exits; `release` in its directory frees the lock.
const FAKE_CODEX: &str = r#"#!/bin/sh
dir=$(dirname "$0")
printf '%s\n' "$@" > "$dir/argv"
lock_waiter() {
  (while [ ! -e "$dir/release" ]; do sleep 0.05; done) &
  echo $! > "$dir/job.pid.tmp" && mv "$dir/job.pid.tmp" "$dir/job.pid"
}
if [ "$1" = exec ]; then
  printf '%s\n' '{"type":"thread.started","thread_id":"thread"}'
  lock_waiter
  printf '%s\n' '{"type":"item.started","item":{"id":"item_0","type":"command_execution","command":"flock -x gpu.lock job","status":"in_progress"}}'
  wait "$(cat "$dir/job.pid")"
  printf '%s\n' '{"type":"item.completed","item":{"id":"item_0","type":"command_execution","command":"flock -x gpu.lock job","aggregated_output":"got the lock\n","exit_code":0,"status":"completed"}}'
  printf '%s\n' '{"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"done"}}'
  printf '%s\n' '{"type":"turn.completed","usage":{"input_tokens":1,"cached_input_tokens":0,"output_tokens":1}}'
  exit 0
fi
while read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/^{[^{]*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"config/read"'*) result='{"layers":[]}' ;;
    *'"method":"thread/start"'*) result='{"thread":{"id":"thread"},"model":"gpt-test"}' ;;
    *'"method":"mcpServerStatus/list"'*) result='{"data":[{"name":"aether","runtimeStatus":"connected","tools":{"wait_for":{}}}]}' ;;
    *'"method":"turn/start"'*) result='{"turn":{"id":"turn"}}' ;;
    *) result='{}' ;;
  esac
  printf '{"jsonrpc":"2.0","id":%s,"result":%s}\n' "$id" "$result"
  case "$line" in *'"method":"turn/start"'*) break ;; esac
done
lock_waiter
printf '%s\n' '{"jsonrpc":"2.0","method":"item/started","params":{"item":{"type":"commandExecution","id":"call_job","command":"flock -x gpu.lock job","status":"inProgress"}}}'
wait "$(cat "$dir/job.pid")"
printf '%s\n' '{"jsonrpc":"2.0","method":"item/completed","params":{"item":{"type":"commandExecution","id":"call_job","command":"flock -x gpu.lock job","aggregatedOutput":"got the lock\n","exitCode":0,"status":"completed"}}}'
printf '%s\n' '{"jsonrpc":"2.0","method":"item/completed","params":{"item":{"type":"agentMessage","id":"m","text":"done"}}}'
printf '%s\n' '{"jsonrpc":"2.0","method":"turn/completed","params":{"turn":{"status":"completed"}}}'
cat > /dev/null
"#;

fn fake_codex() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    // Written by a child, so no fd this process holds can make a sibling
    // test's exec of it fail with ETXTBSY.
    let status = std::process::Command::new("/bin/sh")
        .args(["-c", r#"printf '%s' "$1" > "$2" && chmod 700 "$2""#, "sh"])
        .arg(FAKE_CODEX)
        .arg(dir.path().join("codex"))
        .status()
        .unwrap();
    assert!(status.success());
    dir
}

/// The file's contents once the fake has written it.
async fn written(path: &std::path::Path) -> String {
    for _ in 0..500 {
        if let Ok(text) = std::fs::read_to_string(path) {
            return text;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("{} was never written", path.display());
}

/// The `--agent` and `--token` the CLI was given for Aether's MCP bridge.
fn bridge_credentials(argv: &str) -> (AgentId, String) {
    let args = argv
        .lines()
        .find_map(|arg| arg.strip_prefix("mcp_servers.aether.args="))
        .expect("the bridge arguments are on the command line");
    let args: Vec<String> = serde_json::from_str(args).unwrap();
    let after = |flag: &str| {
        let at = args.iter().position(|a| a == flag).expect(flag);
        args[at + 1].clone()
    };
    (after("--agent").parse().unwrap(), after("--token"))
}

fn alive(pid: &str) -> bool {
    std::process::Command::new("kill")
        .args(["-0", pid.trim()])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success()
}

fn tool_started(h: &Harness, id: AgentId) -> bool {
    h.store
        .events(0, 10_000, &EventFilter::for_agent(id))
        .unwrap()
        .iter()
        .any(|e| matches!(e.kind, EventKind::ToolStarted { .. }))
}

fn custom_events(h: &Harness, id: AgentId, name: &str) -> usize {
    h.store
        .events(0, 10_000, &EventFilter::for_agent(id))
        .unwrap()
        .iter()
        .filter(|e| matches!(&e.kind, EventKind::Custom { name: n, .. } if n == name))
        .count()
}

/// `job` is the id the CLI gives the lock waiter's command item.
async fn pending_exec_survives_wait_for(
    engine: Arc<dyn Engine>,
    dir: &tempfile::TempDir,
    job: &str,
) {
    let h = harness_with_engine_config(engine, |config| {
        config.engine_park_grace = Duration::from_millis(100);
    })
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "trainer".into(),
                objective: Some("queue the GPU job behind the lock, then wait".into()),
                start: true,
                config: Some(AgentConfig {
                    long_lived: true,
                    approval: ApprovalPolicy::none(),
                    ..Default::default()
                }),
                ..Default::default()
            },
            Actor::user(),
        )
        .unwrap();
    let id = agent.id;
    let pid = written(&dir.path().join("job.pid")).await;
    // The model calls `wait_for` once the command has yielded, long after
    // the runtime recorded it.
    wait_for(h.store.as_ref(), id, |_| tool_started(&h, id)).await;
    let argv = written(&dir.path().join("argv")).await;
    assert!(
        argv.lines()
            .any(|arg| arg == "mcp_servers.aether.tool_timeout_sec=31536000"),
        "Codex's 300 s MCP default would fail a held wait: {argv}"
    );
    let (bridged, token) = bridge_credentials(&argv);
    assert_eq!(bridged, id);
    let runtime = h.runtime.clone();
    let wait = tokio::spawn(async move {
        runtime
            .engine_tool(
                id,
                &token,
                "wait_for".into(),
                json!({"timeout_seconds": 3600}),
            )
            .await
    });
    // Many park graces: a parked turn would have stopped the CLI, and the
    // lock waiter in its process group, by now.
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    if wait.is_finished() {
        panic!("wait_for ended the turn: {:?}", wait.await);
    }
    assert!(alive(&pid), "the pending exec process died across wait_for");
    assert_eq!(custom_events(&h, id, "engine.native_wait_started"), 1);

    std::fs::write(dir.path().join("release"), "").unwrap();
    let (reply, is_error) = tokio::time::timeout(Duration::from_secs(10), wait)
        .await
        .expect("the process's exit ends the wait")
        .unwrap()
        .unwrap();
    assert!(
        !is_error && reply.contains(&format!("{job} finished")),
        "{reply}"
    );
    let ended = wait_for(h.store.as_ref(), id, |a| a.stats.turns >= 1).await;
    assert!(
        !matches!(ended.status, AgentStatus::Failed { .. }),
        "{:?}",
        ended.status
    );
    assert_eq!(
        custom_events(&h, id, "engine.parked"),
        0,
        "the turn was never parked"
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn a_codex_exec_turn_keeps_its_pending_exec_process_across_wait_for() {
    let dir = fake_codex();
    let program = dir.path().join("codex");
    let engine = Arc::new(aether_engine::CodexEngine::new(
        program.to_string_lossy(),
        "0.164.0",
    ));
    pending_exec_survives_wait_for(engine, &dir, "item_0").await;
}

#[tokio::test]
async fn a_codex_app_server_turn_keeps_its_pending_exec_process_across_wait_for() {
    let dir = fake_codex();
    let program = dir.path().join("codex");
    let engine = Arc::new(aether_engine::codex_app::CodexAppEngine::new(
        program.to_string_lossy(),
        "0.164.0",
    ));
    pending_exec_survives_wait_for(engine, &dir, "call_job").await;
}
