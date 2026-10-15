use super::*;
use aether_core::id::{MemoryFeedbackId, TaskId};
use aether_core::memory_feedback::{
    FeedbackTally, MemoryEvidence, MemoryFeedback, MemoryFeedbackSpec, MemoryVerdict,
    RANKED_EVIDENCE,
};
use chrono::Duration;

struct Votes {
    store: SqliteStore,
    start: Timestamp,
}

impl Votes {
    fn new() -> Self {
        Self {
            store: SqliteStore::open_in_memory().unwrap(),
            start: now(),
        }
    }

    fn cast(
        &self,
        memory: MemoryId,
        task: TaskId,
        verdict: MemoryVerdict,
        minute: i64,
    ) -> MemoryFeedback {
        self.cast_as(memory, task, verdict, MemoryEvidence::Review, minute)
    }

    fn cast_as(
        &self,
        memory: MemoryId,
        task: TaskId,
        verdict: MemoryVerdict,
        evidence: MemoryEvidence,
        minute: i64,
    ) -> MemoryFeedback {
        let record = self.unsaved(memory, task, verdict, evidence, minute);
        self.insert(&record);
        record
    }

    fn unsaved(
        &self,
        memory: MemoryId,
        task: TaskId,
        verdict: MemoryVerdict,
        evidence: MemoryEvidence,
        minute: i64,
    ) -> MemoryFeedback {
        MemoryFeedback {
            id: MemoryFeedbackId::new(),
            spec: MemoryFeedbackSpec {
                memory,
                verdict,
                evidence,
                task,
                invocation: None,
                reason: "Checked".into(),
            },
            actor: Actor::user(),
            created_at: self.start + Duration::minutes(minute),
            retracted_at: None,
            retraction_reason: None,
        }
    }

    fn insert(&self, record: &MemoryFeedback) {
        let commit = Commit::new().mutate(Mutation::RecordMemoryFeedback(record.clone()));
        self.store.commit(commit).unwrap();
    }

    fn retract(&self, record: &MemoryFeedback) {
        let commit = Commit::new().mutate(Mutation::RetractMemoryFeedback {
            id: record.id,
            at: now(),
            reason: "Wrong build".into(),
        });
        self.store.commit(commit).unwrap();
    }

    fn tally(&self, memory: MemoryId) -> Option<FeedbackTally> {
        let tallies = self.store.memory_feedback_tallies(&[memory]).unwrap();
        let reference = FeedbackTally::of(&self.store.memory_feedback(memory, 1000).unwrap());
        if let Some(found) = tallies.get(&memory) {
            assert_eq!(*found, reference, "the query and the rule disagree");
        }
        tallies.get(&memory).copied()
    }
}

fn tally(helped: u32, hurt: u32) -> Option<FeedbackTally> {
    Some(FeedbackTally { helped, hurt })
}

#[test]
fn each_task_counts_its_latest_verdict_and_equal_times_break_by_id() {
    let votes = Votes::new();
    let memory = MemoryId::new();
    let task = TaskId::new();
    votes.cast(memory, task, MemoryVerdict::Helped, 0);
    votes.cast(memory, task, MemoryVerdict::Helped, 1);
    votes.cast(memory, task, MemoryVerdict::Hurt, 2);
    assert_eq!(votes.tally(memory), tally(0, 1));
    // Each tied pair goes in in a different order, so row order cannot decide.
    for high_first in [true, false] {
        let tied = TaskId::new();
        let mut low = votes.unsaved(memory, tied, MemoryVerdict::Hurt, MemoryEvidence::Review, 3);
        let mut high = votes.unsaved(
            memory,
            tied,
            MemoryVerdict::Helped,
            MemoryEvidence::Review,
            3,
        );
        if high.id < low.id {
            std::mem::swap(&mut high.id, &mut low.id);
        }
        let order = if high_first {
            [&high, &low]
        } else {
            [&low, &high]
        };
        order.into_iter().for_each(|record| votes.insert(record));
    }
    assert_eq!(votes.tally(memory), tally(2, 1));
}

