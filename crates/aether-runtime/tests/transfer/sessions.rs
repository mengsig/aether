use super::*;

#[tokio::test]
async fn an_engine_session_does_not_travel() {
    let (source, run) = finished_run().await;
    // A session handle names a conversation on this machine; the export had
    // been carrying it and relying on the import to clear it.
    let agents = source
        .store
        .agents(&AgentFilter {
            run: Some(run),
            ..Default::default()
        })
        .expect("agents");
    let mut seeded = agents.first().cloned().expect("an agent");
    seeded.engine_session = Some("sess_on_the_exporting_machine".into());
    source
        .runtime
        .apply(aether_store::Commit::new().mutate(aether_store::Mutation::UpsertAgent(seeded)))
        .expect("seed");

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
        !text.contains("sess_on_the_exporting_machine"),
        "the export carried a live session handle"
    );
}

/// A hand-authored file, not an honest export, is the threat -
/// one carrying a session id and `engine_sent` digests that happen to match
/// would otherwise suppress the assignment block on the imported agent's
/// first engine turn. Import must scrub these regardless of what the file
/// claims, the same way it already scrubs an honest export's own values.
#[tokio::test]
async fn import_scrubs_a_tampered_engine_session_record() {
    let (source, run) = finished_run().await;
    let dir = tempfile::tempdir().expect("dir");
    let file = dir.path().join("run.aether");
    transfer::export_run(
        source.store.as_ref(),
        run,
        &file,
        &aether_tools::Redactor::default(),
    )
    .expect("export");

    let tampered: String = std::fs::read_to_string(&file)
        .expect("read")
        .lines()
        .map(|line| {
            let mut value: serde_json::Value = serde_json::from_str(line).expect("record json");
            if value.get("record").and_then(|r| r.as_str()) == Some("agent") {
                let obj = value.as_object_mut().expect("agent record is an object");
                obj.insert(
                    "engine_session".into(),
                    json!("sess_forged_by_a_tampered_file"),
                );
                obj.insert(
                    "engine_usage".into(),
                    json!({
                        "session": "sess_forged_by_a_tampered_file",
                        "total": {"input_tokens": 999_999, "output_tokens": 0,
                                  "cache_read_tokens": 0, "cache_write_tokens": 0},
                    }),
                );
                obj.insert(
                    "engine_sent".into(),
                    json!({
                        "session": "sess_forged_by_a_tampered_file",
                        "layers": {"role_task": "forged-digest-claiming-already-sent"},
                    }),
                );
            }
            serde_json::to_string(&value).expect("reserialize")
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&file, tampered).expect("write tampered file");

    let target = daemon(Vec::new()).await;
    let imported = transfer::import_run(&target.runtime, &file).expect("import");
    let agents = target
        .store
        .agents(&AgentFilter {
            run: Some(imported.run),
            ..Default::default()
        })
        .expect("agents");
    assert!(!agents.is_empty());
    for agent in agents {
        assert!(
            agent.engine_session.is_none(),
            "a tampered file forged a live session handle"
        );
        assert!(
            agent.engine_usage.is_none(),
            "a tampered file forged billing state"
        );
        assert!(
            agent.engine_sent.is_none(),
            "a tampered file forged a record of what the model was already sent, \
             which would suppress the assignment on its first engine turn"
        );
    }
}
