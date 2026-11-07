//! End-to-end subscription checks with an engine paused between chunks.

use aether_protocol::{ClientFrame, Delta, ServerFrame};
use futures::StreamExt;
use tokio::io::{AsyncWriteExt, DuplexStream, ReadHalf, WriteHalf};
use tokio::sync::Semaphore;
use tokio_util::codec::{FramedRead, LinesCodec};

use super::*;

struct ChunkedCli {
    chunks: Vec<&'static str>,
    advance: Arc<Semaphore>,
    abort: CancellationToken,
    finished: Arc<std::sync::atomic::AtomicBool>,
}

#[async_trait::async_trait]
impl aether_engine::Engine for ChunkedCli {
    fn kind(&self) -> EngineKind {
        EngineKind::CLAUDE_CODE
    }
    fn adapter(&self) -> String {
        "gated-noesis-stream".into()
    }

    async fn run(
        &self,
        _turn: aether_engine::EngineTurn,
        cancel: CancellationToken,
    ) -> Result<EngineRun, CliError> {
        let (tx, events) = mpsc::channel(4);
        let chunks = self.chunks.clone();
        let advance = self.advance.clone();
        let abort = self.abort.clone();
        let finished = self.finished.clone();
        tokio::spawn(async move {
            let mut text = String::new();
            for chunk in &chunks {
                if !advance_chunk(&advance, &cancel, &abort).await {
                    return;
                }
                text.push_str(chunk);
                let event = EngineEvent::Delta(StreamEvent::TextDelta {
                    index: 0,
                    text: (*chunk).into(),
                });
                if tx.send(event).await.is_err() {
                    return;
                }
            }
            if !advance_chunk(&advance, &cancel, &abort).await {
                return;
            }
            finished.store(true, Ordering::SeqCst);
            let receipt = EngineEvent::Done(EngineOutcome {
                text: Some(text),
                closing_accounting: true,
                cost_usd: Some(0.001),
                usage: Usage {
                    input_tokens: 40,
                    output_tokens: 20,
                    ..Default::default()
                },
                ..Default::default()
            });
            if tx.send(receipt).await.is_err() {
                cancel.cancel();
            }
        });
        Ok(EngineRun {
            events,
            input: None,
            decisions: None,
            stop: None,
        })
    }
}

async fn advance_chunk(
    advance: &Semaphore,
    cancel: &CancellationToken,
    abort: &CancellationToken,
) -> bool {
    tokio::select! {
        permit = advance.acquire() => { permit.expect("gate open").forget(); true }
        () = cancel.cancelled() => false,
        () = abort.cancelled() => false,
    }
}

struct Subscription {
    frames: FramedRead<ReadHalf<DuplexStream>, LinesCodec>,
    write: WriteHalf<DuplexStream>,
    stop: CancellationToken,
    serving: tokio::task::JoinHandle<anyhow::Result<aether_remote::Ended>>,
}

impl Subscription {
    async fn open(daemon: Arc<Daemon>, agent: AgentId) -> Self {
        let stop = CancellationToken::new();
        let (client, server) = tokio::io::duplex(16 * 1024);
        let serving = tokio::spawn(crate::connection::serve(
            daemon,
            server,
            crate::ConnectionPolicy::default(),
            None,
            None,
            stop.clone(),
        ));
        let (read, write) = tokio::io::split(client);
        let mut reader = Self {
            frames: FramedRead::new(read, LinesCodec::new()),
            write,
            stop,
            serving,
        };
        reader
            .send(
                1,
                Command::Hello(aether_protocol::Hello::new("noesis-stream-test", "test")),
            )
            .await;
        reader.acknowledged(1).await;
        let command = serde_json::from_value(
            json!({"cmd": "subscribe", "deltas": true, "query": {"agent": agent}}),
        )
        .expect("subscribe");
        reader.send(2, command).await;
        reader.acknowledged(2).await;
        reader
    }

    async fn acknowledged(&mut self, expected: u64) {
        loop {
            match self.next().await {
                ServerFrame::Reply { id, result } => {
                    assert_eq!(id, expected);
                    assert!(result.is_ok(), "{result:?}");
                    return;
                }
                ServerFrame::Event { .. }
                | ServerFrame::Delta {
                    delta: Delta::TurnStarted { .. },
                    ..
                } => {}
                other => panic!("unexpected frame before releasing the model: {other:?}"),
            }
        }
    }

    async fn send(&mut self, id: u64, command: Command) {
        let line = aether_protocol::encode(&ClientFrame { id, command }).expect("encode");
        self.write.write_all(line.as_bytes()).await.expect("write");
    }

