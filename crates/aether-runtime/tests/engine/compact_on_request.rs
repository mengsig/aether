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
