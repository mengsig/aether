//! A Codex command that outlives its tool call (a lock waiter, a CI run) is a
//! pending exec process. `wait_for`, `ask_user` and a waiting `delegate` must
//! keep the session, and with it the process, attached, in both Codex modes.

use super::*;
use serde_json::{Value, json};

/// `codex exec` or `codex app-server` by its first argument. Either way it
/// starts a real lock waiter, reports it as a running command and reports it
/// done only once it exits; `release` in its directory frees the lock. With
/// `gate` in its directory it reports the command only once `announce` is
/// there too, as a start the bridge's wait call can beat.
const FAKE_CODEX: &str = r#"#!/bin/sh
dir=$(dirname "$0")
printf '%s\n' "$@" > "$dir/argv"
lock_waiter() {
  (while [ ! -e "$dir/release" ]; do sleep 0.05; done) &
  echo $! > "$dir/job.pid.tmp" && mv "$dir/job.pid.tmp" "$dir/job.pid"
  if [ -e "$dir/gate" ]; then
    while [ ! -e "$dir/announce" ]; do sleep 0.02; done
  fi
}
if [ "$1" = exec ]; then
  printf '{"type":"thread.started","thread_id":"thread-%s"}\n' "$$"
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
    *'"method":"thread/start"'*) result='{"thread":{"id":"thread-'"$$"'"},"model":"gpt-test"}' ;;
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

type Pending = tokio::task::JoinHandle<aether_runtime::Result<(String, bool)>>;

/// An agent whose Codex turn runs the fake, its lock waiter's pid and the
/// bridge token. One turn slot, which a held delegate hands to its child.
/// Each run of the fake is its own Codex thread, so a child can run it too.
async fn running_job(
    engine: Arc<dyn Engine>,
    dir: &tempfile::TempDir,
) -> (Harness, AgentId, String, String) {
    let auth = "[providers.claude-code.auth]\nnone = true\n[providers.codex.auth]\nnone = true\n";
    let h = build_harness(
        engine,
        Arc::default(),
        Arc::default(),
        Some(auth),
        None,
        None,
        |config| {
            config.engine_park_grace = Duration::from_millis(100);
            config.max_concurrent_turns = 1;
        },
    )
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
    let argv = written(&dir.path().join("argv")).await;
    assert!(
        argv.lines()
            .any(|arg| arg == "mcp_servers.aether.tool_timeout_sec=31536000"),
        "Codex's 300 s MCP default would fail a held wait: {argv}"
    );
    let (bridged, token) = bridge_credentials(&argv);
    assert_eq!(bridged, id);
    (h, id, pid, token)
}

/// The engine's call to an Aether tool, answered whenever the runtime replies.
fn bridge_call(h: &Harness, id: AgentId, token: &str, name: &str, input: Value) -> Pending {
    let runtime = h.runtime.clone();
    let (token, name) = (token.to_owned(), name.to_owned());
    tokio::spawn(async move { runtime.engine_tool(id, &token, name, input).await })
}

/// Many park graces later: a parked turn would have stopped the CLI, and
/// the lock waiter in its process group, by now.
async fn still_waiting(h: &Harness, id: AgentId, pid: &str, call: Pending) -> Pending {
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    if call.is_finished() {
        panic!("the wait ended the turn: {:?}", call.await);
    }
    assert!(alive(pid), "the pending exec process died across the wait");
    assert_eq!(custom_events(h, id, "engine.native_wait_started"), 1);
    call
}

async fn answered(call: Pending) -> String {
    let (reply, is_error) = tokio::time::timeout(Duration::from_secs(10), call)
        .await
        .expect("the wait ends")
        .unwrap()
        .unwrap();
    assert!(!is_error, "{reply}");
    reply
}

