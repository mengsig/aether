//! Secret values the daemon knows are masked verbatim in engine output.
use super::scripted::{LISTEN_TOKEN, MCP_TOKEN};
use super::*;

fn seeded(config: &mut RuntimeConfig) {
    config.redactor = aether_tools::Redactor::with_known_secrets([LISTEN_TOKEN, MCP_TOKEN])
        .expect("known secrets compile");
}

#[tokio::test]
async fn a_listen_token_and_an_mcp_token_echoed_by_a_tool_are_masked_in_the_transcript() {
    let engine = Arc::new(ScriptedEngine {
        reply: Reply::ToolRun,
        prompts: Arc::default(),
        costs: Arc::default(),
        systems: Arc::default(),
        tool_gate: None,
    });
    let h = harness_with_engine_config(engine, seeded).await;
    let id = chat_agent(&h);
    h.runtime
        .send_message(id, vec![ContentBlock::text("print the env")], Actor::user())
        .expect("send");
    wait_for(h.store.as_ref(), id, |a| {
        a.status.kind() == AgentStatusKind::Waiting && a.stats.turns >= 1
    })
    .await;

    let stored = format!("{:?}", h.store.transcript(id, 0).expect("transcript"));
    assert!(
        stored.contains("NOTES_TOKEN="),
        "the tool result is stored: {stored}"
    );
    for secret in [LISTEN_TOKEN, MCP_TOKEN] {
        assert!(!stored.contains(secret), "{secret} survived: {stored}");
    }
    assert_eq!(
        stored.matches("[REDACTED:known_secret]").count(),
        2,
        "{stored}"
    );
    let events = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(id))
        .expect("events");
    let events = format!("{events:?}");
    assert!(!events.contains(LISTEN_TOKEN) && !events.contains(MCP_TOKEN));
    h.runtime.shutdown().await;
}

/// Fails its turn with an error that quotes the listen token, as an engine
/// echoing a rejected credential from its stderr would.
struct LeakingFailureEngine;

#[async_trait::async_trait]
impl Engine for LeakingFailureEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::CLAUDE_CODE
    }
    fn adapter(&self) -> String {
        "leaking-failure/1".into()
    }
    async fn run(
        &self,
        _turn: EngineTurn,
        _cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        let (tx, rx) = mpsc::channel(1);
        tx.send(EngineEvent::Done(EngineOutcome {
            closing_accounting: true,
            is_error: true,
            error: Some(format!("fleet refused token {LISTEN_TOKEN}")),
            ..Default::default()
        }))
        .await
        .expect("fixture channel capacity");
        Ok(EngineRun {
            events: rx,
            input: None,
            decisions: None,
            stop: None,
        })
    }
}

#[tokio::test]
async fn a_listen_token_in_an_engine_error_is_masked_where_it_is_stored() {
    let h = harness_with_engine_config(Arc::new(LeakingFailureEngine), seeded).await;
    let id = chat_agent(&h);
    h.runtime
        .send_message(id, vec![ContentBlock::text("go")], Actor::user())
        .expect("send");
    let agent = wait_for(h.store.as_ref(), id, |a| {
        a.stats.turns >= 1 && a.status.kind() != AgentStatusKind::Running
    })
    .await;

    let events = h
        .store
        .events(0, 10_000, &EventFilter::for_agent(id))
        .expect("events");
    let stored = format!("{events:?} {agent:?}");
    assert!(
        stored.contains("fleet refused token"),
        "the error is stored: {stored}"
    );
    assert!(
        !stored.contains(LISTEN_TOKEN),
        "the token survived: {stored}"
    );
    assert!(stored.contains("[REDACTED:known_secret]"), "{stored}");
    h.runtime.shutdown().await;
}
