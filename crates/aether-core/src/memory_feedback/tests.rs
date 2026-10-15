use super::*;
use chrono::{Duration, TimeZone, Utc};

fn vote(memory: MemoryId, task: TaskId, verdict: MemoryVerdict, minute: i64) -> MemoryFeedback {
    MemoryFeedback {
        id: MemoryFeedbackId::new(),
        spec: MemoryFeedbackSpec {
            memory,
            verdict,
            evidence: MemoryEvidence::Review,
            task,
            invocation: None,
            reason: "Checked".into(),
        },
        actor: Actor::user(),
        created_at: Utc.with_ymd_and_hms(2026, 10, 15, 9, 0, 0).unwrap()
            + Duration::minutes(minute),
        retracted_at: None,
        retraction_reason: None,
    }
}

fn tally(helped: u32, hurt: u32) -> FeedbackTally {
    FeedbackTally { helped, hurt }
}

#[test]
fn the_factor_is_bounded_and_exactly_one_when_even() {
    for helped in 0..=RANKING_WINDOW as u32 {
        for hurt in 0..=RANKING_WINDOW as u32 - helped {
            let factor = tally(helped, hurt).factor();
            assert!((0.75..=1.25).contains(&factor), "{helped}/{hurt}: {factor}");
            if helped == hurt {
                assert_eq!(factor, 1.0);
            }
        }
    }
    assert_eq!(tally(20, 0).factor(), 0.75 + 0.5 * 21.0 / 22.0);
    assert_eq!(tally(0, 20).factor(), 0.75 + 0.5 * 1.0 / 22.0);
}

#[test]
fn ranking_activates_at_two_helped_or_hurt_verdicts() {
    assert_eq!(tally(0, 0).factor(), 1.0);
    assert_eq!(tally(1, 0).factor(), 1.0);
    assert_eq!(tally(0, 1).factor(), 1.0);
    assert!(!tally(1, 0).is_active());
    assert!(tally(1, 1).is_active());
    assert_eq!(tally(2, 0).factor(), 1.125);
    assert_eq!(tally(0, 2).factor(), 0.875);
    assert_eq!(tally(3, 1).factor(), 0.75 + 0.5 * 4.0 / 6.0);
    let ranking = FeedbackRanking::from(tally(1, 0));
    assert_eq!((ranking.helped, ranking.hurt, ranking.factor), (1, 0, 1.0));
    assert_eq!(FeedbackRanking::default().factor, 1.0);
}

#[test]
fn neutral_counts_toward_neither_but_holds_its_task() {
    let memory = MemoryId::new();
    let task = TaskId::new();
    let records = [
        vote(memory, task, MemoryVerdict::Helped, 0),
        vote(memory, task, MemoryVerdict::Neutral, 1),
        vote(memory, TaskId::new(), MemoryVerdict::Neutral, 2),
        vote(memory, TaskId::new(), MemoryVerdict::Helped, 3),
    ];
    assert_eq!(FeedbackTally::of(&records), tally(1, 0));
}

#[test]
fn a_task_voting_several_times_counts_once_with_its_latest_verdict() {
    let memory = MemoryId::new();
    let task = TaskId::new();
    let mut records: Vec<_> = (0..5)
        .map(|minute| vote(memory, task, MemoryVerdict::Helped, minute))
        .collect();
    records.push(vote(memory, task, MemoryVerdict::Hurt, 9));
    assert_eq!(FeedbackTally::of(&records), tally(0, 1));
    records.reverse();
    assert_eq!(FeedbackTally::of(&records), tally(0, 1));
}

#[test]
fn equal_times_break_by_the_larger_id() {
    let memory = MemoryId::new();
    let task = TaskId::new();
    let mut a = vote(memory, task, MemoryVerdict::Helped, 0);
    let mut b = vote(memory, task, MemoryVerdict::Hurt, 0);
    if a.id < b.id {
        std::mem::swap(&mut a.id, &mut b.id);
    }
    assert!(a.id > b.id);
    assert_eq!(FeedbackTally::of(&[b.clone(), a.clone()]), tally(1, 0));
    assert_eq!(FeedbackTally::of(&[a, b]), tally(1, 0));
}

#[test]
fn only_the_twenty_most_recent_tasks_count() {
    let memory = MemoryId::new();
    let mut records: Vec<_> = (0..2)
        .map(|minute| vote(memory, TaskId::new(), MemoryVerdict::Hurt, minute))
        .collect();
    records
        .extend((2..22).map(|minute| vote(memory, TaskId::new(), MemoryVerdict::Neutral, minute)));
    assert_eq!(FeedbackTally::of(&records), tally(0, 0));
    records.truncate(21);
    assert_eq!(FeedbackTally::of(&records), tally(0, 1));
    // A new vote moves its task to the front and keeps one slot, so the
    // oldest other task falls out of the window.
    let oldest = records[0].spec.task;
    records.push(vote(memory, oldest, MemoryVerdict::Helped, 30));
    assert_eq!(FeedbackTally::of(&records), tally(1, 0));
}

#[test]
fn a_retraction_restores_the_same_tasks_previous_vote() {
    let memory = MemoryId::new();
    let task = TaskId::new();
    let other = vote(memory, TaskId::new(), MemoryVerdict::Helped, 0);
    let first = vote(memory, task, MemoryVerdict::Helped, 1);
    let mut second = vote(memory, task, MemoryVerdict::Hurt, 2);
    let records = [other.clone(), first.clone(), second.clone()];
    assert_eq!(FeedbackTally::of(&records), tally(1, 1));
    second.retracted_at = Some(second.created_at);
    second.retraction_reason = Some("Wrong build".into());
    assert_eq!(FeedbackTally::of(&[other, first, second]), tally(2, 0));
}

#[test]
fn outcome_evidence_is_ignored() {
    let memory = MemoryId::new();
    let task = TaskId::new();
    let mut outcome = vote(memory, task, MemoryVerdict::Hurt, 5);
    outcome.spec.evidence = MemoryEvidence::Outcome;
    let mut operator = vote(memory, task, MemoryVerdict::Helped, 1);
    operator.spec.evidence = MemoryEvidence::Operator;
    assert_eq!(FeedbackTally::of(&[operator, outcome]), tally(1, 0));
    assert!(!MemoryEvidence::Outcome.counts_for_ranking());
}