/// Frees the lock: the job exits, and the turn ends without ever parking.
async fn release_and_end(h: Harness, dir: &tempfile::TempDir, id: AgentId) {
    std::fs::write(dir.path().join("release"), "").unwrap();
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

/// `job` is the id the CLI gives the lock waiter's command item.
async fn pending_exec_survives_wait_for(
    engine: Arc<dyn Engine>,
    dir: &tempfile::TempDir,
    job: &str,
) {
    let (h, id, pid, token) = running_job(engine, dir).await;
    // The model calls `wait_for` once the command has yielded, long after
    // the runtime recorded it.
    wait_for(h.store.as_ref(), id, |_| tool_started(&h, id)).await;
    let call = bridge_call(&h, id, &token, "wait_for", json!({"timeout_seconds": 3600}));
    let call = still_waiting(&h, id, &pid, call).await;
    std::fs::write(dir.path().join("release"), "").unwrap();
    let reply = answered(call).await;
    assert!(reply.contains(&format!("{job} finished")), "{reply}");
    release_and_end(h, dir, id).await;
}

/// The operator answers while the job still runs: the answer is the reply,
/// and the job outlives both.
async fn pending_exec_survives_ask_user(engine: Arc<dyn Engine>, dir: &tempfile::TempDir) {
    let (h, id, pid, token) = running_job(engine, dir).await;
    wait_for(h.store.as_ref(), id, |_| tool_started(&h, id)).await;
    let question = json!({"question": "which GPU?"});
    let call = bridge_call(&h, id, &token, "ask_user", question);
    let call = still_waiting(&h, id, &pid, call).await;
    h.runtime
        .send_message(
            id,
            vec![ContentBlock::text("the second one")],
            Actor::user(),
        )
        .unwrap();
    let reply = answered(call).await;
    assert!(
        reply.starts_with("Wait ended: a message arrived.")
            && reply.contains("the second one")
            && reply.ends_with("Question recorded for the operator to see."),
        "{reply}"
    );
    assert!(!reply.contains("Your turn ends here"), "{reply}");
    assert!(alive(&pid), "the job died when the answer arrived");
    release_and_end(h, dir, id).await;
}

/// The child finishes while the job still runs: its result is the reply,
/// and the job outlives both.
async fn pending_exec_survives_delegate(engine: Arc<dyn Engine>, dir: &tempfile::TempDir) {
    let (h, id, pid, token) = running_job(engine, dir).await;
    wait_for(h.store.as_ref(), id, |_| tool_started(&h, id)).await;
    let request = json!({"model": "claude-code/haiku", "effort": "low", "role": "worker",
                         "objective": "warm the cache", "wait": true});
    let call = bridge_call(&h, id, &token, "delegate", request);
    let call = still_waiting(&h, id, &pid, call).await;
    let child = h
        .store
        .agents(&aether_store::AgentFilter {
            parent: Some(id),
            ..Default::default()
        })
        .unwrap()
        .remove(0)
        .id;
    h.runtime
        .cancel_agent(child, Some("not needed".into()), Actor::user())
        .unwrap();
    let reply = answered(call).await;
    assert!(
        reply.starts_with("Wait ended: the delegated task finished.")
            && reply.contains(&format!("Delegated to agent {child}"))
            && reply.contains("not needed"),
        "{reply}"
    );
    assert!(!reply.contains("Your turn ends here"), "{reply}");
    assert!(alive(&pid), "the job died when the child finished");
    release_and_end(h, dir, id).await;
}

/// The model sends `wait_for` in the batch that starts the command, and the
/// call reaches the runtime before the command is reported running.
async fn pending_exec_survives_a_wait_for_in_its_batch(
    engine: Arc<dyn Engine>,
    dir: &tempfile::TempDir,
    job: &str,
) {
    std::fs::write(dir.path().join("gate"), "").unwrap();
    let (h, id, pid, token) = running_job(engine, dir).await;
    let call = bridge_call(&h, id, &token, "wait_for", json!({"timeout_seconds": 3600}));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!tool_started(&h, id), "the command was reported first");
    std::fs::write(dir.path().join("announce"), "").unwrap();
    let call = still_waiting(&h, id, &pid, call).await;
    std::fs::write(dir.path().join("release"), "").unwrap();
    let reply = answered(call).await;
    assert!(reply.contains(&format!("{job} finished")), "{reply}");
    release_and_end(h, dir, id).await;
}

