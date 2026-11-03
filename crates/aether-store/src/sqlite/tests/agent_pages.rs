use super::*;
use std::collections::BTreeSet;

fn create(store: &SqliteStore, agent: Agent, subsystem: Option<&str>) {
    let subsystem = subsystem.map(str::to_owned);
    store
        .commit(Commit::new().mutate(Mutation::CreateAgent { agent, subsystem }))
        .expect("create");
}

/// Every page past `after` until the last, in the order the store hands them.
fn walk(store: &SqliteStore, filter: &AgentFilter, limit: usize) -> Vec<AgentId> {
    let mut seen = Vec::new();
    let mut after = None;
    loop {
        let page = store.agents_page(filter, after, limit).expect("page");
        assert!(page.agents.len() <= limit);
        seen.extend(page.agents.iter().map(|agent| agent.id));
        let Some(next) = page.next_after else { break };
        assert_eq!(Some(next), page.agents.last().map(|agent| agent.id));
        after = Some(next);
    }
    seen
}

/// Keyset paging never shifts: every agent there for the whole walk appears
/// exactly once, and one created meanwhile at most once.
#[test]
fn pages_cover_every_agent_once_while_more_are_created() {
    let dir = tempfile::tempdir().expect("tmp");
    let store = SqliteStore::open(dir.path().join("aether.db")).expect("open");
    let seeded: BTreeSet<AgentId> = (0..250)
        .map(|_| {
            let agent = agent();
            let id = agent.id;
            create(&store, agent, None);
            id
        })
        .collect();
    let seen = std::thread::scope(|scope| {
        scope.spawn(|| {
            for _ in 0..200 {
                create(&store, agent(), None);
            }
        });
        walk(&store, &AgentFilter::default(), 7)
    });
    let unique: BTreeSet<AgentId> = seen.iter().copied().collect();
    assert_eq!(unique.len(), seen.len(), "no agent is paged twice");
    assert!(
        seen.windows(2).all(|pair| pair[0] < pair[1]),
        "pages run in id order"
    );
    assert!(
        seeded.is_subset(&unique),
        "no agent present throughout is skipped"
    );

    let everyone: BTreeSet<AgentId> = store
        .agents(&AgentFilter::default())
        .expect("agents")
        .into_iter()
        .map(|agent| agent.id)
        .collect();
    assert_eq!(everyone.len(), 450);
    assert!(unique.is_subset(&everyone));
    let settled = walk(&store, &AgentFilter::default(), 500);
    assert_eq!(settled.into_iter().collect::<BTreeSet<_>>(), everyone);
}

#[test]
fn a_page_holds_only_what_the_filter_names() {
    let store = SqliteStore::open_in_memory().expect("open");
    let run = RunId::new();
    let mut wanted = BTreeSet::new();
    for i in 0..12 {
        let mut agent = agent();
        if i % 3 == 0 {
            agent.run = run;
            wanted.insert(agent.id);
        }
        if i % 4 == 0 {
            agent.run = run;
            agent.archived = true;
        }
        create(&store, agent, None);
    }
    let filter = AgentFilter {
        run: Some(run),
        exclude_archived: true,
        ..AgentFilter::default()
    };
    let wanted: BTreeSet<AgentId> = wanted
        .into_iter()
        .filter(|id| store.agent(*id).unwrap().is_some_and(|a| !a.archived))
        .collect();
    let paged = walk(&store, &filter, 1);
    assert_eq!(paged.iter().copied().collect::<BTreeSet<_>>(), wanted);
    let listed: BTreeSet<AgentId> = store
        .agents(&filter)
        .expect("agents")
        .into_iter()
        .map(|agent| agent.id)
        .collect();
    assert_eq!(listed, wanted, "paging and listing agree");
}

/// The subsystem is written by the creating insert and nothing after it.
#[test]
fn a_subsystem_is_fixed_at_creation() {
    let store = SqliteStore::open_in_memory().expect("open");
    let mut named = agent();
    create(&store, named.clone(), Some("archon"));
    let plain = agent();
    create(&store, plain.clone(), None);
    assert_eq!(
        store.agent_subsystem(named.id).unwrap().as_deref(),
        Some("archon")
    );
    assert_eq!(store.agent_subsystem(plain.id).unwrap(), None);

    named.name = "renamed".into();
    store
        .commit(Commit::new().mutate(Mutation::UpsertAgent(named.clone())))
        .expect("upsert");
    assert_eq!(store.agent(named.id).unwrap().unwrap().name, "renamed");
    assert_eq!(
        store.agent_subsystem(named.id).unwrap().as_deref(),
        Some("archon")
    );

    let again = store.commit(Commit::new().mutate(Mutation::CreateAgent {
        agent: named.clone(),
        subsystem: Some("other".into()),
    }));
    assert!(again.is_err(), "a second creation of one id is refused");
    let changed = store.write_conn().execute(
        "UPDATE agents SET subsystem = 'other' WHERE id = ?1",
        [named.id.to_string()],
    );
    let error = changed.expect_err("the trigger refuses a change");
    assert!(error.to_string().contains("set once"), "{error}");
    assert_eq!(
        store.agent_subsystem(named.id).unwrap().as_deref(),
        Some("archon")
    );
}
