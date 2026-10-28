use super::*;

#[tokio::test]
async fn a_run_with_a_live_agent_is_not_exported() {
    // No scripted response, so the agent asks the model, gets nothing, and
    // is still live when the export is attempted.
    let source = daemon(vec![ScriptedClient::text("thinking about it")]).await;
    let (agent, _) = source
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                objective: Some("keep going".into()),
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
    let dir = tempfile::tempdir().expect("dir");
    let error = transfer::export_run(
        source.store.as_ref(),
        agent.run,
        &dir.path().join("run.aether"),
        &aether_tools::Redactor::default(),
    )
    .expect_err("a live run must not be exported");
    assert!(error.to_string().contains("live agents"), "{error}");
}

#[tokio::test]
async fn a_file_that_is_not_an_export_is_refused() {
    let target = daemon(Vec::new()).await;
    let dir = tempfile::tempdir().expect("dir");
    let file = dir.path().join("nonsense.json");
    std::fs::write(&file, "{\"hello\":\"world\"}\n").expect("write");
    let error = transfer::import_run(&target.runtime, &file).expect_err("that is not an export");
    assert!(
        error.to_string().contains("not an aether export"),
        "{error}"
    );
}

/// The manifest promises how many events follow and the import holds the
/// file to it. The artifact sweep runs in the scheduler loop and appends
/// `artifact.evicted` to whatever run it evicts from, so the log of the run
/// being exported can grow between the count and the stream. The export is
/// pinned to the sequence it counted, so the file always matches its own
/// header - a mismatch used to be a `debug_assert` that shipped a file
/// refused on somebody else's machine.
#[tokio::test]
async fn an_event_appended_mid_export_does_not_produce_an_unimportable_file() {
    let (source, run) = finished_run().await;
    let dir = tempfile::tempdir().expect("dir");
    let store = source.store.clone();

    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let appending = {
        let (store, stop) = (store.clone(), stop.clone());
        std::thread::spawn(move || {
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                store
                    .commit(
                        aether_store::Commit::new().event(
                            aether_core::event::EventDraft::new(
                                Actor::System,
                                aether_core::event::EventKind::ArtifactEvicted {
                                    artifact: aether_core::id::ArtifactId::new(),
                                    size_bytes: 12,
                                    reason: "retention".into(),
                                },
                            )
                            .run(run),
                        ),
                    )
                    .expect("append");
                std::thread::sleep(Duration::from_micros(200));
            }
        })
    };

    for attempt in 0..30 {
        let file = dir.path().join(format!("run-{attempt}.aether"));
        let exported = transfer::export_run(
            store.as_ref(),
            run,
            &file,
            &aether_tools::Redactor::default(),
        )
        .expect("the export must not fail");
        let text = std::fs::read_to_string(&file).expect("read");
        let promised = serde_json::from_str::<serde_json::Value>(
            text.lines().next().expect("a manifest line"),
        )
        .expect("manifest")["events"]
            .as_u64()
            .expect("a count");
        let actual = text
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .filter(|v| v.get("record").and_then(|r| r.as_str()) == Some("event"))
            .count() as u64;
        assert_eq!(
            promised, actual,
            "attempt {attempt}: the file does not match its own manifest"
        );
        assert_eq!(exported.events, actual);
        assert!(
            !dir.path()
                .join(format!("run-{attempt}.aether.partial"))
                .exists(),
            "the partial file was left behind"
        );
    }

    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    appending.join().expect("appender");

    // And a file written while the log moved still imports.
    let target = daemon(Vec::new()).await;
    transfer::import_run(&target.runtime, &dir.path().join("run-29.aether")).expect("import");
}

/// The id map is seeded with the manifest's run, so a record naming another
/// run would be minted a run of its own: agents with no provenance event,
/// absent from the summary the operator reads. The file is refused instead.
#[tokio::test]
async fn a_record_from_another_run_is_refused() {
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

    let stranger = RunId::new();
    let text = std::fs::read_to_string(&file).expect("read");
    let mut out = String::new();
    let mut moved = 0usize;
    for line in text.lines() {
        let mut record: serde_json::Value = serde_json::from_str(line).expect("record");
        if record.get("record").and_then(|r| r.as_str()) == Some("agent") && moved == 0 {
            record["run"] = json!(stranger.to_string());
            moved += 1;
        }
        out.push_str(&serde_json::to_string(&record).expect("line"));
        out.push('\n');
    }
    assert_eq!(moved, 1, "no agent to move");
    std::fs::write(&file, out).expect("write");

    let target = daemon(Vec::new()).await;
    let error = transfer::import_run(&target.runtime, &file).expect_err("a stranger is refused");
    assert!(error.to_string().contains(&stranger.to_string()), "{error}");
    assert!(
        target
            .store
            .agents(&AgentFilter::default())
            .expect("agents")
            .is_empty(),
        "the refused file left rows behind"
    );
}