/// `codex exec` takes no input once started, so a message that ends its
/// wait must arrive in the reply, once: the next wait waits for news.
#[tokio::test]
async fn a_codex_exec_wait_hands_over_the_message_that_ended_it() {
    let dir = fake_codex();
    let (h, id, pid, token) = running_job(exec(&dir), &dir).await;
    wait_for(h.store.as_ref(), id, |_| tool_started(&h, id)).await;
    let call = bridge_call(&h, id, &token, "wait_for", json!({"timeout_seconds": 3600}));
    let call = still_waiting(&h, id, &pid, call).await;
    // Not a person's: a reply owed to one would refuse the next wait anyway.
    let news = vec![ContentBlock::text("use the second GPU")];
    h.runtime.send_message(id, news, Actor::System).unwrap();
    let reply = answered(call).await;
    assert!(
        reply.starts_with("Wait ended: a message arrived")
            && reply.contains("Delivered with this reply:")
            && reply.contains("use the second GPU"),
        "{reply}"
    );
    let again = bridge_call(&h, id, &token, "wait_for", json!({"timeout_seconds": 3600}));
    tokio::time::sleep(Duration::from_millis(1_000)).await;
    if again.is_finished() {
        panic!(
            "the delivered message woke the next wait: {:?}",
            again.await
        );
    }
    assert!(
        alive(&pid),
        "the pending exec process died across the waits"
    );
    release_and_end(h, &dir, id).await;
}

fn exec(dir: &tempfile::TempDir) -> Arc<dyn Engine> {
    let program = dir.path().join("codex");
    Arc::new(aether_engine::CodexEngine::new(
        program.to_string_lossy(),
        "0.164.0",
    ))
}

fn app_server(dir: &tempfile::TempDir) -> Arc<dyn Engine> {
    let program = dir.path().join("codex");
    Arc::new(aether_engine::codex_app::CodexAppEngine::new(
        program.to_string_lossy(),
        "0.164.0",
    ))
}

#[tokio::test]
async fn a_codex_exec_turn_keeps_its_pending_exec_process_across_wait_for() {
    let dir = fake_codex();
    pending_exec_survives_wait_for(exec(&dir), &dir, "item_0").await;
}

#[tokio::test]
async fn a_codex_app_server_turn_keeps_its_pending_exec_process_across_wait_for() {
    let dir = fake_codex();
    pending_exec_survives_wait_for(app_server(&dir), &dir, "call_job").await;
}

#[tokio::test]
async fn a_codex_exec_turn_keeps_its_pending_exec_process_across_ask_user() {
    let dir = fake_codex();
    pending_exec_survives_ask_user(exec(&dir), &dir).await;
}

#[tokio::test]
async fn a_codex_app_server_turn_keeps_its_pending_exec_process_across_ask_user() {
    let dir = fake_codex();
    pending_exec_survives_ask_user(app_server(&dir), &dir).await;
}

#[tokio::test]
async fn a_codex_exec_turn_keeps_its_pending_exec_process_across_a_waiting_delegate() {
    let dir = fake_codex();
    pending_exec_survives_delegate(exec(&dir), &dir).await;
}

#[tokio::test]
async fn a_codex_app_server_turn_keeps_its_pending_exec_process_across_a_waiting_delegate() {
    let dir = fake_codex();
    pending_exec_survives_delegate(app_server(&dir), &dir).await;
}

#[tokio::test]
async fn a_codex_exec_wait_for_in_the_command_s_batch_keeps_it_alive() {
    let dir = fake_codex();
    pending_exec_survives_a_wait_for_in_its_batch(exec(&dir), &dir, "item_0").await;
}

#[tokio::test]
async fn a_codex_app_server_wait_for_in_the_command_s_batch_keeps_it_alive() {
    let dir = fake_codex();
    pending_exec_survives_a_wait_for_in_its_batch(app_server(&dir), &dir, "call_job").await;
}
