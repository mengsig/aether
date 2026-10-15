use crate::Daemon;
use aether_protocol::{Delta, ErrorReply, ServerFrame};
use aether_runtime::{BusMessage, DeltaKind};
use aether_store::EventFilter;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

fn delta_matches(
    store: &dyn aether_store::Store,
    filter: &EventFilter,
    agent: aether_core::id::AgentId,
    invocation: aether_core::id::InvocationId,
    control: &SubscriptionControl,
) -> bool {
    // Deltas have no event family/type. Those filters select durable events only.
    if !filter.families.is_empty() || !filter.types.is_empty() {
        return false;
    }
    if filter.agent.is_none() && filter.run.is_none() && filter.task.is_none() {
        return true;
    }
    let matched = || -> aether_store::Result<bool> {
        let Some(row) = store.agent(agent)? else {
            return Ok(false);
        };
        // A child's deltas are of no type, so no `child_types` list takes them.
        let children = !filter.exclude_children && filter.child_types.is_empty();
        if filter
            .agent
            .is_some_and(|id| id != agent && (!children || row.parent != Some(id)))
            || filter.run.is_some_and(|run| row.run != run)
        {
            return Ok(false);
        }
        if let Some(task) = filter.task {
            let Some(call) = store.invocation(invocation)? else {
                return Ok(false);
            };
            let Some(snapshot) = store.snapshot(call.snapshot_id)? else {
                return Ok(false);
            };
            return Ok(snapshot.agent_id == agent && snapshot.task_id == Some(task));
        }
        Ok(true)
    };
    match matched() {
        Ok(matched) => matched,
        Err(error) => {
            control.failed(error);
            false
        }
    }
}

pub(super) struct SubscriptionControl {
    pub protocol_version: u32,
    pub cancel: CancellationToken,
    pub failure: mpsc::Sender<ErrorReply>,
    pub auth: crate::authz::AuthContext,
}

impl SubscriptionControl {
    async fn send(&self, out: &mpsc::Sender<ServerFrame>, mut frame: ServerFrame) -> bool {
        if self.protocol_version < 20
            && matches!(
                frame,
                ServerFrame::Delta {
                    delta: Delta::Accounting { .. },
                    ..
                }
            )
        {
            return true;
        }
        self.auth.redact_frame(&mut frame);
        tokio::select! {
            biased;
            _ = self.revoked() => false,
            _ = self.cancel.cancelled() => false,
            sent = out.send(frame) => sent.is_ok(),
        }
    }

    async fn revoked(&self) {
        match self.auth.closed() {
            Some(closed) => closed.cancelled().await,
            None => std::future::pending().await,
        }
    }

    fn failed(&self, error: aether_store::StoreError) {
        tracing::warn!(%error, "subscription replay failed; closing its connection");
        match self.failure.try_send(super::store_err(error)) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                tracing::debug!("an earlier replay failure is already closing this connection");
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                tracing::debug!("subscription connection already closed");
            }
        }
    }
}

struct Subscription {
    store: Arc<dyn aether_store::Store>,
    out: mpsc::Sender<ServerFrame>,
    sub: u64,
    filter: EventFilter,
    deltas: bool,
    control: SubscriptionControl,
    last: u64,
    replayed_through: u64,
}

impl Subscription {
    async fn replay(&mut self) -> bool {
        loop {
            if self.control.cancel.is_cancelled()
                || self
                    .control
                    .auth
                    .closed()
                    .is_some_and(CancellationToken::is_cancelled)
            {
                return false;
            }
            let page = match self.store.events(self.last, 500, &self.filter) {
                Ok(page) => page,
                Err(error) => {
                    self.control.failed(error);
                    return false;
                }
            };
            if page.is_empty() {
                self.replayed_through = self.last;
                return true;
            }
            for event in page {
                debug_assert!(event.seq > self.last);
                debug_assert!(self.filter.matches(&event));
                self.last = event.seq;
                if !self
                    .control
                    .send(
                        &self.out,
                        ServerFrame::Event {
                            sub: self.sub,
                            event,
                        },
                    )
                    .await
                {
                    return false;
                }
            }
        }
    }

