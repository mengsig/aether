use super::MessageDelivery;
use crate::attachments::{CheckedImage, message_content};
use crate::submission::wrong_shape;
use crate::{CommitHold, Result, Runtime, RuntimeError};
use aether_core::agent::Agent;
use aether_core::attachment::{AttachmentRef, ImageAttachment};
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

/// The message a recorded delivery created, as its retry is answered.
fn sent_message(outcome: SubmissionOutcome) -> Result<MessageId> {
    let SubmissionOutcome::Message { message } = outcome else {
        return Err(wrong_shape(&outcome, "message"));
    };
    Ok(message)
}

/// The deliveries a recorded fan-out created, as its retry is answered.
fn fanned_out(outcome: SubmissionOutcome) -> Result<Vec<MessageDelivery>> {
    let SubmissionOutcome::FannedOut { deliveries } = outcome else {
        return Err(wrong_shape(&outcome, "fanned-out message"));
    };
    Ok(deliveries)
}

/// The references a message carries for its checked images, in their order.
fn references(images: &[CheckedImage]) -> Vec<AttachmentRef> {
    images.iter().map(|image| image.reference.clone()).collect()
}

/// Whether `content` refers to each of `images`: a delivery stores no other.
fn refers_to_each(content: &[ContentBlock], images: &[CheckedImage]) -> bool {
    images.iter().all(|image| {
        content.iter().any(
            |block| matches!(block, ContentBlock::ImageRef(carried) if *carried == image.reference),
        )
    })
}

/// Refuses a message whose text the engine could not take in one record.
pub(crate) fn check_content_size(content: &[ContentBlock]) -> Result<()> {
    let bytes: usize = content
        .iter()
        .map(|block| match block {
            ContentBlock::Text { text } => text.len(),
            _ => 0,
        })
        .sum();
    check_text_size("message", bytes)
}

/// Refuses a JSON payload that renders into an engine record above the limit.
pub(crate) fn check_payload_size(what: &str, payload: &serde_json::Value) -> Result<()> {
    let bytes = serde_json::to_vec(payload)
        .map_err(|e| RuntimeError::Invalid(format!("{what} is not serializable: {e}")))?
        .len();
    check_text_size(what, bytes)
}

/// [`aether_core::limits::check_text_size`] as the runtime's refusal.
pub(crate) fn check_text_size(what: &str, bytes: usize) -> Result<()> {
    aether_core::limits::check_text_size(what, bytes).map_err(RuntimeError::Invalid)
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
        self.send_message_storing(to, content, &[], from, submission)
    }

    /// A client's message with its images, retried like any keyed delivery.
    /// A held receipt answers before an image is decoded, and a new delivery
    /// writes its images just before it commits: nothing else stores any.
    pub fn send_message_with_images(
        &self,
        to: AgentId,
        text: String,
        images: &[ImageAttachment],
        from: Actor,
        submission: Option<Submission>,
    ) -> Result<MessageId> {
        check_text_size("message", text.len())?;
        // Unheld, ahead of megabytes to decode; the held check is the guarantee.
        if let Some(outcome) = self.recorded_submission(submission.as_ref())? {
            return sent_message(outcome);
        }
        let images = self.check_attachments(images)?;
        let content = message_content(text, references(&images));
        self.send_message_storing(to, content, &images, from, submission)
    }

    fn send_message_storing(
        &self,
        to: AgentId,
        content: Vec<ContentBlock>,
        images: &[CheckedImage],
        from: Actor,
        submission: Option<Submission>,
    ) -> Result<MessageId> {
        debug_assert!(refers_to_each(&content, images), "an image nobody is sent");
        check_content_size(&content)?;
        // Before the liveness read, keyed or not: a recipient ending in
        // between would otherwise get a delivery committed after its end.
        let hold = self.hold();
        // Before the liveness check, not after: an agent that has finished
        // since the first delivery must not turn a retry into a refusal.
        if let Some(outcome) = self.replayed_submission(&hold, submission.as_ref())? {
            return sent_message(outcome);
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
        self.deliver_message_held(hold, &agent, content, images, from, submission)
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
        check_content_size(&content)?;
        let hold = self.hold();
        if let Some(outcome) = self.replayed_submission(&hold, submission.as_ref())? {
            return sent_message(outcome);
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
        self.deliver_message_held(hold, &agent, content, &[], from, submission)
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
        images: &[CheckedImage],
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
        self.store_images(&hold, images)?;
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
        self.fan_out_message_storing(root, content, &[], from, submission)
    }

    /// The fan-out of a client's message with its images: answered from a
    /// held receipt and stored as [`Self::send_message_with_images`] is,
    /// once for the whole subtree.
    pub fn fan_out_message_with_images(
        &self,
        root: AgentId,
        text: String,
        images: &[ImageAttachment],
        from: Actor,
        submission: Option<Submission>,
    ) -> Result<Vec<MessageDelivery>> {
        check_text_size("message", text.len())?;
        // Unheld, ahead of megabytes to decode; the held check is the guarantee.
        if let Some(outcome) = self.recorded_submission(submission.as_ref())? {
            return fanned_out(outcome);
        }
        let images = self.check_attachments(images)?;
        let content = message_content(text, references(&images));
        self.fan_out_message_storing(root, content, &images, from, submission)
    }

    fn fan_out_message_storing(
        &self,
        root: AgentId,
        content: Vec<ContentBlock>,
        images: &[CheckedImage],
        from: Actor,
        submission: Option<Submission>,
    ) -> Result<Vec<MessageDelivery>> {
        debug_assert!(refers_to_each(&content, images), "an image nobody is sent");
        check_content_size(&content)?;
        let at = now();
        // One hold over every read and the commit they feed: a delivery
        // batch is all-or-nothing, so a seq claimed underneath it would drop
        // the whole message rather than one copy of it.
        let hold = self.hold();
        if let Some(outcome) = self.replayed_submission(&hold, submission.as_ref())? {
            return fanned_out(outcome);
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
        let deliveries = self.push_fan_out(&hold, &mut commit, &targets, &content, &from)?;
        if let Some(submission) = &submission {
            commit.push_mutation(Mutation::RecordSubmission(submission.record(
                at,
                SubmissionOutcome::FannedOut {
                    deliveries: deliveries.clone(),
                },
            )));
        }
        self.store_images(&hold, images)?;
        self.apply_held(hold, commit)?;
        Ok(deliveries)
    }

    /// Give each target its own message, inbox item and event in `commit`.
    fn push_fan_out(
        &self,
        hold: &CommitHold<'_>,
        commit: &mut Commit,
        targets: &[Agent],
        content: &[ContentBlock],
        from: &Actor,
    ) -> Result<Vec<MessageDelivery>> {
        let mut deliveries = Vec::with_capacity(targets.len());
        for target in targets {
            let id = target.id;
            let message = MessageId::new();
            commit.push_event(
                EventDraft::new(
                    from.clone(),
                    EventKind::MessageSent {
                        message,
                        from: from.clone(),
                        to: id,
                        content: content.to_vec(),
                    },
                )
                .agent(id)
                .run(target.run),
            );
            self.push_inbox(
                hold,
                commit,
                target,
                InboxKind::Message {
                    message_id: message,
                    from: from.clone(),
                    content: content.to_vec(),
                },
                from.clone(),
            )?;
            deliveries.push(MessageDelivery { agent: id, message });
        }
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
        check_payload_size("trigger payload", &payload)?;
        check_text_size("trigger source", source.len())?;
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
