use std::cell::Cell;
use std::path::Path;
use std::sync::Arc;

use aether_core::id::{RunId, TaskId};
use aether_core::memory::{MemoryCandidate, MemoryKind, MemoryProvenance};
use aether_core::memory_admission::{MemoryRef, RefKind, Supporter};
use aether_store::{SqliteStore, Store, WriteGate};

use super::*;
use crate::Admission;

const MAKE_CI: [&str; 3] = [
    "Run `make ci` before pushing: it runs cargo fmt --check, clippy with -D warnings and the whole test suite.",
    "Before a push, `make ci` has to pass; it covers formatting, clippy (warnings are errors) and every test.",
    "make ci is the pre-push gate for widget-service (fmt + clippy -D warnings + tests); CI fails on anything it would catch.",
];
const RELEASE: [&str; 2] = [
    "Release procedure: bump VERSION, move the Unreleased section of CHANGELOG.md under the new version, then push a vX.Y.Z tag; the release workflow builds and publishes the archives.",
    "To cut a release edit VERSION and CHANGELOG.md, then push a tag like v2.3.0; CI then builds the release archives and publishes them.",
];
/// Alike in topic or wording, and not one fact restated.
const APART: [&str; 4] = [
    "Styling uses styled-components; put a component's styles in a styles.ts file next to it.",
    "Styling uses CSS modules (<Name>.module.css next to the component); styled-components was removed in March.",
    "Integration tests need Docker running locally; they start throwaway Postgres containers.",
    "The test suite uses an in-memory SQLite database, so no database server is needed to run the tests.",
];

fn repo() -> MemoryScope {
    MemoryScope::Repository {
        id: "widget".into(),
    }
}

/// Every fixture memory with its importance: the first of each cluster is the strongest.
fn fixture() -> Vec<(&'static str, f32)> {
    let mut all = vec![(MAKE_CI[0], 0.8), (MAKE_CI[1], 0.7), (MAKE_CI[2], 0.6)];
    all.extend([(RELEASE[0], 0.8), (RELEASE[1], 0.7)]);
    all.extend(APART.iter().map(|t| (*t, 0.7)));
    all
}

fn candidate(text: &str, importance: f32) -> MemoryCandidate {
    MemoryCandidate {
        scope: repo(),
        kind: MemoryKind::Fact,
        text: text.into(),
        structured: None,
        importance,
        confidence: 0.8,
        tags: vec![format!("t{}", (importance * 10.0) as u32)],
        provenance: MemoryProvenance::default(),
        supersedes: None,
    }
}

fn service_on(store: Arc<SqliteStore>) -> MemoryService {
    MemoryService::new(store, WriteGate::new())
}

fn seeded(order: &[(&str, f32)]) -> (Arc<SqliteStore>, MemoryService) {
    let store = Arc::new(SqliteStore::open_in_memory().expect("store"));
    let s = service_on(store.clone());
    order.iter().for_each(|(text, importance)| {
        remember(&s, text, *importance);
    });
    (store, s)
}

fn remember(s: &MemoryService, text: &str, importance: f32) -> MemoryId {
    let admission = s
        .admit(candidate(text, importance), Actor::System, None, None)
        .expect("admit");
    assert!(
        matches!(admission, Admission::Accepted { .. }),
        "{text}: {admission:?}"
    );
    admission.record().expect("admitted").id
}

fn all_records(store: &dyn Store) -> Vec<MemoryRecord> {
    store
        .memories(&MemoryQuery {
            include_superseded: true,
            include_forgotten: true,
            include_candidates: true,
            limit: 1_000,
            ..Default::default()
        })
        .expect("memories")
}

fn record(store: &dyn Store, id: MemoryId) -> MemoryRecord {
    store.memory(id).expect("read").expect("exists")
}

fn text_of(store: &dyn Store, id: MemoryId) -> String {
    record(store, id).text
}

fn current_texts(store: &dyn Store) -> Vec<String> {
    let mut texts: Vec<String> = all_records(store)
        .into_iter()
        .filter(MemoryRecord::is_current)
        .map(|r| r.text)
        .collect();
    texts.sort();
    texts
}

/// The report's clusters by text, so stores written in different orders compare.
fn clusters_by_text(store: &dyn Store, report: &ConsolidationReport) -> Vec<(String, Vec<String>)> {
    report
        .clusters
        .iter()
        .map(|c| {
            let members = c.members.iter().map(|id| text_of(store, *id)).collect();
            (text_of(store, c.survivor), members)
        })
        .collect()
}

