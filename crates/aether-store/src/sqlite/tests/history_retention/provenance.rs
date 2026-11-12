use super::*;
use aether_core::{
    checkpoint::{Checkpoint, CheckpointDraft, CheckpointProvenance},
    id::{CheckpointId, SnapshotId},
    snapshot::{Invocation, InvocationParams, InvocationSnapshot},
};

fn snapshot(f: &Fixture) -> SnapshotId {
    let id = SnapshotId::new();
    f.mutate(Mutation::PutSnapshot(InvocationSnapshot {
        id,
        agent_id: f.agent.id,
        task_id: None,
        turn: 1,
        model: f.agent.model.clone(),
        adapter: None,
        capabilities: None,
        memory_ids: vec![],
        layers: vec![],
        messages: vec![],
        tools: vec![],
        params: InvocationParams::default(),
        token_estimate: 0,
        created_at: f.agent.created_at,
        memory_retrieval: None,
    }));
    id
}

fn checkpoint(f: &Fixture, owner: AgentId) -> Checkpoint {
    Checkpoint {
        id: CheckpointId::new(),
        agent_id: owner,
        version: 1,
        draft: CheckpointDraft {
            objective: "Preserve source provenance".into(),
            state: "Complete".into(),
            ..Default::default()
        },
        provenance: CheckpointProvenance {
            from_seq: 1,
            to_seq: 1,
            source_snapshot: None,
            compactor_model: None,
            previous: None,
            trigger: None,
        },
        created_at: f.agent.created_at,
    }
}

#[test]
fn external_invocation_preserves_its_snapshot() {
    let f = Fixture::new();
    let snapshot_id = snapshot(&f);
    let observer = agent();
    f.mutate(Mutation::UpsertAgent(observer.clone()));
    f.mutate(Mutation::PutInvocation(Invocation {
        id: InvocationId::new(),
        snapshot_id,
        agent_id: observer.id,
        turn: 1,
        model: observer.model,
        started_at: f.agent.created_at,
        finished_at: Some(f.agent.created_at),
        content: vec![],
        usage: Default::default(),
        stop_reason: None,
        error: None,
        provider_message_id: None,
    }));
    assert_eq!(
        f.expire().unwrap(),
        RunExpiry::Protected("external invocation snapshot reference")
    );
    f.assert_preserved();
    assert!(f.store.snapshot(snapshot_id).unwrap().is_some());
}

#[test]
fn external_checkpoint_preserves_each_provenance_target() {
    for source_snapshot in [true, false] {
        let f = Fixture::new();
        let observer = agent();
        f.mutate(Mutation::UpsertAgent(observer.clone()));
        let mut external = checkpoint(&f, observer.id);
        if source_snapshot {
            external.provenance.source_snapshot = Some(snapshot(&f));
        } else {
            let previous = checkpoint(&f, f.agent.id);
            external.provenance.previous = Some(previous.id);
            f.mutate(Mutation::PutCheckpoint(previous));
        }
        f.mutate(Mutation::PutCheckpoint(external));
        assert_eq!(
            f.expire().unwrap(),
            RunExpiry::Protected("external checkpoint provenance")
        );
        f.assert_preserved();
    }
}

#[test]
fn external_agent_preserves_its_checkpoint() {
    let f = Fixture::new();
    let saved = checkpoint(&f, f.agent.id);
    let mut observer = agent();
    observer.checkpoint = Some(saved.id);
    f.mutate(Mutation::PutCheckpoint(saved));
    f.mutate(Mutation::UpsertAgent(observer));
    assert_eq!(
        f.expire().unwrap(),
        RunExpiry::Protected("external agent checkpoint reference")
    );
    f.assert_preserved();
}

#[test]
fn same_run_checkpoint_provenance_does_not_prevent_expiry() {
    let f = Fixture::new();
    let snapshot_id = snapshot(&f);
    let previous = checkpoint(&f, f.agent.id);
    let mut saved = checkpoint(&f, f.agent.id);
    saved.version = 2;
    saved.provenance.source_snapshot = Some(snapshot_id);
    saved.provenance.previous = Some(previous.id);
    let saved_id = saved.id;
    f.mutate(Mutation::PutCheckpoint(previous));
    f.mutate(Mutation::PutCheckpoint(saved));
    assert!(matches!(f.expire().unwrap(), RunExpiry::Expired { .. }));
    assert!(f.store.snapshot(snapshot_id).unwrap().is_none());
    assert!(f.store.checkpoint(saved_id).unwrap().is_none());
}