    async fn deliver(&mut self, message: BusMessage) -> bool {
        let frame = match message {
            BusMessage::Event(event) => {
                if event.seq <= self.last || !self.filter.matches(&event) {
                    return true;
                }
                self.last = event.seq;
                ServerFrame::Event {
                    sub: self.sub,
                    event: (*event).clone(),
                }
            }
            BusMessage::Delta {
                agent,
                invocation,
                delta,
            } => {
                if let DeltaKind::Accounting { after_seq, .. } = &delta
                    && *after_seq < self.replayed_through
                {
                    // Replay already includes a later durable reading; this queued
                    // snapshot must not replace it with older in-memory figures.
                    return true;
                }
                if !self.deltas
                    || !delta_matches(
                        self.store.as_ref(),
                        &self.filter,
                        agent,
                        invocation,
                        &self.control,
                    )
                {
                    return true;
                }
                let delta = match delta {
                    DeltaKind::TurnStarted { turn } => Delta::TurnStarted { turn },
                    DeltaKind::Text { text } => Delta::Text { text },
                    DeltaKind::Thinking { text } => Delta::Thinking { text },
                    DeltaKind::ToolInput { name, partial_json } => {
                        Delta::ToolInput { name, partial_json }
                    }
                    DeltaKind::Accounting {
                        total,
                        turn_usage,
                        context_tokens,
                        ..
                    } => Delta::Accounting {
                        total,
                        turn_usage,
                        context_tokens,
                    },
                    DeltaKind::TurnFinished => Delta::TurnFinished,
                };
                ServerFrame::Delta {
                    sub: self.sub,
                    agent,
                    invocation,
                    delta,
                }
            }
        };
        self.control.send(&self.out, frame).await
    }

