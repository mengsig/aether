use std::collections::BTreeSet;

use super::store::{self, SnapshotFilter};
use super::*;
use crate::store::NoesisStore;

fn origin(source: &str, message: &str) -> Origin {
    Origin {
        source: source.into(),
        message_ref: message.into(),
    }
}

fn finding(key: &str, name: &str, statement: &str, link: Option<&str>) -> Finding {
    Finding {
        key: key.into(),
        name: name.into(),
        statement: statement.into(),
        aliases: Vec::new(),
        origin: origin("ns_a", &format!("m-{key}")),
        link: link.map(Into::into),
    }
}

fn local(key: &str) -> Endpoint {
    Endpoint::Local(key.into())
}

fn chain(premises: &[&str], conclusion: &str) -> FoundChain {
    FoundChain {
        premises: premises.iter().map(|k| local(k)).collect(),
        conclusion: local(conclusion),
        origin: origin("ns_a", "m-chain"),
    }
}

/// A small deterministic generator, so the property test needs no crate.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            items.swap(i, self.below(i + 1));
        }
    }
}

const WORDS: &[&str] = &[
    "water", "boils", "ice", "melts", "heat", "flows", "salt", "lowers",
];

/// A random extraction over a small vocabulary, so statements repeat,
/// links hit and miss, and chains collide, loop and nest.
fn random_extraction(rng: &mut Rng, base: &Graph, source: &str) -> Extraction {
    let base_ids: Vec<&String> = base.nodes.keys().collect();
    let mut findings = Vec::new();
    for i in 0..(3 + rng.below(8)) {
        let statement = format!(
            "{} {} {}",
            WORDS[rng.below(8)],
            WORDS[rng.below(8)],
            WORDS[rng.below(4)]
        );
        let link = match rng.below(4) {
            0 if !base_ids.is_empty() => Some(base_ids[rng.below(base_ids.len())].clone()),
            1 => Some("p_unknown".to_string()),
            _ => None,
        };
        findings.push(Finding {
            key: format!("k{i}"),
            name: WORDS[rng.below(8)].to_string(),
            statement,
            aliases: vec![WORDS[rng.below(8)].to_string()],
            origin: origin(source, &format!("m{}", rng.below(3))),
            link,
        });
    }
    let mut chains = Vec::new();
    for _ in 0..rng.below(10) {
        let premises = (0..1 + rng.below(3))
            .map(|_| local(&format!("k{}", rng.below(findings.len()))))
            .collect();
        let conclusion = local(&format!("k{}", rng.below(findings.len())));
        chains.push(FoundChain {
            premises,
            conclusion,
            origin: origin(source, "m0"),
        });
    }
    Extraction { findings, chains }
}

#[test]
fn the_merge_is_deterministic_under_any_input_order() {
    let mut rng = Rng(0x5eed_1234_abcd_0001);
    for round in 0..200 {
        let base = merge(
            &Graph::default(),
            &random_extraction(&mut rng, &Graph::default(), "ns_base"),
        )
        .graph;
        let extraction = random_extraction(&mut rng, &base, "ns_new");
        let expected = merge(&base, &extraction);
        expected.graph.check_structure().unwrap();
        for _ in 0..8 {
            let mut shuffled = extraction.clone();
            rng.shuffle(&mut shuffled.findings);
            rng.shuffle(&mut shuffled.chains);
            for found in &mut shuffled.chains {
                rng.shuffle(&mut found.premises);
            }
            assert_eq!(merge(&base, &shuffled), expected, "round {round}");
        }
        for (id, node) in &expected.graph.nodes {
            assert_eq!(*id, ids::proposition_id(&node.statement), "a content id");
        }
        for (id, found) in &expected.graph.chains {
            assert_eq!(*id, ids::chain_id(&found.premises, &found.conclusion));
        }
    }
}

#[test]
fn proposition_merges_commute_across_extractions() {
    let mut rng = Rng(42);
    for _ in 0..100 {
        let mut a = random_extraction(&mut rng, &Graph::default(), "ns_a");
        let mut b = random_extraction(&mut rng, &Graph::default(), "ns_b");
        for extraction in [&mut a, &mut b] {
            extraction.chains.clear();
            for (i, finding) in extraction.findings.iter_mut().enumerate() {
                finding.link = None;
                finding.key = format!("{}{i}", finding.origin.source);
            }
        }
        let ab = merge(&merge(&Graph::default(), &a).graph, &b).graph;
        let ba = merge(&merge(&Graph::default(), &b).graph, &a).graph;
        assert_eq!(
            ab.nodes.keys().collect::<Vec<_>>(),
            ba.nodes.keys().collect::<Vec<_>>()
        );
        for (id, node) in &ab.nodes {
            assert_eq!(node.provenance, ba.nodes[id].provenance);
        }
    }
}