#[test]
fn restatements_merge_into_the_strongest_and_the_rest_are_kept_superseded() {
    let (store, s) = seeded(&fixture());
    let report = s
        .consolidate(None, Actor::System, Default::default())
        .expect("consolidate");
    let found = clusters_by_text(store.as_ref(), &report);
    let expected = vec![
        (RELEASE[0].to_owned(), vec![RELEASE[1].to_owned()]),
        (
            MAKE_CI[0].to_owned(),
            vec![MAKE_CI[1].to_owned(), MAKE_CI[2].to_owned()],
        ),
    ];
    assert_eq!(found, expected);
    assert_eq!(report.scanned, fixture().len());
    assert!(
        report.abstractions.is_empty(),
        "abstraction is off by default"
    );
    for cluster in &report.clusters {
        assert_eq!(cluster.outcome, ClusterOutcome::Merged { promoted: false });
        for member in &cluster.members {
            assert_eq!(
                record(store.as_ref(), *member).superseded_by,
                Some(cluster.survivor)
            );
        }
    }
    let survivor = record(store.as_ref(), report.clusters[1].survivor);
    assert!(survivor.is_current());
    assert_eq!(
        survivor.tags,
        ["t8", "t7", "t6"],
        "tags merge in, strongest first"
    );
    assert_eq!(
        all_records(store.as_ref()).len(),
        fixture().len(),
        "no memory is lost"
    );
    assert_eq!(current_texts(store.as_ref()).len(), fixture().len() - 3);
    let again = s
        .consolidate(None, Actor::System, Default::default())
        .expect("again");
    assert!(
        again.clusters.is_empty(),
        "a consolidated store has nothing left to merge"
    );
}

#[test]
fn changed_requirements_are_never_merged() {
    for (before, after) in crate::admission_tests::CHANGED_REQUIREMENTS {
        let (store, s) = seeded(&[(before, 0.7), (after, 0.7)]);
        assert_eq!(current_texts(store.as_ref()).len(), 2, "{before} / {after}");
        let report = s
            .consolidate(Some(repo()), Actor::System, Default::default())
            .expect("run");
        assert!(report.clusters.is_empty(), "{before} merged with {after}");
    }
}

#[test]
fn the_same_memories_cluster_the_same_way_in_any_insertion_order() {
    let base = fixture();
    let mut orders = vec![base.clone(), base.iter().rev().cloned().collect()];
    for shift in [2, 5, 7] {
        let mut rotated = base.clone();
        rotated.rotate_left(shift);
        orders.push(rotated);
    }
    let (odd, even): (Vec<_>, Vec<_>) = base.iter().enumerate().partition(|(i, _)| i % 2 == 1);
    orders.push(odd.into_iter().chain(even).map(|(_, m)| *m).collect());
    let mut outcomes = Vec::new();
    for order in &orders {
        let (store, s) = seeded(order);
        let dry = ConsolidationOptions {
            dry_run: true,
            ..Default::default()
        };
        let planned = s.consolidate(None, Actor::System, dry).expect("plan");
        assert!(
            planned
                .clusters
                .iter()
                .all(|c| c.outcome == ClusterOutcome::Planned)
        );
        assert_eq!(
            current_texts(store.as_ref()).len(),
            base.len(),
            "a dry run writes nothing"
        );
        let merged = s
            .consolidate(None, Actor::System, Default::default())
            .expect("merge");
        let plan = clusters_by_text(store.as_ref(), &planned);
        assert_eq!(
            plan,
            clusters_by_text(store.as_ref(), &merged),
            "the run did what it planned"
        );
        outcomes.push((plan, current_texts(store.as_ref())));
    }
    assert!(!outcomes[0].0.is_empty());
    assert!(outcomes.iter().all(|o| o == &outcomes[0]), "{outcomes:#?}");
}

fn supported(s: &MemoryService, id: MemoryId, run: RunId, refs: Vec<MemoryRef>) {
    let mut admission = s
        .admission(&record(s.store.as_ref(), id))
        .expect("admission");
    admission.state = MemoryState::Candidate;
    admission.support(Supporter {
        task: TaskId::new(),
        run,
    });
    admission.refs = refs;
    s.store
        .commit(Commit::new().mutate(Mutation::PutMemoryAdmission(admission)))
        .expect("commit");
}

