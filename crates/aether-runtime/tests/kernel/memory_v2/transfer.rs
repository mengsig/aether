//! Export and import: every field of every tier and state survives the round
//! trip, a second import changes nothing, and a dry run writes nothing.
use super::*;
use aether_core::memory_admission::{
    MemoryAdmission, MemoryRef, MemoryState, MemoryTier, RefKind, Supporter,
};
use aether_store::MemoryStaleMark;

const TIERS: [MemoryTier; 5] = [
    MemoryTier::Working,
    MemoryTier::Operational,
    MemoryTier::Architecture,
    MemoryTier::History,
    MemoryTier::Evidence,
];

/// A store holding a memory in every tier, every state (candidate, active,
/// superseded, retracted, pending proposal), a stale one and one with no
/// admission row. Returns the store's memory count.
fn populate(h: &Harness) -> usize {
    let (mut kept, implicit, mut others) = seed(h);
    kept.append(&mut others);
    let records: Vec<MemoryRecord> = kept
        .iter()
        .map(|id| h.store.memory(*id).expect("read").expect("stored"))
        .collect();
    let mut commit = Commit::new();
    for (record, tier) in records.iter().zip(TIERS.iter().cycle()) {
        let state = if *tier == MemoryTier::Working {
            MemoryState::Candidate
        } else {
            MemoryState::Active
        };
        let mut admission = MemoryAdmission::derived(record, state);
        admission.tier = *tier;
        admission.support(Supporter {
            task: aether_core::id::TaskId::new(),
            run: aether_core::id::RunId::new(),
        });
        admission.refs.push(MemoryRef {
            kind: RefKind::Doc,
            value: format!("docs/{}.md", tier.as_str()),
        });
        admission.refs.sort();
        admission.promoted_at = (state == MemoryState::Active).then(now);
        commit = commit.mutate(Mutation::PutMemoryAdmission(admission));
    }
    commit = commit.mutate(Mutation::MarkMemoryStale(MemoryStaleMark {
        memory: kept[1],
        reason: "docs/releases no longer states it".into(),
        since: now(),
        revision: Some("abc123".into()),
    }));
    h.store.commit(commit).expect("admissions");
    assert!(h.store.memory_admission(implicit).expect("read").is_none());
    let total = h.store.memory_v2_counts().expect("counts").total as usize;
    assert_eq!(total, records.len() + 1);
    total
}

fn standings(h: &Harness) -> Vec<MemoryRecord> {
    let query = |proposed| aether_store::MemoryQuery {
        include_superseded: true,
        include_forgotten: true,
        include_candidates: true,
        proposed,
        limit: 1_000,
        ..Default::default()
    };
    let mut records = h.store.memories(&query(false)).expect("read");
    records.extend(h.store.memories(&query(true)).expect("read"));
    records.sort_by_key(|record| record.id);
    h.runtime.with_standing(&mut records).expect("standing");
    records
}

fn entries(artifact: &str) -> Vec<&str> {
    artifact.lines().skip(1).collect()
}

#[tokio::test]
async fn every_tier_and_state_round_trips_and_a_second_import_changes_nothing() {
    let source = harness(Vec::new()).await;
    let total = populate(&source);
    let before = standings(&source);
    let states: BTreeSet<_> = before
        .iter()
        .map(|r| r.standing.as_ref().unwrap().state.as_str())
        .collect();
    assert_eq!(
        states.len(),
        4,
        "candidate, active, superseded, retracted: {states:?}"
    );
    let tiers: BTreeSet<_> = before
        .iter()
        .map(|r| r.standing.as_ref().unwrap().tier.as_str())
        .collect();
    assert_eq!(tiers.len(), TIERS.len());
    assert!(
        before
            .iter()
            .any(|r| r.standing.as_ref().unwrap().stale.is_some())
    );
    assert!(before.iter().any(MemoryRecord::is_proposed));

    let export = source
        .runtime
        .export_memories(None, &Actor::user())
        .expect("export");
    assert_eq!(export.records as usize, total);
    assert_eq!(entries(&export.artifact).len(), total);

    let target = harness(Vec::new()).await;
    let dry = target
        .runtime
        .import_memories(&export.artifact, true, Actor::user())
        .expect("dry run");
    assert_eq!(
        (dry.inserted as usize, dry.unchanged, dry.conflicts.len()),
        (total, 0, 0)
    );
    assert_eq!(dry.digest, export.digest);
    assert_eq!(
        target.store.memory_v2_counts().expect("counts").total,
        0,
        "a dry run writes nothing"
    );
    assert!(
        target
            .runtime
            .memory_migration_status()
            .expect("status")
            .last_import
            .is_none()
    );

    let applied = target
        .runtime
        .import_memories(&export.artifact, false, Actor::user())
        .expect("import");
    assert_eq!((applied.inserted as usize, applied.unchanged), (total, 0));
    assert_eq!(standings(&target), before, "every field, standing included");
    let again = target
        .runtime
        .export_memories(None, &Actor::user())
        .expect("export");
    assert_eq!(entries(&again.artifact), entries(&export.artifact));
    let counts = |h: &Harness| h.store.memory_v2_counts().expect("counts");
    let (from, to) = (counts(&source), counts(&target));
    assert_eq!(
        (from.total, from.admitted, from.candidates, from.stale),
        (to.total, to.admitted, to.candidates, to.stale)
    );

    let second = target
        .runtime
        .import_memories(&export.artifact, false, Actor::user())
        .expect("again");
    assert_eq!((second.inserted, second.unchanged as usize), (0, total));
    assert!(second.conflicts.is_empty());
    assert_eq!(standings(&target), before, "idempotent");
    let last = target
        .runtime
        .memory_migration_status()
        .expect("status")
        .last_import;
    assert_eq!(
        last.map(|l| (l.inserted, l.unchanged as usize)),
        Some((0, total))
    );
    source.runtime.shutdown().await;
    target.runtime.shutdown().await;
}

