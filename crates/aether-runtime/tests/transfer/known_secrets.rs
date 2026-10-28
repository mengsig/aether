//! A secret value the daemon knows is masked in every copy of tool output:
//! the stored artifact, a `read_artifact` outcome, the event log and an export.
use super::*;
use aether_core::artifact::ArtifactKind;
use aether_core::model::ContentBlock;
use aether_runtime::artifacts::{ArtifactPublication, StoreArtifactSink};
use aether_tools::ArtifactSink as _;

/// Not a real credential, and no default rule's shape: only the known-secret
/// rule can mask it.
const KNOWN: &str = "fleet-token-0123456789abcdef";
const MASK: &str = "[REDACTED:known_secret]";

struct Seeded {
    daemon: Daemon,
    client: Arc<ScriptedClient>,
}

async fn seeded(responses: Vec<aether_model::testing::ScriptedResponse>) -> Seeded {
    seeded_knowing(responses, &[KNOWN]).await
}

async fn seeded_knowing(
    responses: Vec<aether_model::testing::ScriptedResponse>,
    known: &[&str],
) -> Seeded {
    let workspace = tempfile::tempdir().expect("workspace");
    let config_dir = tempfile::tempdir().expect("config dir");
    let store: Arc<dyn Store> = Arc::new(SqliteStore::open_in_memory().expect("store"));
    let models = Arc::new(Models::load(Some(config_dir.path().to_path_buf())).expect("models"));
    std::mem::forget(config_dir);
    let client = Arc::new(ScriptedClient::with_responses(responses));
    let runtime = Runtime::start_with(
        RuntimeConfig {
            holder: "known-secrets-test".into(),
            max_concurrent_turns: 1,
            lease_ttl: Duration::from_secs(30),
            default_model: Some(ModelRef::new("anthropic", "claude-sonnet-5-5")),
            workspace_root: Some(workspace.path().canonicalize().expect("canonical")),
            redactor: aether_tools::Redactor::with_known_secrets(known.iter().copied())
                .expect("compiles"),
            ..Default::default()
        },
        store.clone(),
        models,
        Arc::new(ToolRegistry::with_builtins()),
        Some(client.clone()),
    )
    .await
    .expect("runtime");
    Seeded {
        daemon: Daemon {
            runtime,
            store,
            _workspace: workspace,
        },
        client,
    }
}

fn stored_payloads(store: &dyn Store) -> Vec<String> {
    store
        .artifacts(&aether_store::ArtifactFilter {
            with_bytes_only: true,
            ..Default::default()
        })
        .expect("artifacts")
        .into_iter()
        .filter_map(|a| store.read_artifact_bytes(a.id).expect("read"))
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .collect()
}

fn event_log(store: &dyn Store) -> String {
    serde_json::to_string(
        &store
            .events(0, 100_000, &EventFilter::default())
            .expect("events"),
    )
    .expect("events serialize")
}

fn start_worker(daemon: &Daemon, objective: &str) -> AgentId {
    let (agent, _) = daemon
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                objective: Some(objective.into()),
                start: true,
                config: Some(AgentConfig {
                    approval: ApprovalPolicy::none(),
                    ..Default::default()
                }),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("create");
    agent.id
}

#[tokio::test]
async fn a_known_secret_in_a_long_shell_output_never_reaches_the_stored_artifact() {
    // Mid-output, past the head/tail excerpt: only the full copy holds it.
    // Split in the command, which the log keeps as typed, so only the output has it.
    let (head, tail) = KNOWN.split_at(6);
    let command = format!(
        "for i in $(seq 1 2000); do echo \"line $i of the log\"; done; \
         echo \"deploy token {head}\"\"{tail}\"; \
         for i in $(seq 2001 4000); do echo \"line $i of the log\"; done"
    );
    let source = seeded(vec![
        ScriptedClient::tool_call("bash", json!({ "command": command })),
        ScriptedClient::tool_call("complete_task", json!({"summary": "read the log"})),
    ])
    .await;
    let agent = start_worker(&source.daemon, "read the log");
    wait_terminal(source.daemon.store.as_ref(), agent).await;

    let payloads = stored_payloads(source.daemon.store.as_ref());
    let log = payloads
        .iter()
        .find(|p| p.contains("line 2000 of the log"))
        .expect("the shell output was stored as an artifact");
    assert!(
        !log.contains(KNOWN),
        "the stored artifact carries the secret"
    );
    assert!(
        log.contains(&format!("deploy token {MASK}")),
        "nothing was masked"
    );
    assert!(!event_log(source.daemon.store.as_ref()).contains(KNOWN));
    source.daemon.runtime.shutdown().await;
}

