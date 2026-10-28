use super::*;

/// A tool output too large for the model context is stored as an artifact,
/// masked before it is stored, and the export masks every payload again on
/// its way out. They are the largest and least reviewed content in the file.
#[tokio::test]
async fn a_secret_in_an_artifact_payload_does_not_travel() {
    let secret = "sk-ant-abcdefghijklmnopqrstuvwxyz0123456789";
    // Buried in the middle of an oversized output, past where the head/tail
    // excerpt that reaches the model looks: only the stored payload has it.
    let command = format!(
        "for i in $(seq 1 2000); do echo \"line $i of the log\"; done; \
         echo \"deploy token {secret}\"; \
         for i in $(seq 2001 4000); do echo \"line $i of the log\"; done"
    );
    let source = daemon(vec![
        ScriptedClient::tool_call("bash", json!({ "command": command })),
        ScriptedClient::tool_call("complete_task", json!({"summary": "read the log"})),
    ])
    .await;
    let (agent, _) = source
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                objective: Some("read the log".into()),
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
    wait_terminal(source.store.as_ref(), agent.id).await;
    let run = source
        .store
        .agent(agent.id)
        .expect("agent")
        .expect("exists")
        .run;

    let stored: Vec<Vec<u8>> = source
        .store
        .artifacts(&aether_store::ArtifactFilter {
            with_bytes_only: true,
            ..Default::default()
        })
        .expect("artifacts")
        .into_iter()
        .filter_map(|a| source.store.read_artifact_bytes(a.id).expect("read"))
        .collect();
    assert!(
        stored.iter().any(|b| {
            let text = String::from_utf8_lossy(b);
            text.contains("line 2000 of the log") && text.contains("[REDACTED:anthropic_key]")
        }),
        "the run stored no masked copy of the output; the test proves nothing"
    );
    assert!(
        stored
            .iter()
            .all(|b| !String::from_utf8_lossy(b).contains(secret)),
        "a key in a tool output reached the artifact store"
    );

    let dir = tempfile::tempdir().expect("dir");
    let file = dir.path().join("run.aether");
    let exported = transfer::export_run(
        source.store.as_ref(),
        run,
        &file,
        &aether_tools::Redactor::default(),
    )
    .expect("export");
    assert!(exported.artifact_payloads > 0, "no payload was written");

    let text = std::fs::read_to_string(&file).expect("read export");
    assert!(
        !text.contains(secret),
        "a key in an artifact payload left the machine base64-encoded"
    );

    // Decoded, so a payload that merely failed to be written still fails.
    let payloads: Vec<String> = text
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|v| v.get("record").and_then(|r| r.as_str()) == Some("artifact_bytes"))
        .filter_map(|v| {
            let b64 = v.get("base64")?.as_str()?.to_owned();
            let bytes =
                base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64.as_bytes())
                    .ok()?;
            Some(String::from_utf8_lossy(&bytes).into_owned())
        })
        .collect();
    assert!(!payloads.is_empty(), "no payload decoded");
    assert!(
        payloads.iter().any(|p| p.contains("line 2000 of the log")),
        "the payload that carried the secret is not the one that travelled"
    );
    assert!(
        payloads.iter().all(|p| !p.contains(secret)),
        "the payload still carries the key once decoded"
    );
    assert!(
        payloads.iter().any(|p| p.contains("REDACTED")),
        "nothing in the payload was redacted at all"
    );
}

#[tokio::test]
async fn a_secret_in_a_tool_argument_does_not_travel() {
    let secret = "sk-ant-abcdefghijklmnopqrstuvwxyz0123456789";
    let source = daemon(vec![
        ScriptedClient::tool_call(
            "bash",
            json!({"command": format!("deploy --token {secret}")}),
        ),
        ScriptedClient::tool_call("complete_task", json!({"summary": "deployed"})),
    ])
    .await;
    let (agent, _) = source
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                objective: Some("deploy".into()),
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
    wait_terminal(source.store.as_ref(), agent.id).await;
    let run = source
        .store
        .agent(agent.id)
        .expect("agent")
        .expect("exists")
        .run;

    let dir = tempfile::tempdir().expect("dir");
    let file = dir.path().join("run.aether");
    transfer::export_run(
        source.store.as_ref(),
        run,
        &file,
        &aether_tools::Redactor::default(),
    )
    .expect("export");
    let text = std::fs::read_to_string(&file).expect("read");
    assert!(
        text.contains("REDACTED"),
        "the argument was never redacted at all"
    );
    assert!(
        !text.contains(secret),
        "a key typed into a tool argument left the machine in the export"
    );

    // And the local log keeps it, because an approval answered later re-runs
    // the call from that event.
    let logged = source
        .store
        .events(0, 100_000, &EventFilter::for_run(run))
        .expect("events")
        .iter()
        .any(|e| {
            serde_json::to_string(&e.kind)
                .expect("ser")
                .contains(secret)
        });
    assert!(logged, "the local log lost the argument the tool needs");
}
