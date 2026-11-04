//! Candidate admission through the service's public path: a candidate is
//! stored but never injected, and becomes active once two independent tasks
//! (distinct runs) state it, or an operator promotes it.

use std::sync::Arc;

use aether_context::TokenEstimator;
use aether_core::event::{Actor, EventKind};
use aether_core::id::{AgentId, MemoryId, RunId, TaskId};
use aether_core::memory::{MemoryCandidate, MemoryKind, MemoryProvenance, MemoryScope};
use aether_core::memory_admission::{MemoryState, Verification};
use aether_core::task::Task;
use aether_core::time::now;
use aether_memory::{Admission, AdmissionMode, AdmissionPolicy, MemoryService, RetrievalRequest};
use aether_store::{Commit, EventFilter, Mutation, SqliteStore, Store, WriteGate};

const FACT: &str = "Release builds sign the daemon binary before packaging";

struct Fixture {
    store: Arc<SqliteStore>,
    candidates: MemoryService,
    agent: AgentId,
}

fn fixture() -> Fixture {
    let store = Arc::new(SqliteStore::open_in_memory().expect("store"));
    let policy = AdmissionPolicy {
        mode: AdmissionMode::Candidate,
        ..Default::default()
    };
    let candidates = MemoryService::new(store.clone(), WriteGate::new()).with_policy(policy);
    Fixture {
        store,
        candidates,
        agent: AgentId::new(),
    }
}

impl Fixture {
    fn task(&self, run: RunId) -> TaskId {
        let task = Task::new("ship the release", Actor::user(), run, now());
        let id = task.id;
        self.store
            .commit(Commit::new().mutate(Mutation::UpsertTask(task)))
            .expect("task");
        id
    }

    fn remember(&self, service: &MemoryService, text: &str, task: Option<TaskId>) -> Admission {
        let candidate = MemoryCandidate {
            scope: repo(),
            kind: MemoryKind::Fact,
            text: text.into(),
            structured: None,
            importance: 0.7,
            confidence: 0.8,
            tags: vec!["release".into()],
            provenance: MemoryProvenance {
                agent: Some(self.agent),
                task,
                ..Default::default()
            },
            supersedes: None,
        };
        service
            .admit(candidate, Actor::agent(self.agent), Some(self.agent), task)
            .expect("admit")
    }

    /// What a turn puts in front of the model: retrieval, then the memory layer.
    fn injected(&self, service: &MemoryService) -> (String, Vec<MemoryId>) {
        let retrieved = service
            .retrieve(&RetrievalRequest {
                scope: Some(repo()),
                role: Some("builder".into()),
                agent: Some(self.agent),
                query: Some("how are release builds signed and packaged".into()),
                limit: 6,
                ..Default::default()
            })
            .expect("retrieve");
        let records: Vec<_> = retrieved.into_iter().map(|r| r.record).collect();
        let (text, ids) = aether_context::memory_layer(&records, &TokenEstimator, 4_000);
        (text.unwrap_or_default(), ids)
    }

    fn state(&self, id: MemoryId) -> MemoryState {
        let record = self.store.memory(id).expect("read").expect("stored");
        let admission = self.candidates.admission(&record).expect("admission");
        MemoryState::of(&record, admission.state)
    }

    fn events_named(&self, name: &str) -> usize {
        let events = self
            .store
            .events(0, 1_000, &EventFilter::default())
            .expect("events");
        events
            .iter()
            .filter(|e| matches!(&e.kind, EventKind::Custom { name: n, .. } if n == name))
            .count()
    }
}

fn repo() -> MemoryScope {
    MemoryScope::Repository {
        id: "aether".into(),
    }
}

fn admitted(admission: &Admission) -> MemoryId {
    match admission {
        Admission::Accepted { record } => record.id,
        other => panic!("expected a new record, got {other:?}"),
    }
}

#[test]
fn a_candidate_is_stored_but_never_injected() {
    let f = fixture();
    let stated = MemoryCandidate {
        scope: repo(),
        kind: MemoryKind::Constraint,
        text: "Release builds are packaged only from a signed tag".into(),
        structured: None,
        importance: 0.8,
        confidence: 0.9,
        tags: vec!["release".into()],
        provenance: MemoryProvenance {
            actor: Some(Actor::user()),
            ..Default::default()
        },
        supersedes: None,
    };
    let operator = admitted(
        &f.candidates
            .admit(stated, Actor::user(), None, None)
            .unwrap(),
    );
    let id = admitted(&f.remember(&f.candidates, FACT, Some(f.task(RunId::new()))));
    assert_eq!(
        (f.state(operator), f.state(id)),
        (MemoryState::Active, MemoryState::Candidate)
    );
    assert!(
        f.store.memory(id).unwrap().unwrap().is_current(),
        "stored and live"
    );

    let (text, ids) = f.injected(&f.candidates);
    assert_eq!(
        ids,
        vec![operator],
        "the operator's word is injected, the candidate is not"
    );
    assert!(
        text.contains("signed tag") && !text.contains(FACT),
        "{text}"
    );

    let immediate = MemoryService::new(f.store.clone(), WriteGate::new());
    let (_, ids) = f.injected(&immediate);
    assert!(
        !ids.contains(&id),
        "the mode decides admission, not what retrieval shows"
    );
}

