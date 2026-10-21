//! A requested compaction on an engine that folds its own session runs that
//! engine's compaction in place instead of Aether's checkpoint compactor.
use super::*;

struct FoldingEngine {
    turns: Arc<Mutex<Vec<EngineTurn>>>,
    folds: bool,
}

impl FoldingEngine {
    fn events(turn: &EngineTurn, folds: bool) -> Vec<EngineEvent> {
        let session = turn.session.clone().unwrap_or(turn.new_session_id.clone());
        let mut events = vec![EngineEvent::Session {
            id: session.clone(),
            model: None,
            tools: vec![],
        }];
        if !turn.compact_session {
            events.push(EngineEvent::Assistant {
                content: vec![ContentBlock::text("noted")],
                usage: None,
                model: None,
            });
        } else if folds {
            events.push(EngineEvent::Compacted {
                automatic: false,
                before_tokens: Some(6164),
                after_tokens: Some(932),
                duration_ms: Some(6214),
            });
        } else {
            events.push(EngineEvent::CompactionFailed {
                error: "Not enough messages to compact.".into(),
            });
        }
        let cost = if turn.compact_session { 0.75 } else { 0.25 };
        let usage = aether_core::model::Usage {
            input_tokens: 10,
            output_tokens: 3,
            ..Default::default()
        };
        events.push(EngineEvent::SessionUsage {
            session,
            total: usage,
            models: None,
        });
        events.push(EngineEvent::Done(EngineOutcome {
            closing_accounting: true,
            text: (!turn.compact_session).then(|| "noted".into()),
            usage,
            cost_usd: Some(cost),
            ..Default::default()
        }));
        events
    }
}

#[async_trait::async_trait]
impl Engine for FoldingEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::CLAUDE_CODE
    }
    fn adapter(&self) -> String {
        "folding-test/1".into()
    }
    fn compacts_natively(&self) -> bool {
        true
    }
    async fn run(
        &self,
        turn: EngineTurn,
        _cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        self.turns.lock().unwrap().push(turn.clone());
        let (tx, rx) = mpsc::channel(8);
        for event in Self::events(&turn, self.folds) {
            tx.send(event).await.unwrap();
        }
        Ok(EngineRun {
            events: rx,
            input: None,
            decisions: None,
            stop: None,
        })
    }
}

#[tokio::test]
async fn a_requested_compaction_runs_the_engine_s_own_fold_and_keeps_the_session() {
    for folds in [true, false] {
        let turns = Arc::new(Mutex::new(Vec::new()));
        let engine = Arc::new(FoldingEngine {
            turns: turns.clone(),
            folds,
        });
        let h = harness_with_engine(engine, Arc::default(), Arc::default()).await;
        let id = chat_agent(&h);
        wait_for(h.store.as_ref(), id, |a| {
            a.status.kind() == AgentStatusKind::Waiting
        })
        .await;
        h.runtime
            .send_message(
                id,
                vec![ContentBlock::text("remember saffron")],
                Actor::user(),
            )
            .unwrap();
        let before = wait_for(h.store.as_ref(), id, |a| {
            a.stats.turns >= 1 && a.status.kind() == AgentStatusKind::Waiting
        })
        .await;
        assert!(before.engine_session.is_some());
        h.runtime.request_compaction(id, Actor::user()).unwrap();
        let settled = |kind: &EventKind| {
            matches!(
                kind,
                EventKind::CompactionNative { .. } | EventKind::CompactionFailed { .. }
            )
        };
        let events = loop {
            let events = h
                .store
                .events(0, 1000, &EventFilter::for_agent(id))
                .unwrap();
            if events.iter().any(|e| settled(&e.kind)) {
                break events;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        let after = wait_for(h.store.as_ref(), id, |a| {
            a.status.kind() == AgentStatusKind::Waiting
        })
        .await;

        let recorded = turns.lock().unwrap().clone();
        assert_eq!(
            recorded.len(),
            2,
            "one reply turn and one fold, no reply after it"
        );
        let fold = &recorded[1];
        assert!(fold.compact_session && !fold.summary_only);
        assert_eq!(fold.session, before.engine_session);
        assert!(fold.mcp.is_none() && !fold.bypass_permissions);
        assert_eq!(
            after.engine_session, before.engine_session,
            "the session is kept"
        );
        assert!(
            after.checkpoint.is_none(),
            "no Aether checkpoint is written"
        );
        assert!(
            (after.stats.usage.cost_usd - 1.0).abs() < 1e-9,
            "the fold is billed once on top of the reply"
        );
        let native = events.iter().find_map(|e| match &e.kind {
            EventKind::CompactionNative {
                provider,
                automatic,
                before_tokens,
                after_tokens,
                duration_ms,
            } => Some((
                provider.clone(),
                *automatic,
                *before_tokens,
                *after_tokens,
                *duration_ms,
            )),
            _ => None,
        });
        let failure = events.iter().find_map(|e| match &e.kind {
            EventKind::CompactionFailed { error, .. } => Some(error.clone()),
            _ => None,
        });
        let requested = events
            .iter()
            .find(|e| matches!(e.kind, EventKind::CompactionRequested { .. }))
            .expect("compaction.requested");
        let outcome = events.iter().find(|e| settled(&e.kind)).unwrap();
        assert_eq!(
            outcome.causation,
            Some(requested.id),
            "the outcome names the request it settles"
        );
        if folds {
            let (provider, automatic, before_tokens, after_tokens, duration_ms) =
                native.expect("compaction.native");
            assert!(!provider.is_empty() && !automatic);
            assert_eq!((before_tokens, after_tokens), (Some(6164), Some(932)));
            assert_eq!(duration_ms, Some(6214));
            assert_eq!(after.stats.context_tokens, Some(932));
            assert!(failure.is_none());
        } else {
            assert!(native.is_none());
            let error = failure.expect("compaction.failed");
            assert!(error.contains("could not compact") && error.contains("Not enough messages"));
        }
    }
}

/// Folds or fails on its own in the middle of a reply turn, once let go.
struct MidTurnFoldEngine {
    turns: Arc<Mutex<Vec<EngineTurn>>>,
    gate: Arc<tokio::sync::Notify>,
    mid_turn: EngineEvent,
}

#[async_trait::async_trait]
impl Engine for MidTurnFoldEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::CLAUDE_CODE
    }
    fn adapter(&self) -> String {
        "mid-turn-fold-test/1".into()
    }
    fn compacts_natively(&self) -> bool {
        true
    }
    async fn run(
        &self,
        turn: EngineTurn,
        _cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        self.turns.lock().unwrap().push(turn.clone());
        let mut events = FoldingEngine::events(&turn, true);
        let gate = (!turn.compact_session).then(|| self.gate.clone());
        if gate.is_some() {
            events.insert(1, self.mid_turn.clone());
        }
        let (tx, rx) = mpsc::channel(8);
        tokio::spawn(async move {
            for (at, event) in events.into_iter().enumerate() {
                if at == 1
                    && let Some(gate) = &gate
                {
                    gate.notified().await;
                }
                if tx.send(event).await.is_err() {
                    return;
                }
            }
        });
        Ok(EngineRun {
            events: rx,
            input: None,
            decisions: None,
            stop: None,
        })
    }
}