    async fn run(mut self, mut rx: tokio::sync::broadcast::Receiver<BusMessage>) {
        if !self.replay().await {
            return;
        }
        loop {
            let message = tokio::select! {
                biased;
                _ = self.control.revoked() => return,
                _ = self.control.cancel.cancelled() => return,
                message = rx.recv() => message,
            };
            match message {
                Ok(message) => {
                    if !self.deliver(message).await {
                        return;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(lagged)) => {
                    tracing::warn!(
                        sub = self.sub,
                        lagged,
                        "subscription lagged; replaying from store"
                    );
                    if !self.replay().await {
                        return;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            }
        }
    }
}

/// Replay history before following the bus, with one cancellation and filtering boundary.
pub(super) fn spawn_subscription(
    daemon: Arc<Daemon>,
    out: mpsc::Sender<ServerFrame>,
    sub: u64,
    after: Option<u64>,
    filter: EventFilter,
    deltas: bool,
    control: SubscriptionControl,
) -> aether_store::Result<(u64, tokio::task::JoinHandle<()>)> {
    let store = daemon.runtime.store().clone();
    let rx = daemon.runtime.subscribe();
    let last = after.unwrap_or(store.last_seq()?);
    let task = tokio::spawn(
        Subscription {
            store,
            out,
            sub,
            filter,
            deltas,
            control,
            last,
            replayed_through: last,
        }
        .run(rx),
    );
    Ok((last, task))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn replayed_durable_events_supersede_queued_accounting() {
        let (daemon, _config) = crate::connection::tests::daemon().await;
        let (out, mut received) = mpsc::channel(4);
        let (failure, _failures) = mpsc::channel(1);
        let mut subscription = Subscription {
            store: daemon.runtime.store().clone(),
            out,
            sub: 1,
            filter: EventFilter::default(),
            deltas: true,
            control: SubscriptionControl {
                protocol_version: aether_protocol::PROTOCOL_VERSION,
                cancel: CancellationToken::new(),
                failure,
                auth: crate::authz::AuthContext::Local,
            },
            last: 20,
            replayed_through: 20,
        };
        let usage = aether_core::model::Usage {
            output_tokens: 5,
            ..Default::default()
        };
        for after_seq in [19, 20] {
            assert!(
                subscription
                    .deliver(BusMessage::Delta {
                        agent: aether_core::id::AgentId::new(),
                        invocation: aether_core::id::InvocationId::new(),
                        delta: DeltaKind::Accounting {
                            after_seq,
                            total: usage,
                            turn_usage: usage,
                            context_tokens: Some(15),
                        },
                    })
                    .await
            );
            if after_seq < 20 {
                assert!(
                    received.try_recv().is_err(),
                    "replay has already passed this reading"
                );
            } else {
                assert!(matches!(received.try_recv().unwrap(), ServerFrame::Delta {
                    delta: Delta::Accounting { total, context_tokens: Some(15), .. }, ..
                } if total == usage));
            }
        }
        assert_eq!(
            subscription.last, 20,
            "deltas never advance replay position"
        );
        daemon.runtime.shutdown().await;
    }

    #[tokio::test]
    async fn legacy_subscribers_keep_existing_frames_and_skip_live_accounting() {
        for protocol_version in 17..=aether_protocol::PROTOCOL_VERSION {
            let (out, mut received) = mpsc::channel(4);
            let (failure, _failures) = mpsc::channel(1);
            let control = SubscriptionControl {
                protocol_version,
                cancel: CancellationToken::new(),
                failure,
                auth: crate::authz::AuthContext::Local,
            };
            let agent = aether_core::id::AgentId::new();
            let invocation = aether_core::id::InvocationId::new();
            let usage = aether_core::model::Usage {
                input_tokens: 20,
                ..Default::default()
            };
            let accounting = ServerFrame::Delta {
                sub: 1,
                agent,
                invocation,
                delta: Delta::Accounting {
                    total: usage,
                    turn_usage: usage,
                    context_tokens: Some(300),
                },
            };
            assert!(control.send(&out, accounting.clone()).await);
            if protocol_version >= 20 {
                assert_eq!(received.try_recv().unwrap(), accounting);
            } else {
                assert!(received.try_recv().is_err(), "v{protocol_version}");
            }
            let text = ServerFrame::Delta {
                sub: 1,
                agent,
                invocation,
                delta: Delta::Text {
                    text: "working".into(),
                },
            };
            let event = aether_core::event::Event::from_draft(
                aether_core::event::EventDraft::new(
                    aether_core::event::Actor::System,
                    aether_core::event::EventKind::AgentUsageUpdated {
                        total: usage,
                        turn_usage: usage,
                    },
                )
                .agent(agent),
                aether_core::id::EventId::new(),
                1,
                aether_core::time::now(),
            );
            let durable = ServerFrame::Event { sub: 1, event };
            for frame in [text, durable] {
                assert!(control.send(&out, frame.clone()).await);
                assert_eq!(received.try_recv().unwrap(), frame, "v{protocol_version}");
            }
        }
    }

    #[tokio::test]
    async fn cancellation_interrupts_blocked_delivery() {
        let (out, mut received) = mpsc::channel(1);
        let frame = || ServerFrame::Reply {
            id: 0,
            result: Ok(aether_protocol::Reply::Ok),
        };
        out.send(frame()).await.unwrap();
        let (failure, _failures) = mpsc::channel(1);
        let control = SubscriptionControl {
            protocol_version: aether_protocol::PROTOCOL_VERSION,
            cancel: CancellationToken::new(),
            failure,
            auth: crate::authz::AuthContext::Local,
        };
        let pending = control.send(&out, frame());
        tokio::pin!(pending);
        assert!(futures::poll!(&mut pending).is_pending());
        control.cancel.cancel();
        assert!(!pending.await);
        assert!(received.try_recv().is_ok());
        assert!(received.try_recv().is_err());
    }

    #[tokio::test]
    async fn cancellation_wins_over_ready_delivery() {
        let (out, mut received) = mpsc::channel(1);
        let (failure, _failures) = mpsc::channel(1);
        let control = SubscriptionControl {
            protocol_version: aether_protocol::PROTOCOL_VERSION,
            cancel: CancellationToken::new(),
            failure,
            auth: crate::authz::AuthContext::Local,
        };
        control.cancel.cancel();
        assert!(
            !control
                .send(
                    &out,
                    ServerFrame::Reply {
                        id: 0,
                        result: Ok(aether_protocol::Reply::Ok)
                    }
                )
                .await
        );
        assert!(received.try_recv().is_err());
    }

    #[test]
    fn simultaneous_replay_failures_do_not_block_connection_cleanup() {
        let (failure, mut received) = mpsc::channel(1);
        let control = SubscriptionControl {
            protocol_version: aether_protocol::PROTOCOL_VERSION,
            cancel: CancellationToken::new(),
            failure,
            auth: crate::authz::AuthContext::Local,
        };
        for _ in 0..2 {
            control.failed(aether_store::StoreError::HistoryExpired {
                first_seq: 1,
                last_seq: 3,
            });
        }
        assert!(
            received
                .try_recv()
                .unwrap()
                .message
                .contains("history expired")
        );
        assert!(received.try_recv().is_err());
        drop(received);
        control.failed(aether_store::StoreError::HistoryExpired {
            first_seq: 1,
            last_seq: 3,
        });
    }
}

#[cfg(test)]
mod device_tests {
    use super::*;
    use aether_core::event::Actor;
    use aether_core::id::{DeviceId, InvocationId, RunId};
    use aether_protocol::remote::{DeviceIdentity, DeviceRole};
    use aether_remote::DeviceSession;

    fn control() -> SubscriptionControl {
        let (failure, _) = mpsc::channel(1);
        SubscriptionControl {
            protocol_version: aether_protocol::PROTOCOL_VERSION,
            cancel: CancellationToken::new(),
            failure,
            auth: crate::authz::AuthContext::Device(DeviceSession {
                identity: DeviceIdentity {
                    id: DeviceId::new(),
                    name: "phone".into(),
                    role: DeviceRole::Observer,
                },
                closed: CancellationToken::new(),
            }),
        }
    }

    #[tokio::test]
    async fn revocation_interrupts_blocked_delta_delivery() {
        let (out, mut received) = mpsc::channel(1);
        let control = control();
        let frame = || ServerFrame::Delta {
            sub: 1,
            agent: aether_core::id::AgentId::new(),
            invocation: InvocationId::new(),
            delta: Delta::Text {
                text: "private".into(),
            },
        };
        out.send(frame()).await.unwrap();
        let pending = control.send(&out, frame());
        tokio::pin!(pending);
        assert!(futures::poll!(&mut pending).is_pending());
        control.auth.closed().unwrap().cancel();
        assert!(!pending.await);
        assert!(received.try_recv().is_ok());
        assert!(received.try_recv().is_err());
    }

    /// A root agent and its child, neither started.
    fn root_and_child(
        daemon: &crate::Daemon,
    ) -> (aether_core::agent::Agent, aether_core::agent::Agent) {
        let create = |spec| daemon.runtime.create_agent(spec, Actor::user()).unwrap().0;
        let root = create(aether_runtime::CreateAgentSpec {
            role: "root".into(),
            start: false,
            ..Default::default()
        });
        let child = create(aether_runtime::CreateAgentSpec {
            role: "child".into(),
            parent: Some(root.id),
            start: false,
            config: Some(aether_core::agent::AgentConfig {
                effort: Some(aether_core::model::Effort::Low),
                ..Default::default()
            }),
            ..Default::default()
        });
        (root, child)
    }

    /// Filters and whether a delta from the root's child matches each.
    fn filter_cases(root: &aether_core::agent::Agent) -> Vec<(EventFilter, bool)> {
        let only = |filter: EventFilter| (filter, false);
        vec![
            (EventFilter::default(), true),
            (EventFilter::for_agent(root.id), true),
            only(EventFilter::own_events(root.id)),
            (EventFilter::for_run(root.run), true),
            only(EventFilter::for_run(RunId::new())),
            only(EventFilter {
                types: vec!["agent.ready".into()],
                ..Default::default()
            }),
            only(EventFilter {
                families: vec!["tool".into()],
                ..Default::default()
            }),
            only(EventFilter {
                task: Some(aether_core::id::TaskId::new()),
                ..Default::default()
            }),
            only(EventFilter {
                agent: Some(root.id),
                child_types: vec!["agent.completed".into()],
                ..Default::default()
            }),
        ]
    }

    #[tokio::test]
    async fn deltas_obey_run_agent_child_and_event_type_filters() {
        let (daemon, _config) = crate::connection::tests::daemon().await;
        let (root, child) = root_and_child(&daemon);
        assert_eq!(child.parent, Some(root.id));
        let control = control();
        for (filter, expected) in filter_cases(&root) {
            assert_eq!(
                delta_matches(
                    daemon.runtime.store().as_ref(),
                    &filter,
                    child.id,
                    InvocationId::new(),
                    &control
                ),
                expected,
                "{filter:?}"
            );
        }
        let narrowed = EventFilter {
            agent: Some(root.id),
            child_types: vec!["agent.completed".into()],
            ..Default::default()
        };
        assert!(
            delta_matches(
                daemon.runtime.store().as_ref(),
                &narrowed,
                root.id,
                InvocationId::new(),
                &control
            ),
            "the agent's own deltas still stream"
        );
        daemon.runtime.shutdown().await;
    }
}
