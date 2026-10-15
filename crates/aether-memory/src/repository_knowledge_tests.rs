use super::*;
use aether_store::SqliteStore;
use std::path::Path;
use std::process::Command;

fn fixture_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    std::fs::create_dir_all(root.join(repo::KNOWLEDGE_DIR)).expect("mkdir");
    std::fs::write(
        root.join(repo::KNOWLEDGE_DIR).join("build.md"),
        "# Build\n\n\
         - **[convention]** The whole build runs through scripts/ci.sh #build\n\
         - **[invariant]** Every state change is committed with its event\n",
    )
    .expect("write");
    std::fs::write(
        root.join(repo::KNOWLEDGE_DIR).join("layout.md"),
        "- Crates live under crates/ and each owns one responsibility\n",
    )
    .expect("write");
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.email", "fixture@example.com"],
        vec!["config", "user.name", "fixture"],
        vec!["add", "-A"],
        vec!["commit", "-q", "-m", "notes"],
    ] {
        git(root, &args);
    }
    dir
}

/// Git in the fixture, deaf to the machine it runs on: commit signing, a
/// hooks path or a default branch from the developer's `~/.gitconfig`
/// would otherwise fail these tests for reasons unrelated to the code.
fn git(root: &Path, args: &[&str]) {
    let out = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .expect("git is on this machine");
    assert!(
        out.status.success(),
        "git {args:?} failed in the fixture: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The fixture plus a note for `team.steward` and one for the whole
/// `team` family, committed.
fn fixture_repo_with_roles() -> tempfile::TempDir {
    let dir = fixture_repo();
    let knowledge = dir.path().join(repo::KNOWLEDGE_DIR);
    for (role, file, note) in [
        (
            "team.steward",
            "merge.md",
            "- Stewards merge only after an independent review passes\n",
        ),
        (
            "team",
            "coordination.md",
            "- Team agents coordinate through the aether inbox\n",
        ),
    ] {
        std::fs::create_dir_all(knowledge.join(role)).expect("mkdir");
        std::fs::write(knowledge.join(role).join(file), note).expect("write");
    }
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-q", "-m", "role notes"]);
    dir
}

const STEWARD_NOTE: &str = "Stewards merge only after an independent review passes";
const FAMILY_NOTE: &str = "Team agents coordinate through the aether inbox";

fn texts_for(service: &MemoryService, repository: Option<&str>, role: &str) -> Vec<String> {
    service
        .retrieve(&RetrievalRequest {
            scope: repository.map(|id| MemoryScope::Repository { id: id.into() }),
            role: Some(role.into()),
            limit: 50,
            ..Default::default()
        })
        .expect("retrieve")
        .into_iter()
        .map(|r| r.record.text)
        .collect()
}

#[test]
fn role_directory_notes_reach_their_role_family_in_their_repository_only() {
    let dir = fixture_repo_with_roles();
    let store: Arc<dyn Store> = Arc::new(SqliteStore::open_in_memory().expect("store"));
    let service = MemoryService::new(store.clone(), WriteGate::new());
    let report = service
        .seed_repository(dir.path(), "fixture", Actor::System)
        .expect("seed");
    assert_eq!((report.found, report.admitted), (5, 5), "{report:?}");

    let steward = store
        .memories(&MemoryQuery {
            scope: Some(MemoryScope::RepositoryRole {
                id: "fixture".into(),
                role: "team.steward".into(),
            }),
            exact_scope: true,
            limit: 10,
            ..Default::default()
        })
        .expect("query");
    assert_eq!(steward.len(), 1, "{steward:?}");
    assert_eq!(
        steward[0]
            .provenance
            .source
            .as_ref()
            .map(|s| s.file.as_str()),
        Some(".aether/knowledge/team.steward/merge.md")
    );

    let has = |texts: &[String], text: &str| texts.iter().any(|t| t == text);
    for role in ["team.steward", "team.steward.lead"] {
        let got = texts_for(&service, Some("fixture"), role);
        assert!(
            has(&got, STEWARD_NOTE) && has(&got, FAMILY_NOTE),
            "{role}: {got:?}"
        );
        assert!(
            got.iter().any(|t| t.contains("scripts/ci.sh")),
            "top-level notes stay"
        );
    }
    let worker = texts_for(&service, Some("fixture"), "team.worker");
    assert!(has(&worker, FAMILY_NOTE), "{worker:?}");
    assert!(
        !has(&worker, STEWARD_NOTE),
        "another role's notes: {worker:?}"
    );
    for (repository, role) in [
        (Some("fixture"), "teammate"),
        (Some("fixture"), "worker"),
        (Some("elsewhere"), "team.steward"),
        (None, "team.steward"),
    ] {
        let got = texts_for(&service, repository, role);
        assert!(
            !has(&got, STEWARD_NOTE) && !has(&got, FAMILY_NOTE),
            "{repository:?}/{role}: {got:?}"
        );
    }
}

#[test]
fn deleting_a_role_directory_retracts_its_notes() {
    let dir = fixture_repo_with_roles();
    let store: Arc<dyn Store> = Arc::new(SqliteStore::open_in_memory().expect("store"));
    let service = MemoryService::new(store.clone(), WriteGate::new());
    service
        .seed_repository(dir.path(), "fixture", Actor::System)
        .expect("seed");

    git(
        dir.path(),
        &["rm", "-q", "-r", ".aether/knowledge/team.steward"],
    );
    git(dir.path(), &["commit", "-q", "-m", "no steward notes"]);
    let report = service
        .seed_repository(dir.path(), "fixture", Actor::System)
        .expect("seed");
    assert_eq!(report.retracted, 1, "{report:?}");
    assert_eq!(report.found, 4);
    let got = texts_for(&service, Some("fixture"), "team.steward");
    assert!(!got.iter().any(|t| t == STEWARD_NOTE), "{got:?}");
    assert!(got.iter().any(|t| t == FAMILY_NOTE), "{got:?}");
}

#[test]
fn a_repositorys_committed_notes_become_memory_it_can_retrieve() {
    let dir = fixture_repo();
    let service = MemoryService::new(
        Arc::new(SqliteStore::open_in_memory().expect("store")),
        WriteGate::new(),
    );
    let scope = MemoryScope::Repository {
        id: "fixture".into(),
    };
    let report = service
        .seed_repository(dir.path(), "fixture", Actor::System)
        .expect("seed");
    assert_eq!(report.found, 3);
    assert_eq!(report.admitted, 3, "{report:?}");

    let got = service
        .retrieve(&RetrievalRequest {
            scope: Some(scope.clone()),
            query: Some("how do I run the build".into()),
            limit: 5,
            ..Default::default()
        })
        .expect("retrieve");
    let build = got
        .iter()
        .find(|r| r.record.text.contains("scripts/ci.sh"))
        .expect("the build note is retrievable");
    assert_eq!(build.record.kind, MemoryKind::Preference);
    assert_eq!(build.record.tags, vec!["build"]);
    let source = build
        .record
        .provenance
        .source
        .as_ref()
        .expect("provenance points at the file it came from");
    assert_eq!(source.file, ".aether/knowledge/build.md");
    let commit = source.commit.as_deref().expect("and at the commit");
    assert_eq!(commit.len(), 40, "a full sha: {commit}");
    assert!(source.label().starts_with(".aether/knowledge/build.md@"));

    // Another repository cannot see it.
    let other = service
        .retrieve(&RetrievalRequest {
            scope: Some(MemoryScope::Repository {
                id: "elsewhere".into(),
            }),
            query: Some("scripts/ci.sh".into()),
            limit: 5,
            ..Default::default()
        })
        .expect("retrieve");
    assert!(other.is_empty(), "{other:?}");
}

#[test]
fn a_forgotten_note_is_not_seeded_back() {
    let dir = fixture_repo();
    let store: Arc<dyn Store> = Arc::new(SqliteStore::open_in_memory().expect("store"));
    let service = MemoryService::new(store.clone(), WriteGate::new());
    let scope = MemoryScope::Repository {
        id: "fixture".into(),
    };
    service
        .seed_repository(dir.path(), "fixture", Actor::System)
        .expect("seed");
    let id = store
        .memories(&MemoryQuery {
            scope: Some(scope.clone()),
            limit: 10,
            ..Default::default()
        })
        .expect("query")
        .into_iter()
        .find(|m| m.text.contains("scripts/ci.sh"))
        .expect("seeded")
        .id;
    service
        .forget(id, Some("out of date".into()), Actor::user())
        .expect("forget");

    let again = service
        .seed_repository(dir.path(), "fixture", Actor::System)
        .expect("seed");
    assert_eq!(again.admitted, 0, "a person's decision outranks the file");
    assert_eq!(again.unchanged, 3);
    assert!(
        store
            .memory(id)
            .expect("get")
            .expect("exists")
            .forgotten_at
            .is_some()
    );
}

/// Idempotency was a set built from one page of the scope. A repository
/// worked in for long enough has more memories than that page, and a note
/// a person forgot then falls outside it and is seeded back as fresh.
#[test]
fn a_forgotten_note_stays_forgotten_however_large_the_scope_grows() {
    let dir = fixture_repo();
    let store: Arc<dyn Store> = Arc::new(SqliteStore::open_in_memory().expect("store"));
    let service = MemoryService::new(store.clone(), WriteGate::new());
    let scope = MemoryScope::Repository {
        id: "fixture".into(),
    };
    service
        .seed_repository(dir.path(), "fixture", Actor::System)
        .expect("seed");
    let id = store
        .memories(&MemoryQuery {
            scope: Some(scope.clone()),
            limit: 10,
            ..Default::default()
        })
        .expect("query")
        .into_iter()
        .find(|m| m.text.contains("scripts/ci.sh"))
        .expect("seeded")
        .id;
    service
        .forget(id, Some("out of date".into()), Actor::user())
        .expect("forget");

    // Past the page the old presence check read, and ranked above the
    // forgotten note by the order it used.
    let mut filler = Commit::new();
    for i in 0..1_100 {
        let mut c = MemoryCandidate {
            scope: scope.clone(),
            kind: MemoryKind::Fact,
            text: format!("An agent wrote down observation number {i} while working here"),
            structured: None,
            importance: 0.9,
            confidence: 0.9,
            tags: vec![],
            provenance: Default::default(),
            supersedes: None,
        };
        c.importance = 0.9;
        filler.push_mutation(Mutation::PutMemory(MemoryRecord::from_candidate(c, now())));
    }
    store.commit(filler).expect("filler");

    let again = service
        .seed_repository(dir.path(), "fixture", Actor::System)
        .expect("seed");
    assert_eq!(
        again.admitted, 0,
        "a person's decision stopped outranking the file at scale: {again:?}"
    );
    assert_eq!(again.unchanged, 3);
}

/// A note that has left the tree - a reverted commit, a deleted file - is
/// retracted. The label promises the fact can be checked against the line
/// that states it, and there is no such line any more.
#[test]
fn a_note_that_leaves_the_tree_is_retracted_with_its_reason() {
    let dir = fixture_repo();
    let store: Arc<dyn Store> = Arc::new(SqliteStore::open_in_memory().expect("store"));
    let service = MemoryService::new(store.clone(), WriteGate::new());
    let scope = MemoryScope::Repository {
        id: "fixture".into(),
    };
    service
        .seed_repository(dir.path(), "fixture", Actor::System)
        .expect("seed");
    std::fs::remove_file(dir.path().join(repo::KNOWLEDGE_DIR).join("layout.md")).expect("remove");
    git(dir.path(), &["add", "-A"]);
    git(
        dir.path(),
        &["commit", "-q", "-m", "revert the layout note"],
    );

    let again = service
        .seed_repository(dir.path(), "fixture", Actor::System)
        .expect("seed");
    assert_eq!(again.found, 2);
    assert_eq!(again.retracted, 1, "{again:?}");
    let gone = store
        .memories(&MemoryQuery {
            scope: Some(scope.clone()),
            include_forgotten: true,
            limit: 10,
            ..Default::default()
        })
        .expect("query")
        .into_iter()
        .find(|m| m.text.contains("Crates live under"))
        .expect("history is never deleted");
    assert!(gone.forgotten_at.is_some());
    let reason = store
        .events(0, 10_000, &Default::default())
        .expect("events")
        .into_iter()
        .find_map(|e| match e.kind {
            EventKind::MemoryForgotten { memory, reason } if memory == gone.id => reason,
            _ => None,
        })
        .expect("the retraction says why");
    assert!(reason.contains("layout.md"), "{reason}");
}

/// A repository scope is shared by every clone, worktree and branch, so
/// the sweep cannot be keyed to whichever tree happens to be scanned: an
/// agent started on an older checkout would forget, for the whole
/// repository and permanently, every note that checkout predates.
#[test]
fn an_older_checkout_does_not_forget_notes_it_simply_predates() {
    let origin = fixture_repo();
    let clone = tempfile::tempdir().expect("tempdir");
    let old = clone.path().join("old");
    git(origin.path(), &["clone", "-q", ".", &old.to_string_lossy()]);

    std::fs::write(
        origin.path().join(repo::KNOWLEDGE_DIR).join("deploy.md"),
        "- Deploys need a signed tag on the release branch\n",
    )
    .expect("write");
    git(origin.path(), &["add", "-A"]);
    git(origin.path(), &["commit", "-q", "-m", "the deploy note"]);

    let store: Arc<dyn Store> = Arc::new(SqliteStore::open_in_memory().expect("store"));
    let service = MemoryService::new(store.clone(), WriteGate::new());
    let scope = MemoryScope::Repository {
        id: "fixture".into(),
    };
    let seed = |root: &Path| {
        service
            .seed_repository(root, "fixture", Actor::System)
            .expect("seed")
    };
    assert_eq!(seed(origin.path()).admitted, 4);

    // The old checkout has never seen the commit the deploy note was read
    // at, so its absence there says nothing about the repository.
    let from_old = seed(&old);
    assert_eq!(
        from_old.retracted, 0,
        "an older checkout forgot notes for every clone: {from_old:?}"
    );
    assert_eq!(from_old.unchanged, 3);

    // The repository dropping a note is still a retraction.
    std::fs::remove_file(origin.path().join(repo::KNOWLEDGE_DIR).join("layout.md"))
        .expect("remove");
    git(origin.path(), &["add", "-A"]);
    git(
        origin.path(),
        &["commit", "-q", "-m", "drop the layout note"],
    );
    assert_eq!(seed(origin.path()).retracted, 1);
    let layout = |store: &dyn Store| {
        store
            .memories(&MemoryQuery {
                scope: Some(scope.clone()),
                exact_scope: true,
                include_forgotten: true,
                include_superseded: true,
                limit: 50,
                ..Default::default()
            })
            .expect("query")
            .into_iter()
            .filter(|m| m.text.contains("Crates live under"))
            .collect::<Vec<_>>()
    };
    let retracted = layout(store.as_ref());
    assert_eq!(retracted.len(), 1);
    let mark = retracted[0]
        .retracted
        .as_ref()
        .expect("a sweep retraction is not a person's forget");
    assert_eq!(mark.file, ".aether/knowledge/layout.md");
    assert!(retracted[0].forgotten_at.is_some());

    // And a tree that still states it seeds it again: a retraction is
    // undone by the note coming back, unlike a person's curation.
    assert_eq!(seed(&old).admitted, 1);
    assert!(
        layout(store.as_ref()).iter().any(|m| m.is_current()),
        "a retracted note that reappeared was never re-admitted"
    );
}

/// Pinning is a person's authority everywhere else on this path, so the
/// sweep does not get to undo it.
#[test]
fn a_pinned_note_is_never_swept() {
    let dir = fixture_repo();
    let store: Arc<dyn Store> = Arc::new(SqliteStore::open_in_memory().expect("store"));
    let service = MemoryService::new(store.clone(), WriteGate::new());
    let scope = MemoryScope::Repository {
        id: "fixture".into(),
    };
    service
        .seed_repository(dir.path(), "fixture", Actor::System)
        .expect("seed");
    let pinned = store
        .memories(&MemoryQuery {
            scope: Some(scope.clone()),
            exact_scope: true,
            limit: 50,
            ..Default::default()
        })
        .expect("query")
        .into_iter()
        .find(|m| m.text.contains("Crates live under"))
        .expect("seeded");
    service.pin(pinned.id, true, Actor::user()).expect("pin");

    std::fs::remove_file(dir.path().join(repo::KNOWLEDGE_DIR).join("layout.md")).expect("remove");
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-q", "-m", "drop it"]);
    let report = service
        .seed_repository(dir.path(), "fixture", Actor::System)
        .expect("seed");
    assert_eq!(report.retracted, 0, "{report:?}");
    assert!(
        store
            .memory(pinned.id)
            .expect("get")
            .expect("exists")
            .is_current()
    );
}

#[test]
fn seeding_the_same_tree_twice_writes_nothing_the_second_time() {
    let dir = fixture_repo();
    let store: Arc<dyn Store> = Arc::new(SqliteStore::open_in_memory().expect("store"));
    let service = MemoryService::new(store.clone(), WriteGate::new());
    service
        .seed_repository(dir.path(), "fixture", Actor::System)
        .expect("seed");
    let before = store
        .events(0, 10_000, &Default::default())
        .expect("events")
        .len();
    let again = service
        .seed_repository(dir.path(), "fixture", Actor::System)
        .expect("seed");
    assert_eq!(again.unchanged, 3);
    assert_eq!(again.admitted, 0);
    assert_eq!(
        store
            .events(0, 10_000, &Default::default())
            .expect("events")
            .len(),
        before,
        "an unchanged tree is not even a candidate"
    );
}

#[test]
fn proposal_does_not_count_as_an_already_seeded_note() {
    let dir = fixture_repo();
    let service = MemoryService::new(
        Arc::new(SqliteStore::open_in_memory().unwrap()),
        WriteGate::new(),
    );
    let mut candidate = repo::scan(dir.path(), "fixture").unwrap().remove(0);
    candidate.provenance = aether_core::memory::MemoryProvenance {
        actor: Some(Actor::user()),
        ..Default::default()
    };
    let hold = service.gate.hold();
    let (admission, commit) = service
        .evaluate_operator(&hold, candidate, Actor::user(), true)
        .unwrap();
    let pending = admission.record().unwrap().clone();
    assert!(pending.is_proposed());
    assert!(!pending.is_current());
    service.store.commit(commit).unwrap();
    drop(hold);
    let report = service
        .seed_repository(dir.path(), "fixture", Actor::System)
        .unwrap();
    assert_eq!((report.admitted, report.unchanged), (3, 0));
    assert_eq!(
        service
            .store
            .memories(&MemoryQuery::default())
            .unwrap()
            .len(),
        3
    );
    assert_eq!(service.store.memory(pending.id).unwrap(), Some(pending));
}