#[test]
fn the_same_task_twice_does_not_promote() {
    let f = fixture();
    let task = f.task(RunId::new());
    let id = admitted(&f.remember(&f.candidates, FACT, Some(task)));
    let again = f.remember(&f.candidates, FACT, Some(task));
    assert!(matches!(again, Admission::Rejected { .. }), "{again:?}");
    let record = f.store.memory(id).unwrap().unwrap();
    let admission = f.candidates.admission(&record).unwrap();
    assert_eq!((admission.tasks(), admission.independent()), (1, 1));
    assert_eq!(f.state(id), MemoryState::Candidate);
    assert!(!f.injected(&f.candidates).1.contains(&id));
}

#[test]
fn two_tasks_of_one_run_are_one_source() {
    let f = fixture();
    let run = RunId::new();
    let id = admitted(&f.remember(&f.candidates, FACT, Some(f.task(run))));
    let second = f.remember(&f.candidates, FACT, Some(f.task(run)));
    assert!(
        matches!(&second, Admission::Reinforced { record, promoted: false } if record.id == id),
        "{second:?}"
    );
    let record = f.store.memory(id).unwrap().unwrap();
    let admission = f.candidates.admission(&record).unwrap();
    assert_eq!((admission.tasks(), admission.independent()), (2, 1));
    assert_eq!(f.state(id), MemoryState::Candidate);
    assert!(!f.injected(&f.candidates).1.contains(&id));
}

#[test]
fn two_independent_tasks_promote_a_candidate_into_the_prompt() {
    let f = fixture();
    let id = admitted(&f.remember(&f.candidates, FACT, Some(f.task(RunId::new()))));
    assert!(!f.injected(&f.candidates).1.contains(&id));

    let second = f.remember(&f.candidates, FACT, Some(f.task(RunId::new())));
    assert!(
        matches!(&second, Admission::Reinforced { record, promoted: true } if record.id == id),
        "{second:?}"
    );
    let record = f.store.memory(id).unwrap().unwrap();
    let admission = f.candidates.admission(&record).unwrap();
    assert_eq!(admission.state, MemoryState::Active);
    assert_eq!((admission.tasks(), admission.independent()), (2, 2));
    assert_eq!(admission.verification, Verification::Observed);
    assert!(admission.promoted_at.is_some());
    assert_eq!(f.events_named("memory.promoted"), 1);

    let (text, ids) = f.injected(&f.candidates);
    assert_eq!(ids, vec![id]);
    assert!(text.contains(FACT), "{text}");

    let third = f.remember(&f.candidates, FACT, Some(f.task(RunId::new())));
    assert!(
        matches!(
            third,
            Admission::Reinforced {
                promoted: false,
                ..
            }
        ),
        "{third:?}"
    );
    assert_eq!(f.events_named("memory.promoted"), 1, "promoted once");
}

#[test]
fn a_candidate_replaces_an_active_memory_only_once_promoted() {
    let f = fixture();
    let immediate = MemoryService::new(f.store.clone(), WriteGate::new());
    let old = admitted(&f.remember(&immediate, FACT, Some(f.task(RunId::new()))));
    assert_eq!(f.state(old), MemoryState::Active);

    let correction = MemoryCandidate {
        scope: repo(),
        kind: MemoryKind::Fact,
        text: "Release builds sign the daemon binary after packaging".into(),
        structured: None,
        importance: 0.7,
        confidence: 0.8,
        tags: vec!["release".into()],
        provenance: MemoryProvenance::default(),
        supersedes: Some(old),
    };
    let task = f.task(RunId::new());
    let new = admitted(
        &f.candidates
            .admit(correction, Actor::agent(f.agent), Some(f.agent), Some(task))
            .expect("admit"),
    );
    assert_eq!(
        (f.state(old), f.state(new)),
        (MemoryState::Active, MemoryState::Candidate)
    );
    assert_eq!(
        f.injected(&f.candidates).1,
        vec![old],
        "the model still sees the old one"
    );

    assert!(
        f.candidates.promote(new, Actor::agent(f.agent)).is_err(),
        "operators only"
    );
    let promoted = f.candidates.promote(new, Actor::user()).expect("promote");
    assert_eq!(promoted.verification, Verification::Verified);
    assert_eq!(
        (f.state(old), f.state(new)),
        (MemoryState::Superseded, MemoryState::Active)
    );
    assert_eq!(f.injected(&f.candidates).1, vec![new]);
    assert_eq!(
        f.candidates.promote(new, Actor::user()).unwrap(),
        promoted,
        "idempotent"
    );
}

#[test]
fn immediate_admission_is_active_at_once_and_still_counts_support() {
    let f = fixture();
    let immediate = MemoryService::new(f.store.clone(), WriteGate::new());
    let id = admitted(&f.remember(&immediate, FACT, Some(f.task(RunId::new()))));
    assert_eq!(f.state(id), MemoryState::Active);
    assert_eq!(f.injected(&immediate).1, vec![id]);
    let again = f.remember(&immediate, FACT, Some(f.task(RunId::new())));
    assert!(
        matches!(
            again,
            Admission::Reinforced {
                promoted: false,
                ..
            }
        ),
        "{again:?}"
    );
    let record = f.store.memory(id).unwrap().unwrap();
    assert_eq!(immediate.admission(&record).unwrap().independent(), 2);
}