#[tokio::test]
async fn a_known_secret_in_a_bounded_tool_output_never_reaches_the_stored_artifact() {
    let source = seeded(Vec::new()).await;
    let agent = start_worker(&source.daemon, "idle");
    let sink = StoreArtifactSink {
        runtime: source.daemon.runtime.clone(),
    };
    let text = format!("{}\nfetched page with {KNOWN} inside\n", "x".repeat(40_000));
    let artifact = sink
        .store(
            ArtifactKind::CommandOutput,
            "web_fetch",
            "full output",
            Some("text/plain"),
            text.into_bytes(),
            agent,
            None,
        )
        .await
        .expect("store");
    let stored = source
        .daemon
        .store
        .read_artifact_bytes(artifact.id)
        .expect("read")
        .expect("bytes");
    let stored = String::from_utf8(stored).expect("text");
    assert!(
        !stored.contains(KNOWN),
        "the stored artifact carries the secret"
    );
    assert!(
        stored.ends_with(&format!("with {MASK} inside\n")),
        "nothing was masked"
    );
    assert_eq!(artifact.size_bytes, stored.len() as u64);
    source.daemon.runtime.shutdown().await;
}

/// A published artifact keeps its bytes as sent, so the outcome of reading
/// it is where the mask must apply - before the event log, the transcript and
/// the provider see it.
#[tokio::test]
async fn reading_a_published_artifact_commits_and_sends_a_masked_outcome() {
    let source = seeded(Vec::new()).await;
    let runtime = &source.daemon.runtime;
    let (agent, _) = runtime
        .create_agent(
            CreateAgentSpec {
                role: "assistant".into(),
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
        .expect("create");
    let artifact = runtime
        .publish_artifact(
            ArtifactPublication {
                kind: ArtifactKind::File,
                name: "deploy.log".into(),
                summary: "evidence".into(),
                content_type: Some("text/plain".into()),
                bytes: format!("deploy token {KNOWN}\n").into_bytes(),
                agent: agent.id,
                task: None,
            },
            Actor::user(),
        )
        .await
        .expect("publish");
    let raw = source
        .daemon
        .store
        .read_artifact_bytes(artifact.id)
        .expect("read");
    assert!(
        String::from_utf8_lossy(&raw.expect("bytes")).contains(KNOWN),
        "a published artifact is kept as sent; the test proves nothing otherwise"
    );
    source.client.push_all([
        ScriptedClient::tool_call("read_artifact", json!({"id": artifact.id})),
        ScriptedClient::text("read it"),
    ]);
    runtime
        .send_message(
            agent.id,
            vec![ContentBlock::text("read the log")],
            Actor::user(),
        )
        .expect("send");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while source.client.remaining() > 0 || !event_log(source.daemon.store.as_ref()).contains(MASK) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the read never settled"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let events = event_log(source.daemon.store.as_ref());
    assert!(
        !events.contains(KNOWN),
        "the committed outcome carries the secret"
    );
    assert!(events.contains(&format!("deploy token {MASK}")), "{events}");
    let sent = format!("{:?}", source.client.requests());
    assert!(!sent.contains(KNOWN), "the provider was sent the secret");
    runtime.shutdown().await;
}

#[tokio::test]
async fn an_export_masks_the_daemons_known_secrets_in_records_and_payloads() {
    let source = seeded(vec![ScriptedClient::tool_call(
        "complete_task",
        json!({"summary": "published"}),
    )])
    .await;
    let runtime = &source.daemon.runtime;
    let agent = start_worker(&source.daemon, "publish");
    runtime
        .publish_artifact(
            ArtifactPublication {
                kind: ArtifactKind::File,
                name: "deploy.log".into(),
                summary: "evidence".into(),
                content_type: Some("text/plain".into()),
                bytes: format!("deploy token {KNOWN}\n").into_bytes(),
                agent,
                task: None,
            },
            Actor::user(),
        )
        .await
        .expect("publish");
    wait_terminal(source.daemon.store.as_ref(), agent).await;
    let run = source
        .daemon
        .store
        .agent(agent)
        .expect("agent")
        .expect("exists")
        .run;

    let dir = tempfile::tempdir().expect("dir");
    let file = dir.path().join("run.aether");
    let exported = transfer::export_run(
        source.daemon.store.as_ref(),
        run,
        &file,
        &runtime.config().redactor,
    )
    .expect("export");
    assert_eq!(exported.artifact_payloads, 1);
    let text = std::fs::read_to_string(&file).expect("read export");
    let payload = text
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|v| v["record"] == "artifact_bytes")
        .and_then(|v| v["base64"].as_str().map(str::to_owned))
        .expect("a payload record");
    let payload = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, payload)
        .expect("base64");
    assert_eq!(payload, format!("deploy token {MASK}\n").into_bytes());
    assert!(!text.contains(KNOWN));
    runtime.shutdown().await;
}

