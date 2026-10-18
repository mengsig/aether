use super::*;
use aether_core::agent::{AgentStatus, ArchiveTarget};
use aether_core::event::{EventDraft, EventKind};

fn ask(fx: &Fixture, agent: AgentId) -> u64 {
    let run = fx.store.agent(agent).expect("read").expect("agent").run;
    fx.runtime
        .apply(
            Commit::new().event(
                EventDraft::new(
                    Actor::agent(agent),
                    EventKind::QuestionAsked {
                        question: "Proceed?".into(),
                    },
                )
                .agent(agent)
                .run(run)
                .parent(fx.store.agent(agent).expect("read").expect("agent").parent),
            ),
        )
        .expect("ask")[0]
        .seq
}

fn submission(key: &str, to: AgentId, question_seq: u64, content: &str) -> Submission {
    Submission::new(
        key,
        &json!({
            "cmd": "reply_to_question", "to": to, "question_seq": question_seq, "content": content,
        }),
    )
    .expect("key")
}

fn answer(
    fx: &Fixture,
    agent: AgentId,
    seq: u64,
    key: Option<&str>,
) -> aether_runtime::Result<MessageId> {
    fx.runtime.reply_to_question(
        agent,
        seq,
        text("yes"),
        Actor::user(),
        key.map(|key| submission(key, agent, seq, "yes")),
    )
}

#[tokio::test]
async fn fresh_question_is_answered_once_even_before_inbox_consumption() {
    let fx = fixture().await;
    let agent = an_agent(&fx).await;
    let seq = ask(&fx, agent);
    answer(&fx, agent, seq, None).expect("fresh question");
    let before = fx.store.last_seq().expect("seq");
    assert!(answer(&fx, agent, seq, None).is_err());
    assert!(answer(&fx, agent, seq, Some("another-key")).is_err());
    assert_eq!(fx.store.last_seq().expect("seq"), before);
    assert_eq!(fx.store.inbox(agent, 0).expect("inbox").len(), 1);
    assert!(
        fx.store
            .submission(&submission("another-key", agent, seq, "yes").key)
            .expect("record")
            .is_none()
    );
}

#[tokio::test]
async fn only_the_exact_asking_agents_latest_question_can_be_answered() {
    let fx = fixture().await;
    let parent = an_agent(&fx).await;
    let child = fx
        .runtime
        .create_agent(
            CreateAgentSpec {
                parent: Some(parent),
                ..agent_spec("worker")
            },
            Actor::user(),
        )
        .expect("child")
        .0
        .id;
    let parent_question = ask(&fx, parent);
    let old = ask(&fx, child);
    let latest = ask(&fx, child);
    for (agent, seq) in [
        (parent, latest),
        (child, old),
        (child, latest + 1),
        (child, 0),
    ] {
        assert!(answer(&fx, agent, seq, None).is_err(), "{agent} {seq}");
    }
    assert!(fx.store.inbox(child, 0).expect("inbox").is_empty());
    answer(&fx, parent, parent_question, None).expect("child did not supersede parent question");
    answer(&fx, child, latest, None).expect("latest question");
}

#[tokio::test]
async fn operator_messages_close_questions_but_agent_messages_do_not() {
    let fx = fixture().await;
    let agent = an_agent(&fx).await;
    let colleague = an_agent(&fx).await;
    let seq = ask(&fx, agent);
    // Cross a query page boundary without treating a colleague's note as an answer.
    for _ in 0..129 {
        fx.runtime
            .send_message(agent, text("note"), Actor::agent(colleague))
            .expect("agent note");
    }
    answer(&fx, agent, seq, None).expect("operator answer");
    for operator in [
        Actor::user(),
        Actor::Client {
            id: "operator-client".into(),
        },
    ] {
        let seq = ask(&fx, agent);
        fx.runtime
            .send_message(agent, text("manual answer"), operator)
            .expect("manual answer");
        assert!(answer(&fx, agent, seq, None).is_err());
    }
}

#[tokio::test]
async fn terminal_archived_and_pending_subtree_cancellation_are_refused() {
    let fx = fixture().await;
    for status in [
        AgentStatus::Completed { summary: None },
        AgentStatus::Failed {
            error: "failed".into(),
        },
        AgentStatus::Cancelled { reason: None },
    ] {
        let id = an_agent(&fx).await;
        let seq = ask(&fx, id);
        let mut agent = fx.store.agent(id).expect("read").expect("agent");
        agent.status = status;
        fx.runtime
            .apply(Commit::new().mutate(Mutation::UpsertAgent(agent)))
            .expect("terminal");
        assert!(answer(&fx, id, seq, None).is_err());
    }
    // Check the archive flag independently of the terminal-status guard.
    let id = an_agent(&fx).await;
    let seq = ask(&fx, id);
    let mut agent = fx.store.agent(id).expect("read").expect("agent");
    agent.archived = true;
    fx.runtime
        .apply(Commit::new().mutate(Mutation::UpsertAgent(agent)))
        .expect("archive");
    assert!(answer(&fx, id, seq, None).is_err());

    let parent = an_agent(&fx).await;
    let child = fx
        .runtime
        .create_agent(
            CreateAgentSpec {
                parent: Some(parent),
                ..agent_spec("worker")
            },
            Actor::user(),
        )
        .expect("child")
        .0
        .id;
    let seq = ask(&fx, child);
    fx.runtime
        .apply(Commit::new().mutate(Mutation::SetMeta {
            key: "runtime.pending_cancellations".into(),
            value:
                json!({parent.to_string(): {"by": Actor::user(), "reason": "cancel"}}).to_string(),
        }))
        .expect("durable cancellation intent");
    assert!(answer(&fx, child, seq, None).is_err());
    assert!(fx.store.inbox(child, 0).expect("inbox").is_empty());
}