#[test]
fn support_and_refs_merge_into_the_survivor_and_can_promote_it() {
    let (store, s) = seeded(&[]);
    let strong = remember(&s, RELEASE[0], 0.8);
    let weak = remember(&s, RELEASE[1], 0.7);
    let path = |p: &str| MemoryRef {
        kind: RefKind::Path,
        value: p.into(),
    };
    supported(&s, strong, RunId::new(), vec![path("VERSION")]);
    supported(
        &s,
        weak,
        RunId::new(),
        vec![path("CHANGELOG.md"), path("VERSION")],
    );
    let report = s
        .consolidate(None, Actor::System, Default::default())
        .expect("run");
    assert_eq!(report.clusters.len(), 1);
    assert_eq!(report.clusters[0].survivor, strong);
    assert_eq!(
        report.clusters[0].outcome,
        ClusterOutcome::Merged { promoted: true }
    );
    let admission = s
        .admission(&record(store.as_ref(), strong))
        .expect("admission");
    assert_eq!((admission.tasks(), admission.independent()), (2, 2));
    assert_eq!(
        admission.state,
        MemoryState::Active,
        "two independent runs promote it"
    );
    assert_eq!(admission.refs, vec![path("CHANGELOG.md"), path("VERSION")]);
    assert_eq!(record(store.as_ref(), weak).superseded_by, Some(strong));
}

#[test]
fn a_person_s_or_a_pinned_memory_survives_and_two_never_merge() {
    let (store, s) = seeded(&[]);
    let mut stated = candidate(MAKE_CI[1], 0.5);
    stated.provenance.actor = Some(Actor::user());
    let person = s
        .admit(stated, Actor::user(), None, None)
        .expect("admit")
        .record()
        .expect("kept")
        .id;
    let agent = remember(&s, MAKE_CI[0], 0.9);
    let report = s
        .consolidate(None, Actor::System, Default::default())
        .expect("run");
    assert_eq!(
        report.clusters[0].survivor, person,
        "a person's word outranks importance"
    );
    assert_eq!(record(store.as_ref(), agent).superseded_by, Some(person));

    let (store, s) = seeded(&[(RELEASE[0], 0.8), (RELEASE[1], 0.7)]);
    for r in all_records(store.as_ref()) {
        s.pin(r.id, true, Actor::user()).expect("pin");
    }
    let report = s
        .consolidate(None, Actor::System, Default::default())
        .expect("run");
    assert!(
        report.clusters.is_empty(),
        "consolidation retires no pinned memory"
    );
}

#[test]
fn a_cluster_that_changed_under_the_plan_is_left_as_it_is() {
    let (store, s) = seeded(&fixture());
    let member = all_records(store.as_ref())
        .into_iter()
        .find(|r| r.text == RELEASE[1])
        .expect("seeded")
        .id;
    let mut fault = |step: Step| {
        if step == Step::Planned {
            s.forget(member, None, Actor::user())?;
        }
        Ok(())
    };
    let report = s
        .consolidate_with(None, Actor::System, Default::default(), &mut fault)
        .expect("run");
    let release = report
        .clusters
        .iter()
        .find(|c| c.members == [member])
        .expect("planned");
    assert!(
        matches!(&release.outcome, ClusterOutcome::Skipped { reason } if reason.contains("changed"))
    );
    assert!(record(store.as_ref(), release.survivor).is_current());
    assert_eq!(record(store.as_ref(), member).superseded_by, None);
}

/// Each planned cluster is whole or untouched, and every memory is still there.
fn assert_consistent(store: &dyn Store) {
    let all = all_records(store);
    assert_eq!(all.len(), fixture().len(), "a memory was lost");
    for r in all.iter().filter(|r| r.superseded_by.is_some()) {
        let survivor = record(store, r.superseded_by.expect("superseded"));
        assert!(
            survivor.is_current(),
            "{} points at a retired survivor",
            r.id
        );
    }
    let current = current_texts(store);
    for cluster in [&MAKE_CI[..], &RELEASE[..]] {
        let standing = cluster
            .iter()
            .filter(|t| current.contains(&t.to_string()))
            .count();
        assert!(
            standing == 1 || standing == cluster.len(),
            "half merged: {cluster:?}"
        );
    }
}

fn reopened(dir: &Path) -> (Arc<SqliteStore>, MemoryService) {
    let store = Arc::new(SqliteStore::open(dir.join("aether.db")).expect("reopen"));
    (store.clone(), service_on(store))
}