/// A tool's error can quote what it was given or read; the committed
/// `ToolFailed` carries it masked.
#[tokio::test]
async fn a_tool_error_quoting_a_known_secret_is_committed_masked() {
    let source = seeded(vec![
        ScriptedClient::tool_call("read_file", json!({"path": format!("/{KNOWN}/notes")})),
        ScriptedClient::tool_call("complete_task", json!({"summary": "tried"})),
    ])
    .await;
    let agent = start_worker(&source.daemon, "read the notes");
    wait_terminal(source.daemon.store.as_ref(), agent).await;
    let errors: Vec<String> = source
        .daemon
        .store
        .events(0, 10_000, &EventFilter::for_agent(agent))
        .expect("events")
        .into_iter()
        .filter_map(|event| match event.kind {
            aether_core::event::EventKind::ToolFailed { error, .. } => Some(error),
            _ => None,
        })
        .collect();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(!errors[0].contains(KNOWN), "{}", errors[0]);
    assert!(errors[0].contains(MASK), "{}", errors[0]);
    source.daemon.runtime.shutdown().await;
}

fn exported(daemon: &Daemon, agent: AgentId, file: &Path) -> String {
    let run = daemon
        .store
        .agent(agent)
        .expect("agent")
        .expect("exists")
        .run;
    transfer::export_run(
        daemon.store.as_ref(),
        run,
        file,
        &daemon.runtime.config().redactor,
    )
    .expect("export");
    std::fs::read_to_string(file).expect("read export")
}

/// A tool argument is kept as typed in the log, keys included, so the export
/// masks a secret used as a key as it does one used as a value.
#[tokio::test]
async fn an_export_masks_a_known_secret_used_as_a_json_key() {
    let source = seeded(vec![
        ScriptedClient::tool_call("list_agents", json!({ KNOWN: true })),
        ScriptedClient::tool_call("complete_task", json!({"summary": "listed"})),
    ])
    .await;
    let agent = start_worker(&source.daemon, "list");
    wait_terminal(source.daemon.store.as_ref(), agent).await;
    assert!(
        event_log(source.daemon.store.as_ref()).contains(KNOWN),
        "the argument is kept as typed; the test proves nothing otherwise"
    );

    let dir = tempfile::tempdir().expect("dir");
    let text = exported(&source.daemon, agent, &dir.path().join("run.aether"));
    assert!(!text.contains(KNOWN), "the export carries the secret");
    assert!(
        text.contains(&format!("\"{MASK}\":true")),
        "nothing was masked"
    );
    source.daemon.runtime.shutdown().await;
}

/// A digest is hashed from the masked payload, so it is never masked itself:
/// a known secret that happens to be hex would otherwise fail the import.
#[tokio::test]
async fn an_artifact_digest_holding_a_known_secret_still_imports() {
    use sha2::Digest as _;
    let bytes = b"plain evidence\n".to_vec();
    let digest = hex::encode(sha2::Sha256::digest(&bytes));
    let source = seeded_knowing(
        vec![ScriptedClient::tool_call(
            "complete_task",
            json!({"summary": "published"}),
        )],
        &[KNOWN, &digest[8..24]],
    )
    .await;
    let agent = start_worker(&source.daemon, "publish");
    source
        .daemon
        .runtime
        .publish_artifact(
            ArtifactPublication {
                kind: ArtifactKind::File,
                name: "evidence.txt".into(),
                summary: "evidence".into(),
                content_type: Some("text/plain".into()),
                bytes: bytes.clone(),
                agent,
                task: None,
            },
            Actor::user(),
        )
        .await
        .expect("publish");
    wait_terminal(source.daemon.store.as_ref(), agent).await;

    let dir = tempfile::tempdir().expect("dir");
    let file = dir.path().join("run.aether");
    let text = exported(&source.daemon, agent, &file);
    assert!(text.contains(&format!("\"digest\":\"{digest}\"")), "{text}");
    let target = daemon(Vec::new()).await;
    let imported = transfer::import_run(&target.runtime, &file).expect("the archive verifies");
    assert_eq!(imported.artifact_payloads, 1);
    let stored = target.store.live_artifacts().expect("artifacts");
    assert_eq!(stored.len(), 1);
    assert_eq!(
        target
            .store
            .read_artifact_bytes(stored[0].id)
            .expect("read"),
        Some(bytes)
    );
    source.daemon.runtime.shutdown().await;
}

