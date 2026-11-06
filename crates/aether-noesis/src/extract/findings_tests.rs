use std::collections::BTreeSet;

use serde_json::{Value, json};

use super::findings::{MAX_ITEMS, submission};
use crate::graph::{Extraction, Graph, Node, ids, merge};

fn propositions() -> Value {
    json!([
        {"key": "a", "name": "Closed switch", "statement": "The switch closes the circuit.",
         "aliases": ["connected circuit"], "anchor": "m1"},
        {"key": "b", "name": "Lamp current", "statement": "Current flows through the lamp.",
         "anchor": "m1"}
    ])
}

fn parsed(reply: Value, graph: &Graph) -> super::findings::Submission {
    submission(
        &reply.to_string(),
        "source",
        &BTreeSet::from(["m1".into()]),
        graph,
    )
    .expect("submission")
}

#[test]
fn chain_endpoints_resolve_keys_names_statements_aliases_and_objects() {
    let graph = Graph::default();
    for (premise, conclusion) in [
        (json!("a"), json!("b")),
        (json!(" Closed   SWITCH "), json!("Lamp current")),
        (
            json!("The switch closes the circuit."),
            json!("Current flows through the lamp."),
        ),
        (json!("connected circuit"), json!({"key": "b"})),
        (
            json!({"name": "Closed switch"}),
            json!({"statement": "Current flows through the lamp."}),
        ),
    ] {
        let out = parsed(
            json!({"propositions": propositions(), "chains": [
                {"premises": [premise], "conclusion": conclusion, "anchor": "m1"}
            ]}),
            &graph,
        );
        assert_eq!(out.rejected, 0);
        let merged = merge(
            &graph,
            &Extraction {
                findings: out.findings,
                chains: out.chains,
            },
        );
        assert_eq!(merged.chains, 1);
        assert_eq!(merged.dropped_chains, 0);
        assert!(merged.graph.check_structure().is_ok());
    }
}

#[test]
fn alternate_chain_fields_and_nested_chains_keep_the_same_inference() {
    let top = json!({"propositions": propositions(), "links": [
        {"premise_ids": ["a"], "conclusion_id": "b", "anchor": "m1"}
    ]});
    let mut nested = json!({"propositions": propositions()});
    nested["propositions"][1]["chains"] = json!([{"premises": ["a"]}]);
    let graph = Graph::default();
    let expected = parsed(top, &graph);
    assert_eq!(expected.chains.len(), 1);
    assert_eq!(parsed(nested, &graph), expected);
}

#[test]
fn endpoint_resolution_never_guesses_ambiguous_unknown_or_numeric_references() {
    let mut props = propositions();
    props[0]["aliases"] = json!(["shared label"]);
    props[1]["aliases"] = json!(["shared label"]);
    for premise in [
        json!("shared label"),
        json!("missing"),
        json!(0),
        json!({"key": "a", "name": "Lamp current"}),
        json!({"index": 0}),
    ] {
        let out = parsed(
            json!({"propositions": props, "chains": [
                {"premises": ["a", premise], "conclusion": "b", "anchor": "m1"}
            ]}),
            &Graph::default(),
        );
        assert_eq!(out.findings.len(), 2);
        assert_eq!(out.rejected, 1);
        assert!(
            out.chains.is_empty(),
            "a missing premise cannot be dropped to weaken the inference"
        );
    }
}

#[test]
fn explicit_numeric_keys_work_without_guessing_zero_or_one_based_indices() {
    let mut props = propositions();
    props[0]["key"] = json!("0");
    props[1]["key"] = json!("1");
    let out = parsed(
        json!({"propositions": props, "chains": [
            {"premises": ["0"], "conclusion": "1", "anchor": "m1"}
        ]}),
        &Graph::default(),
    );
    assert_eq!(out.rejected, 0);
    assert_eq!(out.chains.len(), 1);
}

#[test]
fn existing_names_and_audited_local_aliases_survive_merge() {
    let statement = "A connected switch completes the circuit.";
    let id = ids::proposition_id(statement);
    let graph = Graph {
        nodes: [(
            id.clone(),
            Node {
                name: "Complete circuit".into(),
                statement: statement.into(),
                aliases: BTreeSet::from(["switch connection".into()]),
                provenance: BTreeSet::new(),
            },
        )]
        .into(),
        ..Graph::default()
    };
    for premise in [
        json!("connected circuit"),
        json!("Complete circuit"),
        json!({"id": id}),
        json!(statement),
        json!("switch connection"),
    ] {
        let mut out = parsed(
            json!({"propositions": propositions(), "chains": [
                {"premises": [premise], "conclusion": "b", "anchor": "m1"}
            ]}),
            &graph,
        );
        assert_eq!(out.chains.len(), 1);
        out.findings[0].link = Some(id.clone());
        let merged = merge(
            &graph,
            &Extraction {
                findings: out.findings,
                chains: out.chains,
            },
        );
        assert_eq!(merged.graph.nodes.len(), 2);
        assert_eq!(merged.chains, 1);
        assert_eq!(
            merged.graph.chains.values().next().unwrap().premises,
            BTreeSet::from([id.clone()])
        );
    }
}

#[test]
fn nested_chains_cannot_bypass_limits_or_anchor_validation() {
    let mut reply = json!({"propositions": propositions()});
    reply["propositions"][1]["chains"] = json!([{"premises": ["a"], "anchor": "outside"}]);
    let out = parsed(reply.clone(), &Graph::default());
    assert_eq!(out.rejected, 1);
    assert!(out.chains.is_empty());
    reply["propositions"][1]["chains"] = json!(vec![json!({"premises": ["a"]}); MAX_ITEMS + 1]);
    let result = submission(
        &reply.to_string(),
        "source",
        &BTreeSet::from(["m1".into()]),
        &Graph::default(),
    );
    assert!(result.unwrap_err().contains("more than"));
}

#[test]
fn duplicate_wordings_and_existing_copies_do_not_make_labels_ambiguous() {
    let mut props = propositions();
    let mut duplicate = props[0].clone();
    duplicate["key"] = json!("same");
    duplicate["statement"] = json!(" THE SWITCH closes the circuit ");
    props.as_array_mut().unwrap().push(duplicate);
    let statement = props[0]["statement"].as_str().unwrap();
    let id = ids::proposition_id(statement);
    let graph = Graph {
        nodes: [(
            id.clone(),
            Node {
                name: "Closed switch".into(),
                statement: statement.into(),
                aliases: BTreeSet::new(),
                provenance: BTreeSet::new(),
            },
        )]
        .into(),
        ..Graph::default()
    };
    let out = parsed(
        json!({"propositions": props, "chains": [
            {"premises": [{"id": id, "name": "Closed switch"}], "conclusion": "b", "anchor": "m1"}
        ]}),
        &graph,
    );
    assert_eq!(out.rejected, 0);
    assert_eq!(out.chains.len(), 1);
    let merged = merge(
        &graph,
        &Extraction {
            findings: out.findings,
            chains: out.chains,
        },
    );
    assert_eq!(merged.graph.nodes.len(), 2);
    assert_eq!(merged.chains, 1);
}