#[test]
fn a_link_appends_and_an_unknown_link_creates() {
    let first = Extraction {
        findings: vec![finding("a", "boiling", "Water boils at 100 C.", None)],
        chains: Vec::new(),
    };
    let base = merge(&Graph::default(), &first).graph;
    let target = ids::proposition_id("Water boils at 100 C.");
    let mut again = finding(
        "b",
        "boiling point",
        "Pure water boils at 100 C at sea level.",
        Some(&target),
    );
    again.origin = origin("s_notes", "chunk-0");
    let ghost = finding("c", "ghost", "Steam is invisible.", Some("p_missing"));
    let outcome = merge(
        &base,
        &Extraction {
            findings: vec![again, ghost],
            chains: Vec::new(),
        },
    );
    let node = &outcome.graph.nodes[&target];
    assert_eq!(node.name, "boiling", "an existing name stays");
    assert!(node.aliases.contains("boiling point"));
    assert_eq!(node.provenance.len(), 2, "provenance is unioned");
    assert!(
        outcome
            .graph
            .nodes
            .contains_key(&ids::proposition_id("Steam is invisible."))
    );
    assert!(!outcome.graph.nodes.contains_key("p_missing"));
    assert_eq!(outcome.nodes, 2);
}

#[test]
fn chains_dedupe_by_key_keep_their_id_and_drop_circular_ones() {
    let findings = vec![
        finding("a", "a", "Alpha holds.", None),
        finding("b", "b", "Beta holds.", None),
        finding("c", "c", "Gamma holds.", None),
    ];
    let extraction = Extraction {
        findings: findings.clone(),
        chains: vec![
            chain(&["a", "b"], "c"),
            chain(&["b", "a"], "c"),
            chain(&["a", "c"], "c"),
        ],
    };
    let first = merge(&Graph::default(), &extraction);
    assert_eq!(first.graph.chains.len(), 1);
    assert_eq!((first.chains, first.dropped_chains), (1, 1));
    let id = first.graph.chains.keys().next().unwrap().clone();
    let mut again = extraction.clone();
    for f in &mut again.findings {
        f.origin = origin("ns_b", "m9");
    }
    for c in &mut again.chains {
        c.origin = origin("ns_b", "m9");
    }
    let second = merge(&first.graph, &again);
    assert_eq!(
        second.graph.chains.keys().collect::<Vec<_>>(),
        vec![&id],
        "re-asserted, same id"
    );
    assert_eq!(second.graph.chains[&id].provenance.len(), 2);
}

#[test]
fn simplification_drops_supersets_and_shortcuts_covered_by_ancestry() {
    let names = ["a", "b", "c", "d"];
    let findings = names
        .iter()
        .map(|k| finding(k, k, &format!("Fact {k} holds."), None))
        .collect();
    let extraction = Extraction {
        findings,
        chains: vec![
            chain(&["a"], "b"),
            chain(&["b"], "c"),
            chain(&["a"], "c"),
            chain(&["a", "d"], "b"),
        ],
    };
    let outcome = merge(&Graph::default(), &extraction);
    let id = |k: &str| ids::proposition_id(&format!("Fact {k} holds."));
    let kept: BTreeSet<(Vec<String>, String)> = outcome
        .graph
        .chains
        .values()
        .map(|c| (c.premises.iter().cloned().collect(), c.conclusion.clone()))
        .collect();
    let expected: BTreeSet<_> = [(vec![id("a")], id("b")), (vec![id("b")], id("c"))]
        .into_iter()
        .collect();
    assert_eq!(kept, expected);
    assert_eq!(outcome.dropped_chains, 2);
}

#[test]
fn of_two_chains_that_cover_each_other_one_survives() {
    let mut chains = std::collections::BTreeMap::new();
    let set = |items: &[&str]| items.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>();
    for (premises, conclusion) in [
        (set(&["p_a"]), "p_b"),
        (set(&["p_b"]), "p_a"),
        (set(&["p_a"]), "p_c"),
        (set(&["p_b"]), "p_c"),
    ] {
        let id = ids::chain_id(&premises, conclusion);
        chains.insert(
            id,
            ChainNode {
                premises,
                conclusion: conclusion.into(),
                provenance: BTreeSet::new(),
            },
        );
    }
    let dropped = simplify(&mut chains);
    assert_eq!(
        dropped.len(),
        1,
        "a->c and b->c cover each other; the lesser id goes"
    );
    let a_c = ids::chain_id(&set(&["p_a"]), "p_c");
    let b_c = ids::chain_id(&set(&["p_b"]), "p_c");
    assert_eq!(dropped[0], a_c.clone().min(b_c));
    assert_eq!(chains.len(), 3);
}

fn open() -> (tempfile::TempDir, NoesisStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = NoesisStore::open(&dir.path().join("noesis.db")).unwrap();
    (dir, store)
}

fn sample() -> Extraction {
    Extraction {
        findings: vec![
            finding("a", "a", "Alpha holds.", None),
            finding("b", "b", "Beta holds.", None),
            finding("c", "c", "Gamma holds.", None),
        ],
        chains: vec![chain(&["a", "b"], "c")],
    }
}