#[tokio::test]
async fn successful_retry_precedes_staleness_and_survives_runtime_restart() {
    let fx = fixture().await;
    let agent = an_agent(&fx).await;
    let seq = ask(&fx, agent);
    let first = answer(&fx, agent, seq, Some("reply-1")).expect("first");
    ask(&fx, agent);
    assert_eq!(
        answer(&fx, agent, seq, Some("reply-1")).expect("newer question retry"),
        first
    );
    fx.runtime
        .cancel_agent(agent, None, Actor::user())
        .expect("cancel");
    fx.runtime
        .archive_agents(ArchiveTarget::Agent(agent), true, Actor::user())
        .expect("archive");
    fx.runtime.shutdown().await;
    let reopened = over(fx.store.clone()).await;
    assert_eq!(
        answer(&reopened, agent, seq, Some("reply-1")).expect("durable retry"),
        first
    );
    assert_eq!(fx.store.inbox(agent, 0).expect("inbox").len(), 1);
}

#[tokio::test]
async fn a_key_cannot_change_content_agent_or_question_and_agents_cannot_reply() {
    let fx = fixture().await;
    let agent = an_agent(&fx).await;
    let other = an_agent(&fx).await;
    let seq = ask(&fx, agent);
    answer(&fx, agent, seq, Some("reply-1")).expect("first");
    for (to, question, content) in [
        (agent, seq, "no"),
        (other, seq, "yes"),
        (agent, seq + 1, "yes"),
    ] {
        let result = fx.runtime.reply_to_question(
            to,
            question,
            text(content),
            Actor::user(),
            Some(submission("reply-1", to, question, content)),
        );
        assert!(
            result
                .expect_err("conflict")
                .to_string()
                .contains("different request")
        );
    }
    for actor in [Actor::agent(agent), Actor::System, Actor::Scheduler] {
        assert!(
            fx.runtime
                .reply_to_question(
                    agent,
                    seq,
                    text("yes"),
                    actor,
                    Some(submission("reply-1", agent, seq, "yes"))
                )
                .expect_err("operator only, including retries")
                .to_string()
                .contains("only an operator")
        );
    }
}

#[tokio::test]
async fn concurrent_distinct_replies_have_one_winner_and_same_key_retries_all_succeed() {
    let fx = fixture().await;
    let agent = an_agent(&fx).await;
    for keyed in [false, true] {
        let seq = ask(&fx, agent);
        let barrier = Arc::new(std::sync::Barrier::new(8));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let rt = fx.runtime.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    rt.reply_to_question(
                        agent,
                        seq,
                        text("yes"),
                        Actor::user(),
                        keyed.then(|| submission("concurrent", agent, seq, "yes")),
                    )
                })
            })
            .collect();
        let successes: Vec<_> = threads
            .into_iter()
            .filter_map(|thread| thread.join().expect("thread").ok())
            .collect();
        assert_eq!(successes.len(), if keyed { 8 } else { 1 });
        assert!(successes.iter().all(|message| *message == successes[0]));
    }
    assert_eq!(fx.store.inbox(agent, 0).expect("inbox").len(), 2);
}

#[tokio::test]
async fn recording_failure_delivers_nothing_and_leaves_question_open() {
    let log = CommitLog::open(Refusal::Records);
    let fx = over(log).await;
    let agent = an_agent(&fx).await;
    let seq = ask(&fx, agent);
    assert!(answer(&fx, agent, seq, Some("reply-1")).is_err());
    assert!(fx.store.inbox(agent, 0).expect("inbox").is_empty());
    assert_eq!(fx.store.last_seq().expect("seq"), seq);
    assert!(
        fx.store
            .submission(&submission("reply-1", agent, seq, "yes").key)
            .expect("record")
            .is_none()
    );
    answer(&fx, agent, seq, None).expect("failed commit left question unanswered");
}

#[tokio::test]
async fn question_reply_and_submission_are_one_commit() {
    let (fx, log) = logged().await;
    let agent = an_agent(&fx).await;
    let seq = ask(&fx, agent);
    log.take();
    let message = answer(&fx, agent, seq, Some("atomic-reply")).expect("answer");
    assert_committed_together(&log.take(), Work::Delivery(message), "reply_to_question");
}
