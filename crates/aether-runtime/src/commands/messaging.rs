use super::MessageDelivery;
use crate::submission::wrong_shape;
use crate::{CommitHold, Result, Runtime, RuntimeError};
use aether_core::agent::Agent;
use aether_core::event::{Actor, Event, EventDraft, EventKind};
use aether_core::id::{AgentId, InboxItemId, MessageId};
use aether_core::idempotency::{Submission, SubmissionOutcome};
use aether_core::inbox::{InboxItem, InboxKind};
use aether_core::model::ContentBlock;
use aether_core::time::now;
use aether_store::{Commit, EventFilter, Mutation, Store};

/// The question `agent` asked last, unless an operator has answered it since.
/// Delivery of the answer closes it, so two replies cannot both queue.
pub fn pending_question(store: &dyn Store, agent: AgentId) -> Result<Option<Event>> {
    let mut filter = EventFilter::own_events(agent);
    filter.types = vec!["question.asked".into()];
    let Some(asked) = store.latest_events(1, &filter)?.pop() else {
        return Ok(None);
    };
    if asked.actor.agent_id() != Some(agent)
        || !matches!(asked.kind, EventKind::QuestionAsked { .. })
    {
        return Ok(None);
    }
    filter.types = vec!["message.sent".into()];
    let mut after = asked.seq;
    loop {
        let events = store.events(after, 128, &filter)?;
        for event in &events {
            if matches!(&event.kind, EventKind::MessageSent { from, to, .. }
                if *to == agent && matches!(from, Actor::User { .. } | Actor::Client { .. }))
            {
                return Ok(None);
            }
            after = event.seq;
        }
        if events.len() < 128 {
            return Ok(Some(asked));
        }
    }
}

impl Runtime {
    /// Deliver a message to an agent's inbox. Waking is the scheduler's job.
    pub fn send_message(
        &self,
        to: AgentId,
        content: Vec<ContentBlock>,
        from: Actor,
    ) -> Result<MessageId> {
        self.send_message_keyed(to, content, from, None)
    }

    /// Delivery a client can retry: under `submission`'s key, a second
    /// identical request returns the first message instead of putting a
    /// second copy in the inbox.
    pub fn send_message_keyed(
        &self,
        to: AgentId,
        content: Vec<ContentBlock>,
        from: Actor,
        submission: Option<Submission>,
    ) -> Result<MessageId> {
        // Before the liveness read, keyed or not: a recipient ending in
        // between would otherwise get a delivery committed after its end.
        let hold = self.hold();
        // Before the liveness check, not after: an agent that has finished
        // since the first delivery must not turn a retry into a refusal.
        if let Some(outcome) = self.replayed_submission(&hold, submission.as_ref())? {
            let SubmissionOutcome::Message { message } = outcome else {
                return Err(wrong_shape(&outcome, "message"));
            };
            return Ok(message);
        }
        let agent = self
            .store
            .agent(to)?
            .ok_or_else(|| RuntimeError::NotFound(to.to_string()))?;
        if agent.status.is_terminal() {
            return Err(RuntimeError::Invalid(format!(
                "agent {to} is {}",
                agent.status.kind()
            )));
        }
        self.deliver_message_held(hold, &agent, content, from, submission)
    }

    /// Answer exactly this agent's latest pending question, never a newer one.
    /// Validation, delivery and optional retry record share one commit hold.
    pub fn reply_to_question(
        &self,
        to: AgentId,
        question_seq: u64,
        content: Vec<ContentBlock>,
        from: Actor,
        submission: Option<Submission>,
    ) -> Result<MessageId> {
        if !matches!(from, Actor::User { .. } | Actor::Client { .. }) {
            return Err(RuntimeError::Invalid(
                "only an operator can reply to a question".into(),
            ));
        }
        let hold = self.hold();
        if let Some(outcome) = self.replayed_submission(&hold, submission.as_ref())? {
            let SubmissionOutcome::Message { message } = outcome else {
                return Err(wrong_shape(&outcome, "message"));
            };
            return Ok(message);
        }
        let agent = self
            .store
            .agent(to)?
            .ok_or_else(|| RuntimeError::NotFound(to.to_string()))?;
        if agent.status.is_terminal() || agent.archived {
            let state = if agent.archived {
                "archived".into()
            } else {
                agent.status.kind().to_string()
            };
            return Err(RuntimeError::Invalid(format!(
                "agent {to} is {state}; its question is no longer pending"
            )));
        }
        crate::cancellation::check_parent(self, &hold, to)?;
        self.check_pending_question(&hold, to, question_seq)?;
        self.deliver_message_held(hold, &agent, content, from, submission)
    }