/// Exports a finished run whose daemon also knows `extra`, publishing an
/// artifact first when `publish` is set, and returns why it was refused.
async fn refused_export(extra: &str, publish: bool) -> String {
    let source = seeded_knowing(
        vec![ScriptedClient::tool_call(
            "complete_task",
            json!({"summary": "done"}),
        )],
        &[KNOWN, extra],
    )
    .await;
    let runtime = &source.daemon.runtime;
    let agent = start_worker(&source.daemon, "finish");
    if publish {
        runtime
            .publish_artifact(
                ArtifactPublication {
                    kind: ArtifactKind::File,
                    name: "deploy.log".into(),
                    summary: "evidence".into(),
                    content_type: Some("text/plain".into()),
                    bytes: b"deployed\n".to_vec(),
                    agent,
                    task: None,
                },
                Actor::user(),
            )
            .await
            .expect("publish");
    }
    wait_terminal(source.daemon.store.as_ref(), agent).await;
    let run = source
        .daemon
        .store
        .agent(agent)
        .expect("agent")
        .expect("exists")
        .run;

    let dir = tempfile::tempdir().expect("dir");
    let file = dir.path().join("run.aether");
    let refused = transfer::export_run(
        source.daemon.store.as_ref(),
        run,
        &file,
        &runtime.config().redactor,
    )
    .expect_err("a record that would not import as it was is not written");
    assert!(!file.exists(), "{extra}: no archive is named as complete");
    runtime.shutdown().await;
    refused.to_string()
}

/// A known secret that is also part of the export format cannot be masked
/// without breaking the file, so the export is refused instead: a record's
/// field, the manifest's, an optional field the import would drop, or the
/// framing of an artifact payload.
#[tokio::test]
async fn an_export_is_refused_when_a_known_secret_matches_the_format() {
    for (extra, publish) in [
        ("created_at", false),
        ("binary_version", false),
        ("content_type", true),
        ("artifact_bytes", true),
    ] {
        let refused = refused_export(extra, publish).await;
        assert!(refused.contains("export format"), "{extra}: {refused}");
        assert!(!refused.contains(extra), "{extra}: {refused}");
    }
}

/// `read_artifact` renders its result as JSON, which escapes a newline, quote
/// or backslash in a secret, twice over for a secret the artifact held as JSON.
#[tokio::test]
async fn reading_an_artifact_masks_secrets_that_json_escapes() {
    const SECRETS: [&str; 3] = [
        "alpha123\nbeta4567",
        "q-sec-77\"q-tail-88",
        "b-sec-55\\b-tail-66",
    ];
    const FRAGMENTS: [&str; 6] = [
        "alpha123",
        "beta4567",
        "q-sec-77",
        "q-tail-88",
        "b-sec-55",
        "b-tail-66",
    ];
    let source = seeded_knowing(Vec::new(), &SECRETS).await;
    let runtime = &source.daemon.runtime;
    let (agent, _) = runtime
        .create_agent(
            CreateAgentSpec {
                role: "assistant".into(),
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
        .expect("create");
    let artifact = runtime
        .publish_artifact(
            ArtifactPublication {
                kind: ArtifactKind::File,
                name: "secrets.log".into(),
                summary: "evidence".into(),
                content_type: Some("text/plain".into()),
                bytes: format!(
                    "{}\n{}",
                    SECRETS.join("\n"),
                    serde_json::to_string(&SECRETS).expect("json")
                )
                .into_bytes(),
                agent: agent.id,
                task: None,
            },
            Actor::user(),
        )
        .await
        .expect("publish");
    source.client.push_all([
        ScriptedClient::tool_call("read_artifact", json!({"id": artifact.id})),
        ScriptedClient::text("read it"),
    ]);
    runtime
        .send_message(agent.id, vec![ContentBlock::text("read it")], Actor::user())
        .expect("send");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let content = loop {
        let succeeded = source
            .daemon
            .store
            .events(0, 10_000, &EventFilter::for_agent(agent.id))
            .expect("events")
            .into_iter()
            .find_map(|event| match event.kind {
                aether_core::event::EventKind::ToolSucceeded { outcome, .. } => Some(outcome),
                _ => None,
            });
        if let Some(outcome) = succeeded
            && source.client.remaining() == 0
        {
            break outcome.content;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the read never settled"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };

    let decoded: serde_json::Value = serde_json::from_str(&content).expect("JSON content");
    assert_eq!(
        decoded["text"],
        format!(
            "{}\n{}",
            [MASK; 3].join("\n"),
            serde_json::to_string(&[MASK; 3]).expect("json")
        ),
        "{content}"
    );
    let events = event_log(source.daemon.store.as_ref());
    let sent = format!("{:?}", source.client.requests());
    for fragment in FRAGMENTS {
        assert!(!events.contains(fragment), "the log carries {fragment}");
        assert!(!sent.contains(fragment), "the provider was sent {fragment}");
    }
    runtime.shutdown().await;
}