#[test]
fn only_the_twenty_most_recent_tasks_count() {
    let votes = Votes::new();
    let memory = MemoryId::new();
    for minute in 0..2 {
        votes.cast(memory, TaskId::new(), MemoryVerdict::Hurt, minute);
    }
    for minute in 2..22 {
        votes.cast(memory, TaskId::new(), MemoryVerdict::Helped, minute);
    }
    assert_eq!(votes.tally(memory), tally(20, 0));
    let recent = TaskId::new();
    votes.cast(memory, recent, MemoryVerdict::Neutral, 30);
    assert_eq!(votes.tally(memory), tally(19, 0));
}

#[test]
fn retracted_and_outcome_feedback_is_not_counted() {
    let votes = Votes::new();
    let memory = MemoryId::new();
    let task = TaskId::new();
    votes.cast(memory, task, MemoryVerdict::Helped, 0);
    let wrong = votes.cast(memory, task, MemoryVerdict::Hurt, 1);
    votes.retract(&wrong);
    let other = TaskId::new();
    votes.cast_as(
        memory,
        other,
        MemoryVerdict::Hurt,
        MemoryEvidence::Operator,
        2,
    );
    votes.cast_as(
        memory,
        other,
        MemoryVerdict::Helped,
        MemoryEvidence::Outcome,
        3,
    );
    assert_eq!(votes.tally(memory), tally(1, 1));
    let unseen = MemoryId::new();
    votes.cast_as(
        unseen,
        task,
        MemoryVerdict::Hurt,
        MemoryEvidence::Outcome,
        0,
    );
    let retracted = votes.cast(unseen, other, MemoryVerdict::Hurt, 1);
    votes.retract(&retracted);
    assert_eq!(votes.tally(unseen), None);
}

#[test]
fn one_read_tallies_several_memories_and_none_for_an_empty_list() {
    let votes = Votes::new();
    let (helped, hurt, neutral, silent) = (
        MemoryId::new(),
        MemoryId::new(),
        MemoryId::new(),
        MemoryId::new(),
    );
    let task = TaskId::new();
    for (memory, verdict) in [
        (helped, MemoryVerdict::Helped),
        (hurt, MemoryVerdict::Hurt),
        (neutral, MemoryVerdict::Neutral),
    ] {
        votes.cast(memory, task, verdict, 0);
        votes.cast(memory, TaskId::new(), verdict, 1);
    }
    votes.cast(MemoryId::new(), task, MemoryVerdict::Helped, 9);
    let tallies = votes
        .store
        .memory_feedback_tallies(&[helped, hurt, neutral, silent, helped])
        .unwrap();
    assert_eq!(tallies.len(), 3, "{tallies:?}");
    assert_eq!(tallies[&helped], FeedbackTally { helped: 2, hurt: 0 });
    assert_eq!(tallies[&hurt], FeedbackTally { helped: 0, hurt: 2 });
    assert_eq!(tallies[&neutral], FeedbackTally::default());
    assert!(votes.store.memory_feedback_tallies(&[]).unwrap().is_empty());
}

#[test]
fn the_query_agrees_with_the_rule_on_mixed_history() {
    let votes = Votes::new();
    let memories: Vec<MemoryId> = (0..4).map(|_| MemoryId::new()).collect();
    let tasks: Vec<TaskId> = (0..30).map(|_| TaskId::new()).collect();
    let verdicts = [
        MemoryVerdict::Helped,
        MemoryVerdict::Hurt,
        MemoryVerdict::Neutral,
    ];
    let evidence = [
        MemoryEvidence::Operator,
        MemoryEvidence::Review,
        MemoryEvidence::Outcome,
    ];
    let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
    for minute in 0..400 {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        let pick = |shift: u32, len: usize| ((state >> shift) as usize) % len;
        let record = votes.cast_as(
            memories[pick(5, memories.len())],
            tasks[pick(13, tasks.len())],
            verdicts[pick(23, 3)],
            evidence[pick(31, 3)],
            minute / 3,
        );
        if pick(41, 7) == 0 {
            votes.retract(&record);
        }
    }
    let tallies = votes.store.memory_feedback_tallies(&memories).unwrap();
    assert_eq!(tallies.len(), memories.len());
    for memory in &memories {
        let records = votes.store.memory_feedback(*memory, 1000).unwrap();
        assert_eq!(tallies[memory], FeedbackTally::of(&records), "{memory}");
    }
}

