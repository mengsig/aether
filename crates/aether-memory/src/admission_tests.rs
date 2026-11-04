//! Admission against the real service: only the same text, trimmed, is a
//! duplicate, however many words a changed note shares with the old one.

use super::*;
use aether_core::memory::{MemoryProvenance, MemorySource};
use aether_store::SqliteStore;

fn service() -> MemoryService {
    MemoryService::new(
        Arc::new(SqliteStore::open_in_memory().expect("store")),
        WriteGate::new(),
    )
}

fn repo() -> MemoryScope {
    MemoryScope::Repository {
        id: "aether".into(),
    }
}

fn candidate(text: &str, importance: f32) -> MemoryCandidate {
    MemoryCandidate {
        scope: repo(),
        kind: MemoryKind::Constraint,
        text: text.into(),
        structured: None,
        importance,
        confidence: 0.9,
        tags: vec![],
        provenance: MemoryProvenance::default(),
        supersedes: None,
    }
}

fn admitted(s: &MemoryService, c: MemoryCandidate, actor: Actor) -> MemoryId {
    let admission = s.admit(c, actor, None, None).expect("admit");
    admission.record().expect("admitted").id
}

fn current(s: &MemoryService, id: MemoryId) -> bool {
    s.store
        .memory(id)
        .expect("get")
        .expect("exists")
        .is_current()
}

fn outcome(admission: &Admission) -> String {
    match admission {
        Admission::Accepted { .. } => "accepted".into(),
        Admission::Superseded { old, .. } => format!("superseded {old}"),
        Admission::Rejected { reason } => format!("rejected: {reason}"),
        Admission::Reinforced { record, promoted } => {
            format!("reinforced {} {promoted}", record.id)
        }
    }
}

/// Each pair shares every word, or all but one, that token overlap counts.
const CHANGED_REQUIREMENTS: [(&str, &str); 17] = [
    (
        "Run cargo fmt before committing changes to the workspace",
        "Do not run cargo fmt before committing changes to the workspace",
    ),
    (
        "Run cargo fmt before committing changes to the workspace",
        "Never run cargo fmt before committing changes to the workspace",
    ),
    (
        "Keep every function under 60 lines in this repository",
        "Keep every function under 40 lines in this repository",
    ),
    (
        "Limit the test harness to 120 parallel jobs on shared build hosts during nightly runs",
        "Limit the test harness to 100 parallel jobs on shared build hosts during nightly runs",
    ),
    (
        "The whole build runs through scripts/ci.sh in this repository",
        "The whole build runs through scripts/cd.sh in this repository",
    ),
    (
        "Release notes for the client are written in docs/v1/notes.md",
        "Release notes for the client are written in docs/v2/notes.md",
    ),
    (
        "Project documentation for every crate lives in docs/README.md",
        "Project documentation for every crate lives in docs/readme.md",
    ),
    (
        "Load the deployment secrets from .env before starting the daemon",
        "Load the deployment secrets from env before starting the daemon",
    ),
    (
        "Read the daemon settings from ./config/settings at startup",
        "Read the daemon settings from /config/settings at startup",
    ),
    (
        "Generated defaults are always written to config/settings.",
        "Generated defaults are always written to config/settings",
    ),
    (
        "Retry the flaky upload only when the queue depth is <=40",
        "Retry the flaky upload only when the queue depth is >=40",
    ),
    (
        "Retry the flaky upload only when the queue depth is < 40",
        "Retry the flaky upload only when the queue depth is > 40",
    ),
    (
        "Do not run cargo fmt and run cargo test before committing",
        "Run cargo fmt and do not run cargo test before committing",
    ),
    (
        "Do not run cargo fmt and run cargo test before committing",
        "Do not run cargo fmt and do not run cargo test before committing",
    ),
    (
        "Size the database pool at read 40 and write 60 connections per worker",
        "Size the database pool at read 60 and write 40 connections per worker",
    ),
    (
        "Enable the sandbox network for build jobs on shared runner hosts during nightly runs",
        "Disable the sandbox network for build jobs on shared runner hosts during nightly runs",
    ),
    (
        "Delegate review tasks to claude-opus-5-5 with high reasoning effort for every worker",
        "Delegate review tasks to claude-sonnet-5-5 with high reasoning effort for every worker",
    ),
];

/// Admit `text` beside `old`: it must be its own memory, and `old` stay current.
fn separate_beside(
    s: &MemoryService,
    old: MemoryId,
    text: &str,
    importance: f32,
) -> std::result::Result<MemoryId, String> {
    let admission = s
        .admit(candidate(text, importance), Actor::System, None, None)
        .expect("admit");
    match admission {
        Admission::Accepted { record } if record.supersedes.is_none() && current(s, old) => {
            Ok(record.id)
        }
        other => Err(format!("{text:?} at {importance}: {}", outcome(&other))),
    }
}

#[test]
fn a_changed_requirement_is_neither_redundant_nor_an_implicit_replacement() {
    let mut collisions = Vec::new();
    for (before, after) in CHANGED_REQUIREMENTS {
        for importance in [0.7, 0.95] {
            let s = service();
            let old = admitted(&s, candidate(before, 0.7), Actor::System);
            collisions.extend(separate_beside(&s, old, after, importance).err());
        }
    }
    assert!(collisions.is_empty(), "{collisions:#?}");
}

