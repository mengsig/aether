//! Questions asked while native background jobs hold the turn: each is its
//! own question, and a held wait is answered only by what came after it.
use super::background::{Pending, held_call, hold, job_harness, reply_of};
use super::*;

fn pending_seqs(h: &Harness, agent: AgentId) -> Vec<u64> {
    aether_runtime::pending_questions(h.store.as_ref(), agent)
        .expect("pending")
        .iter()
        .map(|asked| asked.seq)
        .collect()
}

fn reply(h: &Harness, agent: AgentId, seq: u64, text: &str) {
    h.runtime
        .reply_to_question(
            agent,
            seq,
            vec![ContentBlock::text(text)],
            Actor::user(),
            None,
        )
        .expect("the reply is accepted");
}

/// The held wait is still pending a moment later.
async fn still_held(pending: &Pending) {
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!pending.is_finished(), "the held wait ended early");
}

/// A second `ask_user` while the first is held is recorded as its own
/// question, and the held wait stays held until the first one's answer.
#[tokio::test]
async fn a_second_question_beside_a_held_one_is_recorded_and_the_wait_kept() {
    let first = json!({"question": "which half?"});
    let (h, turn, held) = held_call(job_harness().await, "ask_user", first).await;
    let agent = turn.agent;
    let (answer, is_error) =
        raw_call(&h, &turn, "ask_user", json!({"question": "which colour?"})).await;
    assert!(!is_error, "a second question was refused: {answer}");
    assert!(
        answer.contains("on its own") && answer.contains("kept"),
        "{answer}"
    );
    let asked = pending_seqs(&h, agent);
    assert_eq!(asked.len(), 2, "both questions are pending: {asked:?}");
    still_held(&held).await;
    assert!(!turn.observed.stopped(), "the jobs keep running");

    // The second question's reply is not the first one's answer.
    reply(&h, agent, asked[1], "blue");
    still_held(&held).await;
    assert_eq!(pending_seqs(&h, agent), [asked[0]]);

    reply(&h, agent, asked[0], "the left half");
    let woken = reply_of(held).await;
    assert!(
        woken.starts_with(&format!(
            "Wait ended: the operator answered question {}.",
            asked[0]
        )),
        "{woken}"
    );
    assert!(
        woken.contains("blue") && woken.contains("the left half"),
        "both answers are handed over: {woken}"
    );
    assert!(pending_seqs(&h, agent).is_empty());
    assert!(!turn.observed.stopped() && !turn.observed.stdin_closed());
    h.runtime.shutdown().await;
}

/// Replies to other questions landing while the driver polls the held wait
/// never release it: each is classified with the events committed beside it.
#[tokio::test]
async fn replies_racing_the_held_wait_never_release_it() {
    let first = json!({"question": "which half?"});
    let (h, turn, held) = held_call(job_harness().await, "ask_user", first).await;
    let agent = turn.agent;
    for n in 0..6 {
        let question = json!({"question": format!("which shade {n}?")});
        let (answer, is_error) = raw_call(&h, &turn, "ask_user", question).await;
        assert!(!is_error, "{answer}");
    }
    let asked = pending_seqs(&h, agent);
    assert_eq!(asked.len(), 7, "every question is pending: {asked:?}");
    for (n, seq) in asked[1..].iter().enumerate() {
        reply(&h, agent, *seq, &format!("shade {n}"));
        tokio::task::yield_now().await;
    }
    still_held(&held).await;
    assert_eq!(pending_seqs(&h, agent), [asked[0]]);

    reply(&h, agent, asked[0], "the left half");
    let woken = reply_of(held).await;
    assert!(
        woken.starts_with(&format!(
            "Wait ended: the operator answered question {}.",
            asked[0]
        )),
        "{woken}"
    );
    h.runtime.shutdown().await;
}

/// An answer that came for an earlier question is that question's answer:
/// it never ends the wait of a question asked after it.
#[tokio::test]
async fn an_early_answer_to_one_question_never_satisfies_the_next() {
    let mut h = job_harness().await;
    let agent = start_coordinator(&h);
    let turn = turn_of(&mut h, agent).await;
    let run = h.store.agent(agent).expect("agent").expect("exists").run;
    let earlier = h
        .runtime
        .apply(
            Commit::new().event(
                aether_core::event::EventDraft::new(
                    Actor::Agent { id: agent },
                    EventKind::QuestionAsked {
                        question: "which half?".into(),
                    },
                )
                .agent(agent)
                .run(run),
            ),
        )
        .expect("record the first question")[0]
        .seq;
    reply(&h, agent, earlier, "the left half");

    let held = hold(&h, &turn, "ask_user", json!({"question": "which colour?"})).await;
    still_held(&held).await;
    let later = pending_seqs(&h, agent);
    assert_eq!(later.len(), 1, "only the new question is open: {later:?}");
    assert!(later[0] > earlier);

    reply(&h, agent, later[0], "blue");
    let woken = reply_of(held).await;
    assert!(
        woken.starts_with(&format!(
            "Wait ended: the operator answered question {}.",
            later[0]
        )),
        "{woken}"
    );
    assert!(pending_seqs(&h, agent).is_empty());
    h.runtime.shutdown().await;
}