#[test]
fn a_written_merge_loads_back_identically() {
    let (_dir, store) = open();
    let conn = store.connection();
    let first = merge(&Graph::default(), &sample()).graph;
    store::write_diff(conn, &Graph::default(), &first, &store::now_text()).unwrap();
    assert_eq!(store::load(conn).unwrap(), first);
    let mut more = sample();
    more.findings[0].aliases = vec!["first".into()];
    more.findings[0].origin = origin("s_src", "chunk-1");
    more.chains.clear();
    let second = merge(&first, &more).graph;
    store::write_diff(conn, &first, &second, &store::now_text()).unwrap();
    assert_eq!(store::load(conn).unwrap(), second);
}

#[test]
fn deleting_a_proposition_takes_its_chains() {
    let (_dir, store) = open();
    let conn = store.connection();
    let graph = merge(&Graph::default(), &sample()).graph;
    store::write_diff(conn, &Graph::default(), &graph, &store::now_text()).unwrap();
    assert!(store::delete_proposition(conn, &ids::proposition_id("Alpha holds.")).unwrap());
    let left = store::load(conn).unwrap();
    assert_eq!(left.nodes.len(), 2);
    assert!(left.chains.is_empty());
    assert!(!store::delete_proposition(conn, "p_nothing").unwrap());
}

#[test]
fn deleting_a_source_keeps_what_is_supported_elsewhere() {
    let (_dir, store) = open();
    let conn = store.connection();
    let mut from_source = sample();
    for f in &mut from_source.findings {
        f.origin = origin("s_src", "chunk-0");
    }
    from_source.chains[0].origin = origin("s_src", "chunk-0");
    let session = Extraction {
        findings: vec![finding("x", "a", "Alpha holds.", None)],
        chains: Vec::new(),
    };
    let graph = merge(&merge(&Graph::default(), &from_source).graph, &session).graph;
    store::write_diff(conn, &Graph::default(), &graph, &store::now_text()).unwrap();
    let row = store::SourceRow {
        id: "s_src".into(),
        name: "notes".into(),
        artifact: "a1".into(),
        imported_at: store::now_text(),
    };
    store::insert_source(conn, &row).unwrap();
    assert!(store::delete_source(conn, "s_src", true).unwrap());
    let left = store::load(conn).unwrap();
    assert_eq!(
        left.nodes.keys().collect::<Vec<_>>(),
        vec![&ids::proposition_id("Alpha holds.")]
    );
    assert!(left.chains.is_empty());
    assert!(!store::delete_source(conn, "s_src", true).unwrap());
}

#[test]
fn preserving_a_deleted_source_keeps_the_graph() {
    let (_dir, store) = open();
    let conn = store.connection();
    let graph = merge(&Graph::default(), &sample()).graph;
    store::write_diff(conn, &Graph::default(), &graph, &store::now_text()).unwrap();
    let row = store::SourceRow {
        id: "s_keep".into(),
        name: "n".into(),
        artifact: "a".into(),
        imported_at: store::now_text(),
    };
    assert_eq!(store::insert_source(conn, &row).unwrap(), row);
    assert!(store::delete_source(conn, "s_keep", false).unwrap());
    assert_eq!(store::load(conn).unwrap(), graph);
    assert!(store::source(conn, "s_keep").unwrap().is_none());
}

#[test]
fn a_snapshot_filters_drops_dangling_chains_and_clusters_by_birth_session() {
    let (_dir, store) = open();
    let conn = store.connection();
    let graph = merge(&Graph::default(), &sample()).graph;
    store::write_diff(conn, &Graph::default(), &graph, &store::now_text()).unwrap();
    let all = store::snapshot(conn, &SnapshotFilter::default()).unwrap();
    assert_eq!(
        (all.nodes.len(), all.chains.len(), all.truncated),
        (3, 1, false)
    );
    assert_eq!(all.clusters.len(), 1);
    assert_eq!(all.clusters[0].session, "ns_a");
    assert_eq!(all.clusters[0].members.len(), 3);
    let filter = SnapshotFilter {
        query: Some("ALPHA".into()),
        ..SnapshotFilter::default()
    };
    let some = store::snapshot(conn, &filter).unwrap();
    assert_eq!(some.nodes.len(), 1);
    assert!(
        some.chains.is_empty(),
        "a chain reaching outside the result is dropped"
    );
    assert!(
        some.clusters.is_empty(),
        "fewer than 3 members form no cluster"
    );
    let cut = store::snapshot(
        conn,
        &SnapshotFilter {
            limit: Some(2),
            ..SnapshotFilter::default()
        },
    )
    .unwrap();
    assert!(cut.truncated);
    let other = SnapshotFilter {
        source: Some("s_none".into()),
        ..SnapshotFilter::default()
    };
    assert!(store::snapshot(conn, &other).unwrap().nodes.is_empty());
}
