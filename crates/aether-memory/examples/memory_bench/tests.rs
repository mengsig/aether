#![allow(clippy::expect_used, clippy::unwrap_used)]

use crate::plan::Plan;
use crate::run;

fn plan() -> Plan {
    let plan = serde_json::json!({
        "index": "none",
        "memory_max": 2000,
        "latency": {"reps": 2, "filler": 40},
        "memories": [
            {"key": "a/old", "scope": {"scope": "repository", "id": "github.com/example-org/a"},
             "kind": "fact", "text": "The service listens on port 8080 by default.",
             "importance": 0.6, "confidence": 0.8, "age_days": 90.0},
            {"key": "a/new", "scope": {"scope": "repository", "id": "github.com/example-org/a"},
             "kind": "fact", "text": "The service listens on port 9090 by default.",
             "importance": 0.6, "confidence": 0.9, "supersedes": "a/old",
             "feedback": {"helped": 2}},
            {"key": "a/again", "scope": {"scope": "repository", "id": "github.com/example-org/a"},
             "kind": "fact", "text": "The service listens on port 9090 by default.",
             "importance": 0.5, "confidence": 0.5},
            {"key": "a/transient", "scope": {"scope": "global"}, "kind": "fact",
             "text": "The build is currently broken on main.", "importance": 0.5, "confidence": 0.5},
        ],
        "queries": [
            {"id": "port", "repository": "github.com/example-org/a", "role": "worker",
             "query": "Which port does the service listen on?", "limit": 6},
        ],
    });
    let plan: Plan = serde_json::from_value(plan).unwrap();
    plan.validate().unwrap();
    plan
}

#[test]
fn a_run_admits_ranks_restarts_and_probes_damage() {
    let sandbox = tempfile::tempdir().unwrap();
    let report = run(&plan(), sandbox.path()).unwrap();
    let outcomes: Vec<&str> = report.admissions.iter().map(|a| a.outcome).collect();
    assert_eq!(outcomes, ["accepted", "superseded", "rejected", "rejected"]);
    assert_eq!(report.admissions[1].replaced.as_deref(), Some("a/old"));
    assert_eq!(report.current, ["a/new"], "read back from the store");
    let ranked: Vec<&str> = report.queries[0]
        .ranked
        .iter()
        .map(|r| r.key.as_str())
        .collect();
    assert_eq!(ranked, ["a/new"], "the superseded fact is never retrieved");
    assert!(report.queries[0].ranked[0].injected);
    assert!(report.queries[0].layer_tokens > report.queries[0].ranked[0].tokens);
    assert_eq!(report.storage.records, 2);
    assert_eq!(report.storage.current, 1);
    assert_eq!(report.restart.identical, 1);
    assert_eq!(report.restart.current_after, 1);
    let control = &report.corruption[0];
    assert_eq!((control.case, control.recovered), ("intact_copy", Some(1)));
    assert!(report.corruption.iter().all(|c| c.expected == 1));
    assert_eq!(report.latency.small[0].micros.len(), 2);
    assert_eq!(report.latency.scaled.memories, 41);
}

#[test]
fn a_plan_that_supersedes_a_later_key_is_refused() {
    let mut plan = plan();
    plan.memories.swap(0, 1);
    let error = plan.validate().unwrap_err().to_string();
    assert!(error.contains("supersedes a/old"), "{error}");
}

#[test]
fn a_sandbox_that_is_not_empty_is_refused() {
    let sandbox = tempfile::tempdir().unwrap();
    std::fs::write(sandbox.path().join("stray"), b"x").unwrap();
    let error = run(&plan(), sandbox.path()).unwrap_err().to_string();
    assert!(error.contains("must be an empty directory"), "{error}");
}

#[test]
fn a_consolidating_run_merges_restatements_before_it_ranks() {
    let mut plan = plan();
    plan.consolidate = true;
    plan.memories.push(
        serde_json::from_value(serde_json::json!({
            "key": "a/restated", "scope": {"scope": "repository", "id": "github.com/example-org/a"},
            "kind": "lesson", "importance": 0.5, "confidence": 0.8,
            "text": "By default the service listens on port 9090, so point clients at that port."
        }))
        .unwrap(),
    );
    let sandbox = tempfile::tempdir().unwrap();
    let report = run(&plan, sandbox.path()).unwrap();
    assert_eq!(report.admissions.last().unwrap().outcome, "accepted");
    assert_eq!(report.consolidated, 1);
    assert_eq!(report.current, ["a/new"], "the restatement merged into it");
    assert_eq!(report.storage.records, 3, "and was kept");
    assert_eq!(report.restart.current_after, 1);
}