    async fn next(&mut self) -> ServerFrame {
        let line = tokio::time::timeout(Duration::from_secs(5), self.frames.next())
            .await
            .expect("frame before timeout")
            .expect("connected")
            .expect("line");
        aether_protocol::decode(&line).expect("frame")
    }

    async fn text(&mut self, agent: AgentId) -> String {
        loop {
            if let ServerFrame::Delta {
                agent: seen, delta, ..
            } = self.next().await
            {
                assert_eq!(seen, agent);
                match delta {
                    Delta::Text { text } => return text,
                    Delta::TurnStarted { .. } => {}
                    other => panic!("unexpected delta before text: {other:?}"),
                }
            }
        }
    }

    async fn finish(&mut self, agent: AgentId, private: &[&str]) {
        loop {
            let frame = self.next().await;
            let wire = serde_json::to_string(&frame).expect("wire");
            for secret in private {
                assert!(!wire.contains(secret), "private data escaped: {wire}");
            }
            if let ServerFrame::Delta {
                agent: seen, delta, ..
            } = frame
            {
                assert_eq!(seen, agent);
                assert!(
                    matches!(delta, Delta::TurnFinished),
                    "text must not replay at EOF: {delta:?}"
                );
                return;
            }
        }
    }

    async fn close(self) {
        self.stop.cancel();
        self.serving
            .await
            .expect("connection task")
            .expect("connection exit");
    }
}

async fn running(chunks: Vec<&'static str>) -> (Home, Arc<Daemon>, Arc<ChunkedCli>, AgentId) {
    let home = Home::new();
    std::fs::write(home.config.path().join("providers.toml"), PROVIDERS).expect("providers");
    let cli = Arc::new(ChunkedCli {
        chunks,
        advance: Arc::new(Semaphore::new(0)),
        abort: CancellationToken::new(),
        finished: Default::default(),
    });
    let mut engines: HashMap<EngineKind, Arc<dyn aether_engine::Engine>> = HashMap::new();
    engines.insert(EngineKind::CLAUDE_CODE, cli.clone());
    let harness = Harness::open_on_engines(&home, engines).await;
    harness
        .configure(NoesisSettingsPatch {
            enabled: Some(true),
            ..Default::default()
        })
        .await;
    let session = harness.create(json!({"goal": "Explore circuits"})).await;
    let agent = agent_of(&session);
    assert!(!cli.finished.load(Ordering::SeqCst));
    assert_eq!(cli.advance.available_permits(), 0);
    (home, Arc::new(harness.daemon), cli, agent)
}

async fn close(daemon: Arc<Daemon>, reader: Subscription) {
    reader.close().await;
    daemon.noesis.shutdown().await;
    daemon.runtime.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn noesis_subscription_delivers_each_text_chunk_before_the_engine_finishes() {
    let (_home, daemon, cli, agent) = running(vec![
        r#"{"text":"First "#,
        "second ",
        "third.",
        r#"","tool_calls":[]}"#,
    ])
    .await;
    let _abort_on_failure = cli.abort.clone().drop_guard();
    let mut reader = Subscription::open(daemon.clone(), agent).await;
    for expected in ["First ", "second ", "third."] {
        cli.advance.add_permits(1);
        assert_eq!(reader.text(agent).await, expected);
        assert!(
            !cli.finished.load(Ordering::SeqCst),
            "the subscriber sees text before Done"
        );
    }
    cli.advance.add_permits(2);
    reader.finish(agent, &[]).await;
    assert!(cli.finished.load(Ordering::SeqCst));
    close(daemon, reader).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn noesis_subscription_withholds_secrets_split_across_engine_chunks() {
    let chunks = vec![
        r#"{"text":"Quick "#,
        "check.",
        r#"","tool_calls":[{"name":"noesis_ask","input":{"kind":"recall","prompt":"Name a code","answer_key":"SECRET-"#,
        r#"ANSWER","explanation":"HIDDEN-"#,
        r#"EXPLANATION"}}]}"#,
    ];
    let (_home, daemon, cli, agent) = running(chunks).await;
    let _abort_on_failure = cli.abort.clone().drop_guard();
    let mut reader = Subscription::open(daemon.clone(), agent).await;
    for expected in ["Quick ", "check."] {
        cli.advance.add_permits(1);
        assert_eq!(reader.text(agent).await, expected);
        assert!(!cli.finished.load(Ordering::SeqCst));
    }
    cli.advance.add_permits(4);
    reader
        .finish(agent, &["SECRET-", "ANSWER", "HIDDEN-", "EXPLANATION"])
        .await;
    assert!(cli.finished.load(Ordering::SeqCst));
    close(daemon, reader).await;
}
