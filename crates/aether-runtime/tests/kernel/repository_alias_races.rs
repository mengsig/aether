//! An alias committing between resolving a repository name and using it.
use super::memory_scope::memories_shown_after;
use super::repository_alias::{CANONICAL, archon_agent, filed, in_repository};
use super::store_double::Armed;
use super::*;
use aether_core::memory::MemoryScope;
use aether_core::repository::RepositoryAlias;

async fn hooked_harness(
    responses: Vec<aether_model::testing::ScriptedResponse>,
) -> (Harness, Arc<CommitsMidPass>) {
    let inner: Arc<dyn Store> = Arc::new(SqliteStore::open_in_memory().expect("store"));
    let store = Arc::new(CommitsMidPass::new(inner, Hook::RepositoryResolved));
    let h = harness_full(
        responses,
        |_| {},
        |_| {},
        None,
        None,
        Some(store.clone()),
        |_| {},
    )
    .await;
    (h, store)
}

/// A concurrent operator aliasing `alias` to `canonical`, straight to the store.
fn aliases(alias: &'static str, canonical: &'static str) -> Armed {
    Box::new(move |store| {
        let alias = RepositoryAlias::new(alias, canonical, now()).expect("alias");
        let gate = aether_store::WriteGate::new();
        store
            .alias_repository(&alias, Actor::user(), &gate.hold())
            .map(|_| ())
    })
}

#[tokio::test]
async fn an_operator_query_racing_the_alias_reads_where_the_memories_went() {
    let (h, store) = hooked_harness(Vec::new()).await;
    let mut old = archon_agent("worker", None);
    old.start = false;
    h.runtime
        .create_agent(old, Actor::user())
        .expect("old agent");
    let note = h
        .runtime
        .create_memory(
            filed("archon", "Archon's tests run under cargo nextest"),
            Actor::user(),
        )
        .expect("filed under the old name");

    store.arm(aliases("archon", CANONICAL));
    let found = in_repository(&h, "archon");
    assert!(
        store.fired(),
        "the alias landed after the name was resolved"
    );
    assert_eq!(found.iter().map(|m| m.id).collect::<Vec<_>>(), [note.id]);
    assert_eq!(
        found[0].scope,
        MemoryScope::Repository {
            id: CANONICAL.into()
        }
    );
}

#[tokio::test]
async fn a_turn_racing_the_alias_still_sees_the_repository_memories() {
    let (h, store) = hooked_harness(vec![ScriptedClient::text("Done.")]).await;
    let (old, _) = h
        .runtime
        .create_agent(archon_agent("worker", None), Actor::user())
        .expect("old agent");
    let note = h
        .runtime
        .create_memory(
            filed("archon", "Archon's tests run under cargo nextest"),
            Actor::user(),
        )
        .expect("filed under the old name");
    h.wait_until(old.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;

    store.arm(aliases("archon", CANONICAL));
    let shown = memories_shown_after(&h, old.id, "how do the tests run?", 1).await;
    assert!(
        store.fired(),
        "the alias landed after the name was resolved"
    );
    assert!(shown.contains(&note.id), "{shown:?}");
}

/// A checkout whose `origin` derives `example.com/acme/legacy`, with one note.
fn commit_note(root: &std::path::Path) {
    let notes = root.join(aether_memory::repo::KNOWLEDGE_DIR);
    std::fs::create_dir_all(&notes).expect("mkdir");
    std::fs::write(
        notes.join("build.md"),
        "# Build\n\n- **[convention]** The whole build runs through scripts/ci.sh #build\n",
    )
    .expect("write");
    git_in(root, &["init", "-q"]);
    git_in(root, &["config", "user.email", "f@example.com"]);
    git_in(root, &["config", "user.name", "fixture"]);
    git_in(root, &["add", "-A"]);
    git_in(root, &["commit", "-q", "-m", "notes"]);
    let origin = "https://example.com/acme/legacy.git";
    git_in(root, &["remote", "add", "origin", origin]);
}

/// A person forgot a seeded note; an alias moving it mid-seed must not let the
/// seed miss it under the old name and admit it again with a new id.
#[tokio::test]
async fn seeding_racing_the_alias_keeps_a_forgotten_note_forgotten() {
    const OLD: &str = "example.com/acme/legacy";
    const NEW: &str = "example.com/acme/renamed";
    let (h, store) = hooked_harness(Vec::new()).await;
    commit_note(&h.workspace_root);
    let (repository, first) = h
        .runtime
        .sync_repository_knowledge(None, Actor::user())
        .expect("seed");
    assert_eq!((repository.as_str(), first.admitted), (OLD, 1));
    let seeded = in_repository(&h, OLD);
    assert_eq!(seeded.len(), 1);
    h.runtime
        .forget_memory(seeded[0].id, Some("not ours".into()), Actor::user())
        .expect("forget");

    // Skips the runtime's own resolution and lands after the seed's.
    store.arm_skipping(1, aliases(OLD, NEW));
    let (_, again) = h
        .runtime
        .sync_repository_knowledge(None, Actor::user())
        .expect("seed again");
    assert!(store.fired(), "the alias landed after the seed resolved");
    assert_eq!((again.found, again.admitted, again.unchanged), (1, 0, 1));
    let stored = h
        .store
        .memories(&aether_store::MemoryQuery {
            scope: Some(MemoryScope::Repository { id: NEW.into() }),
            exact_scope: true,
            include_forgotten: true,
            include_superseded: true,
            limit: 10,
            ..Default::default()
        })
        .expect("raw");
    assert_eq!(
        stored.iter().map(|m| m.id).collect::<Vec<_>>(),
        [seeded[0].id]
    );
    assert!(stored[0].forgotten_at.is_some(), "still forgotten");
}
