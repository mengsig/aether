//! An old repository name aliased to the id its checkout derives now.
use super::memory_scope::memories_shown_after;
use super::*;
use aether_core::agent::NetworkPolicy;
use aether_core::memory::{MemoryScope, OperatorMemoryScope, OperatorMemorySpec};
use aether_core::schedule::{CreateScheduleSpec, Recurrence, ScheduleTarget};

pub(super) const CANONICAL: &str = "github.com/mengsig/archon";

/// What Archon up to 1.2.1 sends: the bare name, not a checkout.
pub(super) fn archon_agent(role: &str, parent: Option<AgentId>) -> CreateAgentSpec {
    CreateAgentSpec {
        scope: Some(Scope {
            repository: Some("archon".into()),
            ..Default::default()
        }),
        ..long_lived(role, parent)
    }
}

pub(super) fn filed(repository: &str, text: &str) -> OperatorMemorySpec {
    OperatorMemorySpec {
        text: text.into(),
        kind: aether_core::memory::MemoryKind::Fact,
        scope: OperatorMemoryScope::Repository {
            root: None,
            repository: Some(repository.into()),
        },
        importance: Some(0.9),
        confidence: None,
        tags: Vec::new(),
        supersedes: None,
    }
}

fn alias_archon(h: &Harness, dry_run: bool) -> aether_core::repository::RepositoryAliasReport {
    h.runtime
        .alias_repository(
            "archon",
            None,
            Some(CANONICAL.into()),
            dry_run,
            Actor::user(),
        )
        .expect("alias")
}

pub(super) fn in_repository(
    h: &Harness,
    repository: &str,
) -> Vec<aether_core::memory::MemoryRecord> {
    h.runtime
        .memories(aether_store::MemoryQuery {
            scope: Some(MemoryScope::Repository {
                id: repository.into(),
            }),
            exact_scope: true,
            limit: 100,
            ..Default::default()
        })
        .expect("memories")
}

/// An agent created before the alias keeps naming `archon`, and still reads
/// what was filed under it, what is filed under the canonical id since, and
/// files what it remembers there too.
#[tokio::test]
async fn an_agent_still_naming_the_old_id_reads_and_writes_the_canonical_scope() {
    const LESSON: &str = "Archon releases are cut from the main branch on Fridays";
    let h = harness(vec![
        ScriptedClient::tool_call(
            "remember",
            json!({"text": LESSON, "kind": "lesson", "scope": "repository", "importance": 0.8}),
        ),
        ScriptedClient::text("Noted."),
        ScriptedClient::text("Done."),
    ])
    .await;
    let (old, _) = h
        .runtime
        .create_agent(archon_agent("worker", None), Actor::user())
        .expect("old agent");
    assert_eq!(old.scope.repository.as_deref(), Some("archon"));
    let before = h
        .runtime
        .create_memory(
            filed("archon", "Archon's tests run under cargo nextest"),
            Actor::user(),
        )
        .expect("filed under the old name");

    let preview = alias_archon(&h, true);
    assert!(preview.dry_run && !preview.existing);
    assert_eq!((preview.counts.repository, preview.counts.agents), (1, 1));
    assert_eq!(
        in_repository(&h, CANONICAL).len(),
        0,
        "a dry run moves nothing"
    );
    let report = alias_archon(&h, false);
    assert_eq!(report.counts, preview.counts);
    assert_eq!(report.canonical, CANONICAL);

    let after = h
        .runtime
        .create_memory(
            filed(CANONICAL, "Archon's CI pins the Rust toolchain"),
            Actor::user(),
        )
        .expect("an aliased canonical id is a known repository");
    assert_eq!(
        after.scope,
        MemoryScope::Repository {
            id: CANONICAL.into()
        }
    );
    let via_alias = h
        .runtime
        .create_memory(
            filed("archon", "Archon's docs live under docs/"),
            Actor::user(),
        )
        .expect("the old name still names the repository");
    assert_eq!(via_alias.scope, after.scope);

    let shown = memories_shown_after(&h, old.id, "remember the release day", 2).await;
    assert!(
        shown.contains(&before.id) && shown.contains(&after.id),
        "{shown:?}"
    );
    let reread = h.store.agent(old.id).expect("agent").expect("exists");
    assert_eq!(
        reread.scope.repository.as_deref(),
        Some("archon"),
        "rows are not rewritten"
    );

    let by_alias = in_repository(&h, "archon");
    assert_eq!(by_alias, in_repository(&h, CANONICAL));
    let texts: Vec<_> = by_alias.iter().map(|m| m.text.as_str()).collect();
    assert!(
        texts.contains(&LESSON),
        "remember lands in the canonical scope: {texts:?}"
    );
    assert_eq!(by_alias.len(), 4);
    assert!(
        h.store
            .memories(&aether_store::MemoryQuery {
                scope: Some(MemoryScope::Repository {
                    id: "archon".into()
                }),
                exact_scope: true,
                include_forgotten: true,
                include_superseded: true,
                limit: 10,
                ..Default::default()
            })
            .expect("raw")
            .is_empty(),
        "nothing is filed under the alias any more"
    );
}

