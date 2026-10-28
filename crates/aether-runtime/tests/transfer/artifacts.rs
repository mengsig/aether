use super::*;
use aether_core::artifact::{Artifact, ArtifactKind};
use aether_core::event::EventKind;
use aether_core::transfer::Record;
use aether_runtime::artifacts::ArtifactPublication;
use sha2::Digest as _;

async fn artifact_run(bytes: &[u8]) -> (Daemon, AgentId, Artifact) {
    let source = daemon(Vec::new()).await;
    let (agent, _) = source
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                ..Default::default()
            },
            Actor::user(),
        )
        .unwrap();
    source
        .runtime
        .cancel_agent(agent.id, None, Actor::user())
        .unwrap();
    let artifact = publish(&source, agent.id, bytes).await;
    (source, agent.id, artifact)
}

async fn publish(source: &Daemon, agent: AgentId, bytes: &[u8]) -> Artifact {
    source
        .runtime
        .publish_artifact(
            ArtifactPublication {
                kind: ArtifactKind::Result,
                name: "result".into(),
                summary: String::new(),
                content_type: Some("text/plain".into()),
                bytes: bytes.to_vec(),
                agent,
                task: None,
            },
            Actor::agent(agent),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn deduplicated_artifacts_travel_with_their_records_and_bytes() {
    let bytes = b"identical result";
    let (source, agent, first) = artifact_run(bytes).await;
    let second = publish(&source, agent, bytes).await;
    assert_eq!(first.path, second.path);
    let run = source.store.agent(agent).unwrap().unwrap().run;
    assert!(source.store.events(0, 100, &EventFilter::for_run(run)).unwrap().iter().any(|e| {
        matches!(e.kind, EventKind::ArtifactDeduplicated { artifact, .. } if artifact == second.id)
    }));
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("run.aether");
    let exported = transfer::export_run(
        source.store.as_ref(),
        run,
        &file,
        &aether_tools::Redactor::default(),
    )
    .unwrap();
    assert_eq!(exported.artifacts, 2);
    assert_eq!(exported.artifact_payloads, 2);
    let target = daemon(Vec::new()).await;
    let imported = transfer::import_run(&target.runtime, &file).unwrap();
    assert_eq!(imported.artifacts, 2);
    let records = target.store.live_artifacts().unwrap();
    assert_eq!(records.len(), 2);
    for record in records {
        assert_eq!(
            target.store.read_artifact_bytes(record.id).unwrap(),
            Some(bytes.to_vec())
        );
    }
}

#[tokio::test]
async fn redacted_artifact_metadata_matches_payload_and_cannot_dedup_raw_secrets() {
    let raw = b"token=sk-ant-abcdefghijklmnopqrstuvwxyz0123456789";
    let (source, agent, original) = artifact_run(raw).await;
    let twin = publish(&source, agent, raw).await;
    let run = source.store.agent(agent).unwrap().unwrap().run;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("run.aether");
    transfer::export_run(
        source.store.as_ref(),
        run,
        &file,
        &aether_tools::Redactor::default(),
    )
    .unwrap();
    let text = std::fs::read_to_string(&file).unwrap();
    let records: Vec<Record> = text
        .lines()
        .skip(1)
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let exported = records
        .iter()
        .find_map(|record| match record {
            Record::Artifact(artifact) => Some(artifact),
            _ => None,
        })
        .unwrap();
    let payload = records
        .iter()
        .find_map(|record| match record {
            Record::ArtifactBytes { base64, .. } => Some(
                base64::Engine::decode(&base64::engine::general_purpose::STANDARD, base64).unwrap(),
            ),
            _ => None,
        })
        .unwrap();
    assert_ne!(payload, raw);
    assert_eq!(exported.size_bytes, payload.len() as u64);
    assert_eq!(
        exported.digest,
        Some(hex::encode(sha2::Sha256::digest(&payload)))
    );

    let mut created = false;
    let mut deduplicated = false;
    for record in &records {
        if let Record::Event(event) = record {
            match &event.kind {
                EventKind::ArtifactCreated { artifact } => {
                    assert_eq!(artifact.size_bytes, exported.size_bytes);
                    assert_eq!(artifact.digest, exported.digest);
                    created = true;
                }
                EventKind::ArtifactDeduplicated {
                    artifact, digest, ..
                } => {
                    assert_eq!(*artifact, twin.id);
                    assert_eq!(Some(digest), exported.digest.as_ref());
                    deduplicated = true;
                }
                _ => {}
            }
        }
    }
    assert!(created && deduplicated);

    // Old exports advertised raw metadata beside redacted bytes. Import must repair them too.
    let legacy = text
        .lines()
        .map(|line| {
            let mut record: serde_json::Value = serde_json::from_str(line).unwrap();
            if record.get("aether_export").is_some() {
                record["aether_export"] = json!(1);
            }
            if record["record"] == "artifact" {
                record["size_bytes"] = json!(original.size_bytes);
                record["digest"] = json!(original.digest);
            }
            serde_json::to_string(&record).unwrap()
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    std::fs::write(&file, legacy).unwrap();
    let target = daemon(Vec::new()).await;
    let imported = transfer::import_run(&target.runtime, &file).unwrap();
    let stored = target.store.live_artifacts().unwrap().pop().unwrap();
    assert_eq!(stored.size_bytes, payload.len() as u64);
    assert_eq!(stored.digest, exported.digest);
    assert_eq!(
        target.store.read_artifact_bytes(stored.id).unwrap(),
        Some(payload)
    );
    let imported_agent = target
        .store
        .agents(&AgentFilter {
            run: Some(imported.run),
            ..Default::default()
        })
        .unwrap()[0]
        .id;
    let fresh = publish(&target, imported_agent, raw).await;
    assert_ne!(fresh.path, stored.path);
    assert_eq!(
        target.store.read_artifact_bytes(fresh.id).unwrap(),
        Some(raw.to_vec())
    );
}

#[tokio::test]
async fn invalid_ulid_shaped_prose_survives_run_import() {
    let prose = "agt_IIIIIIIIIIIIIIIIIIIIIIIIII";
    let (source, agent, _) = artifact_run(b"result").await;
    let mut record = source.store.agent(agent).unwrap().unwrap();
    let run = record.run;
    record.name = prose.into();
    source
        .store
        .commit(aether_store::Commit::new().mutate(aether_store::Mutation::UpsertAgent(record)))
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("run.aether");
    transfer::export_run(
        source.store.as_ref(),
        run,
        &file,
        &aether_tools::Redactor::default(),
    )
    .unwrap();
    let target = daemon(Vec::new()).await;
    transfer::import_run(&target.runtime, &file).unwrap();
    assert_eq!(
        target.store.agents(&AgentFilter::default()).unwrap()[0].name,
        prose
    );
}

#[tokio::test]
async fn imported_legacy_dedup_events_without_records_can_be_exported_again() {
    let (source, agent, _) = artifact_run(b"identical").await;
    let missing = publish(&source, agent, b"identical").await;
    let run = source.store.agent(agent).unwrap().unwrap().run;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("legacy.aether");
    transfer::export_run(
        source.store.as_ref(),
        run,
        &file,
        &aether_tools::Redactor::default(),
    )
    .unwrap();
    let text = std::fs::read_to_string(&file).unwrap();
    let mut lines = text.lines();
    let mut legacy = lines.next().unwrap().to_owned() + "\n";
    for line in lines {
        let record: Record = serde_json::from_str(line).unwrap();
        if matches!(&record, Record::Artifact(a) if a.id == missing.id)
            || matches!(&record, Record::ArtifactBytes { id, .. } if *id == missing.id)
        {
            continue;
        }
        legacy.push_str(line);
        legacy.push('\n');
    }
    std::fs::write(&file, legacy).unwrap();
    let target = daemon(Vec::new()).await;
    let imported = transfer::import_run(&target.runtime, &file).unwrap();
    let output = dir.path().join("reexport.aether");
    let exported = transfer::export_run(
        target.store.as_ref(),
        imported.run,
        &output,
        &aether_tools::Redactor::default(),
    )
    .unwrap();
    assert_eq!(exported.artifacts, 1);
    let records: Vec<Record> = std::fs::read_to_string(output)
        .unwrap()
        .lines()
        .skip(1)
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(records.iter().any(|record| matches!(record,
        Record::Event(event) if matches!(&event.kind, EventKind::ArtifactDeduplicated { digest, .. } if digest.is_empty())
    )));
}

#[tokio::test]
async fn unavailable_artifacts_export_no_payload_or_secret_metadata() {
    for evicted in [false, true] {
        let raw = b"token=sk-ant-abcdefghijklmnopqrstuvwxyz0123456789";
        let (source, agent, first) = artifact_run(raw).await;
        let twin = publish(&source, agent, raw).await;
        for id in [first.id, twin.id] {
            source.store.evict_artifact_bytes(id).unwrap();
        }
        if evicted {
            // Model an interrupted unlink: evicted rows must not export leftover bytes.
            source.store.write_artifact_bytes(first.id, raw).unwrap();
        } else {
            for id in [first.id, twin.id] {
                let mut artifact = source.store.artifact(id).unwrap().unwrap();
                artifact.evicted_at = None;
                source
                    .store
                    .commit(
                        aether_store::Commit::new()
                            .mutate(aether_store::Mutation::PutArtifact(artifact)),
                    )
                    .unwrap();
            }
        }
        let run = source.store.agent(agent).unwrap().unwrap().run;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("run.aether");
        let exported = transfer::export_run(
            source.store.as_ref(),
            run,
            &file,
            &aether_tools::Redactor::default(),
        )
        .unwrap();
        assert_eq!(exported.artifacts, 2);
        assert_eq!(exported.artifact_payloads, 0);
        let records: Vec<Record> = std::fs::read_to_string(&file)
            .unwrap()
            .lines()
            .skip(1)
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        for record in records {
            match record {
                Record::Artifact(artifact) => {
                    assert_eq!(artifact.size_bytes, 0);
                    assert_eq!(artifact.digest, None);
                    assert_eq!(artifact.evicted_at.is_some(), evicted);
                }
                Record::Event(event) => match event.kind {
                    EventKind::ArtifactCreated { artifact } => {
                        assert_eq!(artifact.size_bytes, 0);
                        assert_eq!(artifact.digest, None);
                        assert_eq!(artifact.evicted_at.is_some(), evicted);
                    }
                    EventKind::ArtifactDeduplicated { digest, .. } => assert!(digest.is_empty()),
                    _ => {}
                },
                Record::ArtifactBytes { .. } => panic!("evicted bytes were exported"),
                _ => {}
            }
        }
        let target = daemon(Vec::new()).await;
        let imported = transfer::import_run(&target.runtime, &file).unwrap();
        assert_eq!(imported.artifact_payloads, 0);
        assert_eq!(
            target.store.live_artifacts().unwrap().len(),
            if evicted { 0 } else { 2 }
        );
    }
}

#[tokio::test]
async fn disk_import_preserves_legacy_evicted_payloads_until_pending_unlink() {
    for evicted in [false, true] {
        let bytes = b"legacy payload";
        let (source, agent, _) = artifact_run(bytes).await;
        let run = source.store.agent(agent).unwrap().unwrap().run;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("legacy.aether");
        transfer::export_run(
            source.store.as_ref(),
            run,
            &file,
            &aether_tools::Redactor::default(),
        )
        .unwrap();
        if evicted {
            let archive = std::fs::read_to_string(&file).unwrap();
            let mut lines = archive.lines();
            let mut legacy = lines.next().unwrap().to_owned() + "\n";
            for line in lines {
                let mut record: Record = serde_json::from_str(line).unwrap();
                if let Record::Artifact(artifact) = &mut record {
                    // Older exports could carry bytes left behind by an interrupted eviction.
                    artifact.evicted_at = Some(now());
                }
                legacy.push_str(&serde_json::to_string(&record).unwrap());
                legacy.push('\n');
            }
            std::fs::write(&file, legacy).unwrap();
        }
        let database = dir.path().join("target.db");
        let target = restart_over(Arc::new(SqliteStore::open(&database).unwrap())).await;
        let imported = transfer::import_run(&target.runtime, &file).unwrap();
        assert_eq!(imported.artifact_payloads, 1);
        let artifacts = target
            .store
            .artifacts(&aether_store::ArtifactFilter::default())
            .unwrap();
        assert_eq!(artifacts.len(), 1);
        let artifact = &artifacts[0];
        assert_eq!(artifact.evicted_at.is_some(), evicted);
        assert_eq!(
            target.store.read_artifact_bytes(artifact.id).unwrap(),
            Some(bytes.to_vec())
        );
        target.runtime.shutdown().await;
        drop(target);
        let reopened = SqliteStore::open(database).unwrap();
        assert_eq!(
            reopened.read_artifact_bytes(artifact.id).unwrap(),
            (!evicted).then(|| bytes.to_vec())
        );
    }
}

#[tokio::test]
async fn authenticated_exports_reject_tampered_or_missing_payloads_before_commit() {
    let (source, agent, _) = artifact_run(b"verified content").await;
    let run = source.store.agent(agent).unwrap().unwrap().run;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("run.aether");
    transfer::export_run(
        source.store.as_ref(),
        run,
        &file,
        &aether_tools::Redactor::default(),
    )
    .unwrap();
    let original = std::fs::read_to_string(&file).unwrap();
    for missing in [false, true] {
        let mut lines = original.lines();
        let mut archive = lines.next().unwrap().to_owned() + "\n";
        for line in lines {
            let mut record: Record = serde_json::from_str(line).unwrap();
            if let Record::ArtifactBytes { base64, .. } = &mut record {
                if missing {
                    continue;
                }
                *base64 = base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    b"tampered content",
                );
            }
            archive.push_str(&serde_json::to_string(&record).unwrap());
            archive.push('\n');
        }
        std::fs::write(&file, archive).unwrap();
        let target = daemon(Vec::new()).await;
        let before = target
            .store
            .events(0, 100, &EventFilter::default())
            .unwrap();
        let error = transfer::import_run(&target.runtime, &file)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(if missing {
                "no payload"
            } else {
                "does not match"
            }),
            "{error}"
        );
        assert!(
            target
                .store
                .agents(&AgentFilter::default())
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            target
                .store
                .events(0, 100, &EventFilter::default())
                .unwrap(),
            before
        );
        assert!(
            target
                .store
                .artifacts(&aether_store::ArtifactFilter::default())
                .unwrap()
                .is_empty()
        );
    }
}

#[tokio::test]
async fn legacy_redacted_payload_repairs_the_record_and_created_event() {
    let (source, agent, _) = artifact_run(b"redacted legacy content").await;
    let run = source.store.agent(agent).unwrap().unwrap().run;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("legacy.aether");
    transfer::export_run(
        source.store.as_ref(),
        run,
        &file,
        &aether_tools::Redactor::default(),
    )
    .unwrap();
    let original = std::fs::read_to_string(&file).unwrap();
    let mut lines = original.lines();
    let mut manifest: aether_core::transfer::Manifest =
        serde_json::from_str(lines.next().unwrap()).unwrap();
    manifest.aether_export = 1;
    let mut archive = serde_json::to_string(&manifest).unwrap() + "\n";
    for line in lines {
        let mut record: Record = serde_json::from_str(line).unwrap();
        let artifact = match &mut record {
            Record::Artifact(artifact) => Some(artifact),
            Record::Event(event) => match &mut event.kind {
                EventKind::ArtifactCreated { artifact } => Some(artifact),
                _ => None,
            },
            _ => None,
        };
        if let Some(artifact) = artifact {
            artifact.size_bytes = 999;
            artifact.digest = Some("stale raw digest".into());
        }
        archive.push_str(&serde_json::to_string(&record).unwrap());
        archive.push('\n');
    }
    std::fs::write(&file, archive).unwrap();
    let target = daemon(Vec::new()).await;
    let imported = transfer::import_run(&target.runtime, &file).unwrap();
    let artifact = target.store.live_artifacts().unwrap().pop().unwrap();
    assert_eq!(artifact.size_bytes, b"redacted legacy content".len() as u64);
    assert_eq!(
        artifact.digest,
        Some(hex::encode(sha2::Sha256::digest(
            b"redacted legacy content"
        )))
    );
    let created = target
        .store
        .events(0, 100, &EventFilter::for_run(imported.run))
        .unwrap()
        .into_iter()
        .find_map(|event| match event.kind {
            EventKind::ArtifactCreated { artifact } => Some(artifact),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        (created.size_bytes, created.digest),
        (artifact.size_bytes, artifact.digest)
    );
}