#[test]
fn a_crash_between_any_two_steps_recovers_whole_and_finishes_on_the_next_run() {
    let (reference, s) = seeded(&fixture());
    s.consolidate(None, Actor::System, Default::default())
        .expect("uninterrupted");
    let expected = current_texts(reference.as_ref());
    let points = [
        Step::Planned,
        Step::Prepared(0),
        Step::Committed(0),
        Step::Prepared(1),
        Step::Committed(1),
    ];
    for at in points {
        let dir = tempfile::tempdir().expect("tempdir");
        {
            let (_, s) = reopened(dir.path());
            fixture().iter().for_each(|(t, i)| {
                remember(&s, t, *i);
            });
            let mut crash = |step: Step| match step == at {
                true => Err(MemoryError::Invalid("injected crash".into())),
                false => Ok(()),
            };
            let result = s.consolidate_with(None, Actor::System, Default::default(), &mut crash);
            assert!(result.is_err(), "{at:?} did not stop the run");
        }
        let (store, s) = reopened(dir.path());
        assert_consistent(store.as_ref());
        s.consolidate(None, Actor::System, Default::default())
            .expect("recovery run");
        assert_consistent(store.as_ref());
        assert_eq!(
            current_texts(store.as_ref()),
            expected,
            "after a crash at {at:?}"
        );
    }
}

/// Writes "Summary: " and the start of the survivor, counting its calls.
#[derive(Default)]
struct Summary {
    calls: Cell<usize>,
}

impl Abstractor for Summary {
    fn abstract_cluster(&self, members: &[MemoryRecord]) -> Result<String> {
        assert!(members.len() >= 2, "a cluster has a survivor and a member");
        self.calls.set(self.calls.get() + 1);
        Ok(format!(
            "Summary: {}",
            members[0].text.chars().take(60).collect::<String>()
        ))
    }
}

fn budget(max_clusters: usize, max_tokens: u64) -> AbstractionBudget {
    AbstractionBudget {
        max_clusters,
        max_tokens,
        output_tokens: 32,
    }
}

#[test]
fn abstraction_over_budget_refuses_to_start_and_writes_nothing() {
    for over in [budget(1, 10_000), budget(10, 50)] {
        let (store, s) = seeded(&fixture());
        let summary = Summary::default();
        let options = ConsolidationOptions {
            dry_run: false,
            abstraction: Some(AbstractionOptions {
                abstractor: &summary,
                budget: over,
            }),
        };
        let error = s
            .consolidate(None, Actor::System, options)
            .expect_err("over budget");
        assert!(error.to_string().contains("over budget"), "{error}");
        assert_eq!(summary.calls.get(), 0);
        assert_eq!(
            current_texts(store.as_ref()).len(),
            fixture().len(),
            "nothing merged"
        );
    }
}

#[test]
fn an_abstraction_keeps_its_members_and_rolls_back_to_the_survivor() {
    let (store, s) = seeded(&fixture());
    let summary = Summary::default();
    let abstraction = || {
        Some(AbstractionOptions {
            abstractor: &summary,
            budget: budget(2, 1_000),
        })
    };
    let dry = ConsolidationOptions {
        dry_run: true,
        abstraction: abstraction(),
    };
    let planned = s.consolidate(None, Actor::System, dry).expect("dry run");
    assert!(
        planned
            .abstractions
            .iter()
            .all(|a| matches!(a.outcome, AbstractionOutcome::Planned { .. }))
    );
    assert_eq!(summary.calls.get(), 0, "a dry run calls no abstractor");
    let options = ConsolidationOptions {
        dry_run: false,
        abstraction: abstraction(),
    };
    let report = s.consolidate(None, Actor::System, options).expect("run");
    assert_eq!(summary.calls.get(), 2);
    let (cluster, outcome) = (&report.clusters[0], &report.abstractions[0].outcome);
    let AbstractionOutcome::Written { memory } = outcome.clone() else {
        panic!("not written: {outcome:?}");
    };
    let written = record(store.as_ref(), memory);
    assert!(written.is_current() && written.text.starts_with("Summary: "));
    assert_eq!(
        record(store.as_ref(), cluster.survivor).superseded_by,
        Some(memory)
    );
    let restored = s
        .rollback_abstraction(memory, Actor::user())
        .expect("rollback");
    assert_eq!(restored.id, cluster.survivor);
    assert!(record(store.as_ref(), cluster.survivor).is_current());
    assert!(record(store.as_ref(), memory).forgotten_at.is_some());
    for member in &cluster.members {
        assert_eq!(
            record(store.as_ref(), *member).superseded_by,
            Some(cluster.survivor)
        );
    }
    assert!(
        s.rollback_abstraction(memory, Actor::user()).is_err(),
        "rolled back once"
    );
    assert!(
        s.rollback_abstraction(cluster.survivor, Actor::user())
            .is_err(),
        "not an abstraction"
    );
}