/// After the alias, naming `archon` creates an agent in the canonical scope,
/// and a child of an agent created before it inherits and narrows as usual.
#[tokio::test]
async fn new_agents_and_children_of_old_ones_land_in_the_canonical_scope() {
    let h = harness(vec![ScriptedClient::text("Done.")]).await;
    let (old, _) = h
        .runtime
        .create_agent(archon_agent("supervisor", None), Actor::user())
        .expect("old parent");
    let moved = h
        .runtime
        .create_memory(
            filed("archon", "Archon's tests run under cargo nextest"),
            Actor::user(),
        )
        .expect("filed under the old name");
    alias_archon(&h, false);

    let (fresh, _) = h
        .runtime
        .create_agent(archon_agent("worker", None), Actor::user())
        .expect("new agent");
    assert_eq!(fresh.scope.repository.as_deref(), Some(CANONICAL));
    for named in [None, Some("archon"), Some(CANONICAL)] {
        let spec = match named {
            Some(id) => CreateAgentSpec {
                scope: Some(Scope {
                    repository: Some(id.into()),
                    ..Default::default()
                }),
                ..long_lived("worker", Some(old.id))
            },
            None => long_lived("worker", Some(old.id)),
        };
        let (child, _) = h
            .runtime
            .create_agent(spec, Actor::user())
            .unwrap_or_else(|e| panic!("child naming {named:?}: {e}"));
        assert_eq!(
            child.scope.repository.as_deref(),
            Some(CANONICAL),
            "{named:?}"
        );
    }
    let mut elsewhere = archon_agent("worker", Some(old.id));
    elsewhere.scope.as_mut().expect("scope").repository = Some("github.com/acme/other".into());
    let err = h
        .runtime
        .create_agent(elsewhere, Actor::user())
        .expect_err("another repository is not narrower");
    assert!(err.to_string().contains("`repository` widens"), "{err}");

    let shown = memories_shown_after(&h, fresh.id, "what do we know?", 1).await;
    assert!(
        shown.contains(&moved.id),
        "the moved memory reaches the canonical scope: {shown:?}"
    );
}

/// Only an operator aliases, and a refused alias is the caller's mistake.
#[tokio::test]
async fn an_alias_is_an_operators_and_a_conflicting_one_is_refused() {
    let h = harness(Vec::new()).await;
    let (agent, _) = h
        .runtime
        .create_agent(archon_agent("worker", None), Actor::user())
        .expect("agent");
    let by_agent = h.runtime.alias_repository(
        "archon",
        None,
        Some(CANONICAL.into()),
        false,
        Actor::Agent { id: agent.id },
    );
    assert!(matches!(
        by_agent,
        Err(aether_runtime::RuntimeError::Invalid(_))
    ));
    assert!(h.runtime.repository_aliases().expect("aliases").is_empty());

    alias_archon(&h, false);
    let again = alias_archon(&h, false);
    assert!(again.existing && again.counts.memories() == 0);
    let conflict = h.runtime.alias_repository(
        "archon",
        None,
        Some("github.com/someone/else".into()),
        true,
        Actor::user(),
    );
    let Err(aether_runtime::RuntimeError::Invalid(why)) = conflict else {
        panic!("{conflict:?}");
    };
    assert!(why.contains("already an alias"), "{why}");
    let neither = h
        .runtime
        .alias_repository("archon", None, None, true, Actor::user());
    assert!(matches!(
        neither,
        Err(aether_runtime::RuntimeError::Invalid(_))
    ));
    assert_eq!(h.runtime.repository_aliases().expect("aliases").len(), 1);
}

/// A child created after the alias stores the canonical id while its parent
/// still names `archon`: tightening it, restarting it and scheduling to it
/// compare the two as one repository.
#[tokio::test]
async fn a_child_in_the_canonical_scope_stays_within_a_parent_naming_the_alias() {
    let h = harness(Vec::new()).await;
    let mut parent = archon_agent("supervisor", None);
    parent.start = false;
    parent.scope.as_mut().expect("scope").network = NetworkPolicy::Allowed;
    let (parent, _) = h
        .runtime
        .create_agent(parent, Actor::user())
        .expect("old parent");
    alias_archon(&h, false);
    let child = CreateAgentSpec {
        start: false,
        ..long_lived("worker", Some(parent.id))
    };
    let (child, _) = h
        .runtime
        .create_agent(child, Actor::agent(parent.id))
        .expect("child");
    assert_eq!(child.scope.repository.as_deref(), Some(CANONICAL));
    assert_eq!(child.scope.network, NetworkPolicy::Allowed);

    let tightened = h
        .runtime
        .update_agent(
            child.id,
            AgentUpdate {
                network: Some(NetworkPolicy::Denied),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("denying the network narrows the child");
    assert_eq!(tightened.scope.network, NetworkPolicy::Denied);
    h.runtime
        .cancel_agent(child.id, None, Actor::user())
        .expect("cancel");
    let successor = h
        .runtime
        .restart_agent(child.id, Actor::user())
        .expect("the parent was not tightened");
    assert_eq!(successor.scope.repository.as_deref(), Some(CANONICAL));
    let schedule = h
        .runtime
        .create_schedule(
            CreateScheduleSpec {
                name: "nightly".into(),
                target: ScheduleTarget::Agent(successor.id),
                recurrence: Recurrence::Interval {
                    seconds: std::num::NonZeroU64::new(3600).expect("nonzero"),
                },
                missed: Default::default(),
                busy: Default::default(),
                payload: json!({"source": "timer"}),
                max_deliveries: None,
            },
            Actor::agent(parent.id),
        )
        .expect("the child is within its creator's authority");
    assert_eq!(schedule.target(), Some(successor.id));
}