/// An engine's own fold mid-turn once settled the request queued behind it,
/// so the person's /compact (or a handoff's checkpoint) never ran.
#[tokio::test]
async fn a_fold_the_engine_makes_mid_turn_leaves_a_queued_request_owed() {
    let mid_turn_events = [
        EngineEvent::Compacted {
            automatic: true,
            before_tokens: Some(9000),
            after_tokens: Some(2000),
            duration_ms: Some(1000),
        },
        EngineEvent::CompactionFailed {
            error: "Conversation too long.".into(),
        },
    ];
    for mid_turn in mid_turn_events {
        let turns = Arc::new(Mutex::new(Vec::new()));
        let gate = Arc::new(tokio::sync::Notify::new());
        let engine = Arc::new(MidTurnFoldEngine {
            turns: turns.clone(),
            gate: gate.clone(),
            mid_turn,
        });
        let h = harness_with_engine(engine, Arc::default(), Arc::default()).await;
        let id = chat_agent(&h);
        wait_for(h.store.as_ref(), id, |a| {
            a.status.kind() == AgentStatusKind::Waiting
        })
        .await;
        h.runtime
            .send_message(id, vec![ContentBlock::text("go on")], Actor::user())
            .unwrap();
        wait_for(h.store.as_ref(), id, |a| {
            a.status.kind() == AgentStatusKind::Running
        })
        .await;
        h.runtime.request_compaction(id, Actor::user()).unwrap();
        gate.notify_one();
        let requested_fold = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let events = h
                    .store
                    .events(0, 1000, &EventFilter::for_agent(id))
                    .unwrap();
                let fold = events.iter().find(|e| {
                    matches!(
                        e.kind,
                        EventKind::CompactionNative {
                            automatic: false,
                            ..
                        }
                    )
                });
                if let Some(fold) = fold {
                    break fold.clone();
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the queued /compact runs once the turn ends");
        let recorded = turns.lock().unwrap().clone();
        assert_eq!(recorded.len(), 2, "the reply turn, then the requested fold");
        assert!(recorded[1].compact_session);
        assert_eq!(requested_fold.agent_id, Some(id));
        let events = h
            .store
            .events(0, 1000, &EventFilter::for_agent(id))
            .unwrap();
        let requests: Vec<_> = events
            .iter()
            .filter(|e| matches!(e.kind, EventKind::CompactionRequested { .. }))
            .collect();
        assert_eq!(requests.len(), 2, "the request, then its re-issue");
        assert_eq!(requests[1].causation, Some(requests[0].id));
        assert_eq!(
            requested_fold.causation,
            Some(requests[1].id),
            "the fold settles the re-issued request"
        );
    }
}
