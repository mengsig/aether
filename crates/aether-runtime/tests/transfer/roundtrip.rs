use super::*;

#[tokio::test]
async fn a_run_survives_the_round_trip_and_reports_the_same() {
    let (source, run) = finished_run().await;
    let before = report_of(source.store.as_ref(), run);
    assert!(before.provenance.is_none(), "it was not imported here");
    assert!(before.body.events > 5, "{:?}", before.body.events);

    let dir = tempfile::tempdir().expect("dir");
    let file = dir.path().join("run.aether");
    let exported = transfer::export_run(
        source.store.as_ref(),
        run,
        &file,
        &aether_tools::Redactor::default(),
    )
    .expect("export");
    assert_eq!(exported.run, run);
    assert_eq!(exported.events, before.body.events);
    assert!(exported.bytes_written > 0);

    let target = daemon(Vec::new()).await;
    let imported = transfer::import_run(&target.runtime, &file).expect("import");
    assert_eq!(imported.source_run, run);
    assert_ne!(imported.run, run, "the import is a run of its own");
    assert_eq!(imported.events, exported.events);
    assert_eq!(imported.agents, exported.agents);

    let after = report_of(target.store.as_ref(), imported.run);
    assert_eq!(
        after.provenance.as_ref().map(|p| p.source_run),
        Some(run),
        "the import says where it came from"
    );
    assert_ne!(before.run, after.run);
    assert_eq!(
        before.canonical_json().expect("canonical"),
        after.canonical_json().expect("canonical"),
        "the report of the import must equal the report of the export"
    );
}

#[tokio::test]
async fn an_imported_run_keeps_the_times_it_ran_at() {
    let (source, run) = finished_run().await;
    let before = report_of(source.store.as_ref(), run);
    let dir = tempfile::tempdir().expect("dir");
    let file = dir.path().join("run.aether");
    transfer::export_run(
        source.store.as_ref(),
        run,
        &file,
        &aether_tools::Redactor::default(),
    )
    .expect("export");

    // Long enough that a timestamp taken at import could not be mistaken for
    // one taken while the run was going.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let target = daemon(Vec::new()).await;
    let imported = transfer::import_run(&target.runtime, &file).expect("import");
    let after = report_of(target.store.as_ref(), imported.run);
    assert_eq!(before.body.started_at, after.body.started_at);
    assert_eq!(before.body.ended_at, after.body.ended_at);
    assert_eq!(before.body.duration_ms, after.body.duration_ms);
}

#[tokio::test]
async fn an_artifact_travels_with_its_bytes() {
    let (source, run) = finished_run().await;
    let dir = tempfile::tempdir().expect("dir");
    let file = dir.path().join("run.aether");
    let exported = transfer::export_run(
        source.store.as_ref(),
        run,
        &file,
        &aether_tools::Redactor::default(),
    )
    .expect("export");
    assert!(
        exported.artifacts > 0,
        "the run produced no artifact to move"
    );
    assert_eq!(
        exported.artifact_payloads, exported.artifacts,
        "every artifact still had its bytes"
    );

    let target = daemon(Vec::new()).await;
    let imported = transfer::import_run(&target.runtime, &file).expect("import");
    let after = report_of(target.store.as_ref(), imported.run);
    for line in &after.body.artifacts {
        let bytes = target
            .store
            .read_artifact_bytes(line.id)
            .expect("read")
            .expect("bytes landed here");
        assert_eq!(bytes.len() as u64, line.size_bytes);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn an_export_is_readable_by_its_owner_only() {
    use std::os::unix::fs::PermissionsExt as _;

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
    let mode = std::fs::metadata(&file)
        .expect("metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        mode, 0o600,
        "the export carries every message and payload in the run"
    );
}

#[tokio::test]
async fn a_rule_answered_approval_survives_the_round_trip() {
    let (source, run) = run_answered_by_a_rule().await;
    let before = report_of(source.store.as_ref(), run);
    let answered_by_rule = before
        .body
        .approvals
        .iter()
        .filter(|a| matches!(a.decided_by, Some(Actor::Rule { .. })))
        .count();
    assert!(
        answered_by_rule > 0,
        "the fixture never exercised a remembered decision: {:?}",
        before.body.approvals
    );

    let dir = tempfile::tempdir().expect("dir");
    let file = dir.path().join("run.aether");
    transfer::export_run(
        source.store.as_ref(),
        run,
        &file,
        &aether_tools::Redactor::default(),
    )
    .expect("export");
    let target = daemon(Vec::new()).await;
    let imported = transfer::import_run(&target.runtime, &file).expect("import");
    let after = report_of(target.store.as_ref(), imported.run);
    assert_ne!(before.body, after.body, "raw bodies carry local ids");
    assert_eq!(
        before.canonical_json().expect("canonical"),
        after.canonical_json().expect("canonical"),
        "a decision a rule answered must survive the transfer like any other"
    );
}

#[tokio::test]
async fn the_provenance_event_carries_the_count_it_actually_imported() {
    let (source, run) = finished_run().await;
    let dir = tempfile::tempdir().expect("dir");
    let file = dir.path().join("run.aether");
    let exported = transfer::export_run(
        source.store.as_ref(),
        run,
        &file,
        &aether_tools::Redactor::default(),
    )
    .expect("export");
    let target = daemon(Vec::new()).await;
    let imported = transfer::import_run(&target.runtime, &file).expect("import");

    let provenance = target
        .store
        .events(0, 100_000, &EventFilter::for_run(imported.run))
        .expect("events")
        .into_iter()
        .find_map(|e| match e.kind {
            aether_core::event::EventKind::RunImported { events, .. } => Some(events),
            _ => None,
        })
        .expect("the import writes its provenance");
    assert_eq!(
        provenance, exported.events,
        "the one durable record of what arrived said {provenance}"
    );
    assert!(provenance > 0);
}