#[test]
fn a_repeatedly_corrected_number_keeps_every_version_current() {
    let s = service();
    let version = |n: u32| format!("Keep every function under {n} lines in this repository");
    let mut kept = vec![admitted(&s, candidate(&version(60), 0.7), Actor::System)];
    for n in [40, 30, 50] {
        let previous = *kept.last().expect("one version");
        let id = separate_beside(&s, previous, &version(n), 0.95).unwrap_or_else(|e| panic!("{e}"));
        kept.push(id);
    }
    assert!(kept.iter().all(|id| current(&s, *id)));
}

/// The price of comparing text: a note that changes only case or punctuation is
/// kept beside the original instead of being recognised as the same note.
#[test]
fn a_restatement_with_other_case_or_punctuation_is_kept_separately() {
    let before = "Run cargo fmt before committing changes to the workspace";
    for after in [
        "Run cargo fmt before committing changes to the workspace.",
        "run cargo fmt before committing changes to the workspace",
        "Run `cargo fmt` before committing changes to the workspace",
    ] {
        let s = service();
        let old = admitted(&s, candidate(before, 0.7), Actor::System);
        separate_beside(&s, old, after, 0.7).unwrap_or_else(|e| panic!("{e}"));
    }
}

#[test]
fn the_same_note_is_still_redundant_or_a_stronger_replacement() {
    let text = "Run cargo fmt before committing changes to the workspace";
    let padded = "  Run cargo fmt before committing changes to the workspace\n";
    let restatements = [(text, text), (text, padded), (padded, text)];
    for (before, after) in restatements {
        let s = service();
        let old = admitted(&s, candidate(before, 0.7), Actor::System);
        let same = s
            .admit(candidate(after, 0.7), Actor::System, None, None)
            .expect("admit");
        assert!(
            matches!(same, Admission::Rejected { ref reason } if reason.contains("redundant")),
            "{after:?}: {same:?}"
        );
        let stronger = s
            .admit(candidate(after, 0.95), Actor::System, None, None)
            .expect("admit");
        assert!(
            matches!(stronger, Admission::Superseded { old: id, .. } if id == old),
            "{after:?}: {stronger:?}"
        );
        assert!(!current(&s, old));
    }
}

#[test]
fn an_explicit_correction_still_replaces_the_record_it_names() {
    let agent = Actor::agent(AgentId::new());
    for (before, after) in CHANGED_REQUIREMENTS {
        let s = service();
        let old = admitted(&s, candidate(before, 0.7), agent.clone());
        let mut correction = candidate(after, 0.7);
        correction.supersedes = Some(old);
        let admission = s
            .admit(correction, agent.clone(), None, None)
            .expect("admit");
        let Admission::Superseded {
            record,
            old: replaced,
        } = admission
        else {
            panic!("{after:?} did not replace {before:?}: {admission:?}");
        };
        assert_eq!(replaced, old);
        assert_eq!(record.supersedes, Some(old));
        assert!(!current(&s, old) && current(&s, record.id));
    }
}

#[derive(Debug, Clone, Copy)]
enum Protection {
    Operator,
    Pinned,
    File,
}

fn protected_record(protection: Protection, text: &str) -> (MemoryService, MemoryId) {
    let s = service();
    let mut c = candidate(text, 0.7);
    let id = match protection {
        Protection::Operator => {
            c.provenance.actor = Some(Actor::user());
            admitted(&s, c, Actor::user())
        }
        Protection::Pinned => {
            let id = admitted(&s, c, Actor::System);
            s.pin(id, true, Actor::user()).expect("pin");
            id
        }
        Protection::File => {
            c.provenance.source = Some(MemorySource {
                file: ".aether/knowledge/build.md".into(),
                commit: Some("abc1234".into()),
            });
            admitted(&s, c, Actor::System)
        }
    };
    (s, id)
}

/// Pinned, operator-authored and file-sourced records keep their protection
/// on the implicit path; a changed requirement beside them replaces nothing.
#[test]
fn a_protected_record_is_never_replaced_through_resemblance() {
    let (before, after) = CHANGED_REQUIREMENTS[0];
    let agent = Actor::agent(AgentId::new());
    let cases = [
        (Protection::Operator, "operator"),
        (Protection::Pinned, "pinned"),
        (Protection::File, "build.md"),
    ];
    for (protection, refusal) in cases {
        let (s, id) = protected_record(protection, before);
        let refused = |admission: &Admission| matches!(admission, Admission::Rejected { reason } if reason.contains(refusal));

        let restated = s
            .admit(candidate(before, 0.95), agent.clone(), None, None)
            .expect("admit");
        assert!(refused(&restated), "{protection:?}: {restated:?}");

        let changed = s
            .admit(candidate(after, 0.95), agent.clone(), None, None)
            .expect("admit");
        assert!(
            matches!(changed, Admission::Accepted { ref record } if record.supersedes.is_none()),
            "{protection:?}: {changed:?}"
        );

        let mut explicit = candidate(after, 0.95);
        explicit.supersedes = Some(id);
        let explicit = s.admit(explicit, agent.clone(), None, None).expect("admit");
        assert!(refused(&explicit), "{protection:?}: {explicit:?}");
        assert!(current(&s, id), "{protection:?} record was replaced");
    }
}

#[test]
fn an_identical_note_in_another_scope_is_its_own_memory() {
    let s = service();
    let text = "Keep every function under 60 lines in this repository";
    let here = admitted(&s, candidate(text, 0.7), Actor::System);
    let mut elsewhere = candidate(text, 0.95);
    elsewhere.scope = MemoryScope::Repository { id: "other".into() };
    let admission = s
        .admit(elsewhere, Actor::System, None, None)
        .expect("admit");
    assert!(
        matches!(admission, Admission::Accepted { ref record } if record.supersedes.is_none()),
        "{admission:?}"
    );
    assert!(current(&s, here));
}