    fn check_pending_question(
        &self,
        _hold: &CommitHold<'_>,
        to: AgentId,
        question_seq: u64,
    ) -> Result<()> {
        match pending_question(self.store.as_ref(), to)? {
            Some(asked) if asked.seq == question_seq => Ok(()),
            _ => Err(RuntimeError::Invalid(format!(
                "question {question_seq} is not the pending question for agent {to}"
            ))),
        }
    }

    fn deliver_message_held(
        &self,
        hold: CommitHold<'_>,
        agent: &Agent,
        content: Vec<ContentBlock>,
        from: Actor,
        submission: Option<Submission>,
    ) -> Result<MessageId> {
        let at = now();
        let to = agent.id;
        let message = MessageId::new();
        let mut commit = Commit::new();
        commit.push_event(
            EventDraft::new(
                from.clone(),
                EventKind::MessageSent {
                    message,
                    from: from.clone(),
                    to,
                    content: content.clone(),
                },
            )
            .agent(to)
            .run(agent.run),
        );
        self.push_inbox(
            &hold,
            &mut commit,
            agent,
            InboxKind::Message {
                message_id: message,
                from: from.clone(),
                content,
            },
            from,
        )?;
        if let Some(submission) = &submission {
            commit.push_mutation(Mutation::RecordSubmission(
                submission.record(at, SubmissionOutcome::Message { message }),
            ));
        }
        self.apply_held(hold, commit)?;
        Ok(message)
    }

    /// VISION §3.5: fan one message out to every live agent in `root`'s
    /// subtree. Each agent gets its own message, its own inbox item and its
    /// own events, all in one commit: everybody hears it or nobody does.
    /// The walk visits each agent once, so nothing is delivered twice.
    pub fn fan_out_message(
        &self,
        root: AgentId,
        content: Vec<ContentBlock>,
        from: Actor,
    ) -> Result<Vec<MessageDelivery>> {
        self.fan_out_message_keyed(root, content, from, None)
    }

    /// Fan-out a client can retry: under `submission`'s key, a second
    /// identical request returns the first fan-out's deliveries and delivers
    /// nothing. The subtree it reached then is what the retry reports, even
    /// if the subtree has since grown.
    pub fn fan_out_message_keyed(
        &self,
        root: AgentId,
        content: Vec<ContentBlock>,
        from: Actor,
        submission: Option<Submission>,
    ) -> Result<Vec<MessageDelivery>> {
        let at = now();
        // One hold over every read and the commit they feed: a delivery
        // batch is all-or-nothing, so a seq claimed underneath it would drop
        // the whole message rather than one copy of it.
        let hold = self.hold();
        if let Some(outcome) = self.replayed_submission(&hold, submission.as_ref())? {
            let SubmissionOutcome::FannedOut { deliveries } = outcome else {
                return Err(wrong_shape(&outcome, "fanned-out message"));
            };
            return Ok(deliveries);
        }
        let agent = self
            .store
            .agent(root)?
            .ok_or_else(|| RuntimeError::NotFound(root.to_string()))?;
        if agent.status.is_terminal() {
            return Err(RuntimeError::Invalid(format!(
                "agent {root} is {}",
                agent.status.kind()
            )));
        }
        // Completed intermediates still connect live descendants to the root.
        let targets = self.live_targets(root, true, "message recipient")?;
        let mut commit = Commit::new();
        let mut deliveries = Vec::with_capacity(targets.len());
        for target in &targets {
            let id = target.id;
            let message = MessageId::new();
            commit.push_event(
                EventDraft::new(
                    from.clone(),
                    EventKind::MessageSent {
                        message,
                        from: from.clone(),
                        to: id,
                        content: content.clone(),
                    },
                )
                .agent(id)
                .run(target.run),
            );
            self.push_inbox(
                &hold,
                &mut commit,
                target,
                InboxKind::Message {
                    message_id: message,
                    from: from.clone(),
                    content: content.clone(),
                },
                from.clone(),
            )?;
            deliveries.push(MessageDelivery { agent: id, message });
        }
        if let Some(submission) = &submission {
            commit.push_mutation(Mutation::RecordSubmission(submission.record(
                at,
                SubmissionOutcome::FannedOut {
                    deliveries: deliveries.clone(),
                },
            )));
        }
        self.apply_held(hold, commit)?;
        Ok(deliveries)
    }