#[tokio::test]
async fn a_truncated_file_imports_nothing() {
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

    // Cut the last record in half: the failure lands after hundreds of good
    // records, which is exactly where a batching import committed a partial
    // run that no command could then name or remove.
    let text = std::fs::read_to_string(&file).expect("read");
    let lines: Vec<&str> = text.lines().collect();
    assert!(
        lines.len() > 20,
        "only {} records: the failure has to land after several good ones",
        lines.len()
    );
    let mut truncated = lines[..lines.len() - 1].join("\n");
    truncated.push('\n');
    let last = lines[lines.len() - 1];
    truncated.push_str(&last[..last.len() / 2]);
    truncated.push('\n');
    std::fs::write(&file, truncated).expect("write");

    let target = daemon(Vec::new()).await;
    let before = target.store.last_seq().expect("seq");
    let error = transfer::import_run(&target.runtime, &file).expect_err("a cut record");
    assert!(error.to_string().contains("unreadable record"), "{error}");

    assert_eq!(
        target.store.last_seq().expect("seq"),
        before,
        "a failed import committed events"
    );
    assert!(
        target
            .store
            .agents(&AgentFilter::default())
            .expect("agents")
            .is_empty(),
        "a failed import left agents behind"
    );
    assert!(
        target
            .store
            .tasks(&aether_store::TaskFilter::default())
            .expect("tasks")
            .is_empty(),
        "a failed import left tasks behind"
    );
    assert!(
        aether_daemon_runs(target.store.as_ref()).is_empty(),
        "a failed import left a run that runs list would show"
    );
}

#[tokio::test]
async fn a_file_missing_records_is_refused_against_its_own_manifest() {
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

    // Drop one event line, keeping the file otherwise well-formed: without
    // the manifest's count there is nothing to notice it by.
    let text = std::fs::read_to_string(&file).expect("read");
    let mut kept: Vec<&str> = Vec::new();
    let mut dropped = false;
    for line in text.lines() {
        let is_event = serde_json::from_str::<serde_json::Value>(line)
            .ok()
            .and_then(|v| v.get("record").and_then(|r| r.as_str()).map(str::to_owned))
            == Some("event".into());
        if is_event && !dropped {
            dropped = true;
            continue;
        }
        kept.push(line);
    }
    assert!(dropped, "the export carried no event to drop");
    std::fs::write(&file, kept.join("\n") + "\n").expect("write");

    let target = daemon(Vec::new()).await;
    let error = transfer::import_run(&target.runtime, &file).expect_err("a short file");
    assert!(
        error.to_string().contains("the manifest promises"),
        "{error}"
    );
    assert!(
        target
            .store
            .agents(&AgentFilter::default())
            .expect("agents")
            .is_empty(),
        "a refused import left agents behind"
    );
}

#[tokio::test]
async fn duplicate_artifact_payload_is_refused_before_commit() {
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
    let mut text = std::fs::read_to_string(&file).expect("read");
    let payload = text
        .lines()
        .find(|line| {
            serde_json::from_str::<serde_json::Value>(line)
                .ok()
                .and_then(|record| record["record"].as_str().map(str::to_owned))
                .as_deref()
                == Some("artifact_bytes")
        })
        .expect("artifact payload")
        .to_owned();
    text.push_str(&payload);
    text.push('\n');
    std::fs::write(&file, text).expect("write");

    let target = daemon(Vec::new()).await;
    let before = target.store.last_seq().expect("seq");
    let error = transfer::import_run(&target.runtime, &file).expect_err("duplicate payload");
    assert!(
        error.to_string().contains("duplicate artifact payload"),
        "{error}"
    );
    assert_eq!(target.store.last_seq().expect("seq"), before);
}

#[tokio::test]
async fn export_refuses_existing_partial_without_replacing_it() {
    let (source, run) = finished_run().await;
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("run.aether");
    let partial = dir.path().join("run.aether.partial");
    std::fs::write(&path, b"previous export").expect("target");
    std::fs::write(&partial, b"previous partial").expect("partial");
    let error = transfer::export_run(
        source.store.as_ref(),
        run,
        &path,
        &aether_tools::Redactor::default(),
    )
    .expect_err("existing partial");
    assert!(
        matches!(error, aether_runtime::RuntimeError::Store(aether_store::StoreError::Io(ref error)) if error.kind() == std::io::ErrorKind::AlreadyExists)
    );
    let message = error.to_string();
    assert!(
        message.contains(&partial.display().to_string()),
        "{message}"
    );
    assert!(message.contains("inspect or remove"), "{message}");
    assert_eq!(std::fs::read(&path).expect("target"), b"previous export");
    assert_eq!(
        std::fs::read(&partial).expect("partial"),
        b"previous partial"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn export_refuses_symlink_partial_without_touching_its_target() {
    use std::os::unix::fs::PermissionsExt;
    let (source, run) = finished_run().await;
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("run.aether");
    let partial = dir.path().join("run.aether.partial");
    let target = dir.path().join("victim");
    std::fs::write(&target, b"keep").expect("target");
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).expect("mode");
    std::os::unix::fs::symlink(&target, &partial).expect("symlink");
    let error = transfer::export_run(
        source.store.as_ref(),
        run,
        &path,
        &aether_tools::Redactor::default(),
    )
    .expect_err("symlink partial");
    assert!(
        matches!(error, aether_runtime::RuntimeError::Store(aether_store::StoreError::Io(ref error)) if error.kind() == std::io::ErrorKind::AlreadyExists)
    );
    assert_eq!(std::fs::read(&target).expect("target"), b"keep");
    assert_eq!(
        std::fs::metadata(&target)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777,
        0o644
    );
    assert!(
        std::fs::symlink_metadata(&partial)
            .expect("partial")
            .file_type()
            .is_symlink()
    );
    assert!(!path.exists());
}