#[test]
fn the_tally_query_reads_feedback_through_the_ranked_index() {
    let store = SqliteStore::open_in_memory().unwrap();
    let conn = store.read_conn();
    let mut plan = conn
        .prepare(&format!(
            "EXPLAIN QUERY PLAN {}",
            super::super::memory_feedback::TALLIES
        ))
        .unwrap();
    let ids = serde_json::to_string(&[MemoryId::new().to_string()]).unwrap();
    let details: Vec<String> = plan
        .query_map(rusqlite::params![ids, 20], |row| row.get(3))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    eprintln!("{}", details.join("\n"));
    assert!(
        details.iter().any(|d| d
            == "SEARCH memory_feedback USING INDEX memory_feedback_ranked (memory_id=?)")
    );
    assert!(
        !details
            .iter()
            .any(|d| d.starts_with("SCAN memory_feedback"))
    );
    assert!(
        details
            .iter()
            .filter(|d| d.contains("TEMP B-TREE") && d.contains("ORDER BY"))
            .count()
            == 1,
        "the index must supply the first window's ordering: {details:?}"
    );
}

#[test]
fn the_ranked_index_covers_the_read_without_fetching_table_columns() {
    let store = SqliteStore::open_in_memory().unwrap();
    let conn = store.read_conn();
    let table_root: i64 = conn
        .query_row(
            "SELECT rootpage FROM sqlite_master WHERE name = 'memory_feedback'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let mut statement = conn
        .prepare(&format!(
            "EXPLAIN {}",
            super::super::memory_feedback::TALLIES
        ))
        .unwrap();
    let instructions: Vec<(String, i64, i64)> = statement
        .query_map(rusqlite::params!["[]", 20], |r| {
            Ok((r.get(1)?, r.get(2)?, r.get(3)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    let table_cursors: Vec<i64> = instructions
        .iter()
        .filter(|(op, _, root)| op == "OpenRead" && *root == table_root)
        .map(|(_, cursor, _)| *cursor)
        .collect();
    assert!(table_cursors.len() <= 1, "{table_cursors:?}");
    assert!(
        instructions.iter().all(|(op, cursor, _)| {
            !table_cursors.contains(cursor)
                || !matches!(op.as_str(), "Column" | "Rowid" | "SeekRowid")
        }),
        "the partial index must cover the read: {instructions:?}"
    );
}

#[test]
fn ranked_sql_evidence_literals_match_the_core_rule() {
    use std::collections::BTreeSet;
    let store = SqliteStore::open_in_memory().unwrap();
    let conn = store.read_conn();
    let index: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name = 'memory_feedback_ranked'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let expected: BTreeSet<String> = RANKED_EVIDENCE
        .iter()
        .map(|e| {
            serde_json::to_value(e)
                .unwrap()
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    let query = super::super::memory_feedback::TALLIES;
    assert!(!query.contains("json_extract"));
    for sql in [query, &index] {
        let literals = sql
            .split_once("evidence IN (")
            .unwrap()
            .1
            .split_once(')')
            .unwrap()
            .0;
        let actual: BTreeSet<String> = literals
            .split(',')
            .map(|s| s.trim().trim_matches('\'').to_owned())
            .collect();
        assert_eq!(actual, expected, "{sql}");
    }
}

#[test]
fn feedback_columns_match_the_full_record_for_every_verdict_and_evidence() {
    let votes = Votes::new();
    for verdict in [
        MemoryVerdict::Helped,
        MemoryVerdict::Hurt,
        MemoryVerdict::Neutral,
    ] {
        for evidence in [
            MemoryEvidence::Operator,
            MemoryEvidence::Review,
            MemoryEvidence::Outcome,
        ] {
            let record = votes.cast_as(MemoryId::new(), TaskId::new(), verdict, evidence, 0);
            let conn = votes.store.read_conn();
            let (task, verdict, evidence, body): (String, String, String, String) = conn
                .query_row(
                    "SELECT task_id, verdict, evidence, body FROM memory_feedback WHERE id = ?1",
                    [record.id.to_string()],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .unwrap();
            assert_eq!(
                serde_json::from_str::<MemoryFeedback>(&body).unwrap(),
                record
            );
            let fields: serde_json::Value = serde_json::from_str(&body).unwrap();
            assert_eq!(fields["task"], task);
            assert_eq!(fields["verdict"], verdict);
            assert_eq!(fields["evidence"], evidence);
        }
    }
}