    /// Deliver an external trigger to an agent's inbox. The scheduler treats
    /// it like a person's message: a waiting agent wakes to see it.
    pub fn trigger_agent(
        &self,
        id: AgentId,
        source: String,
        payload: serde_json::Value,
        by: Actor,
    ) -> Result<()> {
        self.trigger_agent_keyed(id, source, payload, by, None)
    }

    /// A trigger a webhook relay can retry: under `submission`'s key, a second
    /// identical delivery is a no-op rather than a second inbox item.
    pub fn trigger_agent_keyed(
        &self,
        id: AgentId,
        source: String,
        payload: serde_json::Value,
        by: Actor,
        submission: Option<Submission>,
    ) -> Result<()> {
        let source = source.trim().to_owned();
        if source.is_empty() {
            return Err(RuntimeError::Invalid("trigger source is empty".into()));
        }
        // The source is printed in the inbox heading, where only the daemon
        // may place the operator marker.
        if source.contains(aether_context::render::OPERATOR_MARKER.trim()) {
            return Err(RuntimeError::Invalid(
                "trigger source may not contain the operator marker".into(),
            ));
        }
        let at = now();
        // Before the liveness read, as in `send_message_keyed`.
        let hold = self.hold();
        if let Some(outcome) = self.replayed_submission(&hold, submission.as_ref())? {
            let SubmissionOutcome::Trigger { .. } = outcome else {
                return Err(wrong_shape(&outcome, "trigger"));
            };
            return Ok(());
        }
        let agent = self
            .store
            .agent(id)?
            .ok_or_else(|| RuntimeError::NotFound(id.to_string()))?;
        if agent.status.is_terminal() {
            return Err(RuntimeError::Invalid(format!(
                "agent {id} is {}",
                agent.status.kind()
            )));
        }
        let mut commit = Commit::new();
        let item = self.push_inbox(
            &hold,
            &mut commit,
            &agent,
            InboxKind::ExternalTrigger {
                source,
                payload,
                operator: false,
            },
            by,
        )?;
        if let Some(submission) = &submission {
            commit.push_mutation(Mutation::RecordSubmission(
                submission.record(at, SubmissionOutcome::Trigger { item: item.id }),
            ));
        }
        self.apply_held(hold, commit)?;
        Ok(())
    }

    /// Append an inbox delivery for `agent` to `commit`. `hold` is the proof
    /// the commit lock is held: the seq is `MAX + 1` for that agent, and the
    /// store rejects the whole commit if anything claims it in between.
    pub(crate) fn push_inbox(
        &self,
        _hold: &CommitHold<'_>,
        commit: &mut Commit,
        agent: &Agent,
        kind: InboxKind,
        actor: Actor,
    ) -> Result<InboxItem> {
        let already = commit
            .mutations
            .iter()
            .filter(|m| matches!(m, Mutation::DeliverInbox(i) if i.agent_id == agent.id))
            .count() as u64;
        let seq = self.store.inbox_last_seq(agent.id)? + 1 + already;
        let item = InboxItem {
            id: InboxItemId::new(),
            agent_id: agent.id,
            seq,
            kind,
            delivered_at: now(),
        };
        commit.push_event(
            EventDraft::new(actor, EventKind::InboxDelivered { item: item.clone() })
                .agent(agent.id)
                .parent(agent.parent)
                .run(agent.run),
        );
        commit.push_mutation(Mutation::DeliverInbox(item.clone()));
        Ok(item)
    }
}