#[test]
fn a_promoted_survivor_takes_over_the_memory_it_deferred_replacing() {
    let (store, s) = seeded(&[]);
    let older = remember(&s, APART[2], 0.7);
    let strong = remember(&s, RELEASE[0], 0.8);
    let weak = remember(&s, RELEASE[1], 0.7);
    let mut deferred = record(store.as_ref(), strong);
    deferred.supersedes = Some(older);
    store
        .commit(Commit::new().mutate(Mutation::PutMemory(deferred)))
        .expect("commit");
    supported(&s, strong, RunId::new(), Vec::new());
    supported(&s, weak, RunId::new(), Vec::new());
    let report = s
        .consolidate(None, Actor::System, Default::default())
        .expect("run");
    assert_eq!(
        report.clusters[0].outcome,
        ClusterOutcome::Merged { promoted: true }
    );
    assert_eq!(record(store.as_ref(), older).superseded_by, Some(strong));
    assert!(record(store.as_ref(), strong).is_current());
}

/// Pins the survivor while it writes, as a person might mid-run.
struct PinsMidway<'s> {
    service: &'s MemoryService,
}

impl Abstractor for PinsMidway<'_> {
    fn abstract_cluster(&self, members: &[MemoryRecord]) -> Result<String> {
        self.service.pin(members[0].id, true, Actor::user())?;
        Ok("Summary: cut a release by tagging it after editing VERSION.".into())
    }
}

#[test]
fn a_survivor_pinned_while_it_is_abstracted_is_not_replaced() {
    let (store, s) = seeded(&[(RELEASE[0], 0.8), (RELEASE[1], 0.7)]);
    let pins = PinsMidway { service: &s };
    let abstraction = AbstractionOptions {
        abstractor: &pins,
        budget: budget(1, 1_000),
    };
    let options = ConsolidationOptions {
        dry_run: false,
        abstraction: Some(abstraction),
    };
    let report = s.consolidate(None, Actor::System, options).expect("run");
    let outcome = &report.abstractions[0].outcome;
    assert!(
        matches!(outcome, AbstractionOutcome::Skipped { reason } if reason.contains("changed"))
    );
    let survivor = record(store.as_ref(), report.clusters[0].survivor);
    assert!(survivor.is_current() && survivor.pinned);
    assert_eq!(
        all_records(store.as_ref()).len(),
        2,
        "no abstraction was written"
    );
}

#[test]
fn a_crash_after_an_abstraction_leaves_it_whole_and_out_of_later_runs() {
    let (store, s) = seeded(&fixture());
    let summary = Summary::default();
    let abstraction = AbstractionOptions {
        abstractor: &summary,
        budget: budget(2, 1_000),
    };
    let options = ConsolidationOptions {
        dry_run: false,
        abstraction: Some(abstraction),
    };
    let mut crash = |step: Step| match step == Step::Abstracted(0) {
        true => Err(MemoryError::Invalid("injected crash".into())),
        false => Ok(()),
    };
    let result = s.consolidate_with(None, Actor::System, options, &mut crash);
    assert!(result.is_err(), "the crash stopped the run");
    let all = all_records(store.as_ref());
    assert_eq!(
        all.len(),
        fixture().len() + 1,
        "every member kept, one abstraction written"
    );
    let written = all
        .iter()
        .find(|r| abstraction::is_abstraction(r))
        .expect("written");
    let survivor = written.supersedes.expect("it replaces the survivor");
    assert_eq!(
        record(store.as_ref(), survivor).superseded_by,
        Some(written.id)
    );
    let again = s
        .consolidate(None, Actor::System, Default::default())
        .expect("again");
    assert!(
        again.clusters.is_empty(),
        "an abstraction is never merged again"
    );
    s.rollback_abstraction(written.id, Actor::user())
        .expect("rollback");
    assert!(record(store.as_ref(), survivor).is_current());
}
