use super::*;
use aether_core::id::{MemoryFeedbackId, TaskId};
use aether_core::memory_feedback::{
    FeedbackTally, MemoryEvidence, MemoryFeedback, MemoryFeedbackSpec, MemoryVerdict,
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
fn the_tally_query_reads_feedback_through_the_memory_index() {
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
        .query_map(rusqlite::params![ids, r#"["review"]"#, 20], |row| {
            row.get(3)
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    eprintln!("{}", details.join("\n"));
    assert!(
        details.iter().any(|d| d
            == "SEARCH memory_feedback USING INDEX memory_feedback_memory_created (memory_id=?)")
    );
    assert!(
        !details
            .iter()
            .any(|d| d.starts_with("SCAN memory_feedback"))
    );
}