#[tokio::test]
async fn a_differing_memory_is_a_conflict_and_is_never_overwritten() {
    let source = harness(Vec::new()).await;
    let total = populate(&source);
    let export = source
        .runtime
        .export_memories(None, &Actor::user())
        .expect("export");
    let target = harness(Vec::new()).await;
    target
        .runtime
        .import_memories(&export.artifact, false, Actor::user())
        .expect("import");
    let pinned = standings(&target)
        .into_iter()
        .find(|r| r.is_current() && !r.is_proposed())
        .expect("a current memory");
    let local = target
        .runtime
        .pin_memory(pinned.id, true, Actor::user())
        .expect("pin");
    let report = target
        .runtime
        .import_memories(&export.artifact, false, Actor::user())
        .expect("import");
    assert_eq!(report.conflicts.len(), 1, "{report:?}");
    assert_eq!(report.conflicts[0].memory, pinned.id);
    assert_eq!((report.unchanged as usize, report.inserted), (total - 1, 0));
    let kept = target
        .store
        .memory(pinned.id)
        .expect("read")
        .expect("stored");
    assert_eq!(kept, local, "the local edit stands");
    source.runtime.shutdown().await;
    target.runtime.shutdown().await;
}

#[tokio::test]
async fn a_bad_artifact_is_refused_by_line_and_writes_nothing() {
    let source = harness(Vec::new()).await;
    populate(&source);
    let export = source
        .runtime
        .export_memories(None, &Actor::user())
        .expect("export");
    let lines: Vec<&str> = export.artifact.lines().collect();
    let header: serde_json::Value = serde_json::from_str(lines[0]).expect("header");
    let mut newer = header.clone();
    newer["version"] = json!(2);
    let mut entry: serde_json::Value = serde_json::from_str(lines[1]).expect("entry");
    entry["record"]["standing"] = json!({"tier":"working","state":"active",
        "support":{"tasks":0,"independent":0},"verification":"inferred","refs":[]});
    let mut mismatch: serde_json::Value = serde_json::from_str(lines[1]).expect("entry");
    assert!(
        mismatch["admission"].is_object(),
        "the oldest memory has an admission row"
    );
    mismatch["admission"]["memory"] = json!(MemoryId::new());
    let body = |rest: &[String]| {
        let mut all = vec![lines[0].to_owned()];
        all.extend(rest.iter().cloned());
        all.join("\n")
    };
    let rest: Vec<String> = lines[1..].iter().map(|l| (*l).to_owned()).collect();
    let cases = [
        ("", "no header"),
        (&*format!("{newer}\n{}", rest.join("\n")), "version"),
        (&*body(&rest[1..]), "header counts"),
        (
            &*body(&[rest.clone(), vec!["{".into()]].concat()),
            "malformed",
        ),
        (
            &*body(&[vec![rest[0].clone()], rest[..rest.len() - 1].to_vec()].concat()),
            "twice",
        ),
        (
            &*body(&[vec![entry.to_string()], rest[1..].to_vec()].concat()),
            "standing",
        ),
        (
            &*body(&[vec![mismatch.to_string()], rest[1..].to_vec()].concat()),
            "admission names",
        ),
    ];
    let target = harness(Vec::new()).await;
    for (artifact, expected) in cases {
        let refused = target
            .runtime
            .import_memories(artifact, false, Actor::user());
        let Err(aether_runtime::RuntimeError::Invalid(message)) = refused else {
            panic!("{expected}: {refused:?}");
        };
        assert!(
            message.contains(expected) || message.contains("is empty"),
            "{expected}: {message}"
        );
    }
    assert_eq!(target.store.memory_v2_counts().expect("counts").total, 0);
    let agent = Actor::Agent { id: AgentId::new() };
    assert!(
        target
            .runtime
            .import_memories(&export.artifact, true, agent)
            .is_err()
    );
    source.runtime.shutdown().await;
    target.runtime.shutdown().await;
}
