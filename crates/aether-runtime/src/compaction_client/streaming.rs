//! Forward isolated engine text while retaining final receipt accounting.

use aether_core::model::{BlockStart, ModelError, ModelErrorClass, ModelRequest, StreamEvent};
use aether_engine::{EngineEvent, EngineRun, EngineTurn};
use aether_model::{ModelClient, ModelStream};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{
    Accounting, CompactionClient, checkpoint_result, checkpoint_turn, failure,
    refuse_checkpoint_input,
};

type Sender = mpsc::Sender<Result<StreamEvent, ModelError>>;

const IDLE_TIMEOUT: Duration = Duration::from_secs(90);
const STALLED: &str = "Noesis engine made no progress";

pub(crate) struct StreamingClient(pub(crate) CompactionClient);

#[async_trait::async_trait]
impl ModelClient for StreamingClient {
    fn adapter(&self) -> &str {
        "isolated-engine-stream/1"
    }

    async fn stream(
        &self,
        request: ModelRequest,
        cancel: CancellationToken,
    ) -> Result<ModelStream, ModelError> {
        debug_assert!(!self.0.purpose.is_empty());
        debug_assert!(self.0.turn.mcp.is_none());
        Ok(open(self.0.clone(), request, cancel))
    }
}

fn open(client: CompactionClient, request: ModelRequest, cancel: CancellationToken) -> ModelStream {
    debug_assert!(!client.timeout.is_zero());
    debug_assert!(client.turn.session.is_none());
    let child = cancel.child_token();
    let guard = child.clone().drop_guard();
    let (sender, receiver) = mpsc::channel(16);
    tokio::spawn(produce(client, request, child, sender));
    Box::pin(futures::stream::unfold(
        (receiver, guard),
        |(mut receiver, guard)| async move {
            receiver
                .recv()
                .await
                .map(|event| (event, (receiver, guard)))
        },
    ))
}

async fn produce(
    client: CompactionClient,
    request: ModelRequest,
    cancel: CancellationToken,
    sender: Sender,
) {
    let _cleanup = cancel.clone().drop_guard();
    let mut accounting = Accounting::priced(client.pricing);
    let mut output = TextOutput {
        sender,
        text: String::new(),
    };
    let turn = checkpoint_turn(client.turn.clone(), &request);
    let run = tokio::time::timeout(
        client.timeout,
        collect(&client, turn, &cancel, &mut accounting, &mut output),
    );
    let result = tokio::select! {
        result = run => result.unwrap_or_else(|_| Err(ModelError::new(ModelErrorClass::Timeout, format!("{} engine timed out", client.purpose)))),
        () = cancel.cancelled() => Err(ModelError::new(ModelErrorClass::Cancelled, format!("{} generation cancelled", client.purpose))),
    };
    accounting.finish(cancel.is_cancelled());
    debug_assert!(
        accounting.usage.is_some(),
        "every submitted launch settles usage"
    );
    if output
        .send(StreamEvent::Usage {
            usage: accounting.usage.unwrap_or_default(),
        })
        .await
        .is_err()
    {
        return;
    }
    let result = match result {
        Ok(text) => output.finish(&text).await,
        Err(error) => Err(error),
    };
    // A closed receiver has abandoned this invocation and its error.
    if let Err(error) = result
        && output.sender.send(Err(error)).await.is_err()
    {
        cancel.cancel();
    }
}

async fn collect(
    client: &CompactionClient,
    turn: EngineTurn,
    cancel: &CancellationToken,
    accounting: &mut Accounting,
    output: &mut TextOutput,
) -> Result<String, ModelError> {
    debug_assert!(turn.session.is_none());
    debug_assert!(accounting.usage.is_none());
    let engine_cancel = cancel.child_token();
    let _cleanup = engine_cancel.clone().drop_guard();
    let mut run = tokio::time::timeout(
        IDLE_TIMEOUT,
        client
            .engine
            .run_with_context(turn, client.context.clone(), engine_cancel.clone()),
    )
    .await
    .map_err(|_| stalled(&engine_cancel))?
    .map_err(|error| {
        accounting.unsubmitted = error.proves_no_submission();
        failure(error.to_string())
    })?;
    let mut final_text = String::new();
    let mut deadline = tokio::time::Instant::now() + IDLE_TIMEOUT;
    loop {
        let event = match tokio::time::timeout_at(deadline, run.events.recv()).await {
            Ok(Some(event)) => event,
            Ok(None) => break,
            Err(_) => {
                engine_cancel.cancel();
                drain_cancelled(&mut run, accounting).await;
                return Err(stalled(&engine_cancel));
            }
        };
        if is_progress(&event) {
            deadline = tokio::time::Instant::now() + IDLE_TIMEOUT;
        }
        if let Some(text) =
            observe(event, &run, client, accounting, output, &mut final_text).await?
        {
            return Ok(text);
        }
    }
    Err(failure(format!(
        "{} engine ended without a result",
        client.purpose
    )))
}

fn is_progress(event: &EngineEvent) -> bool {
    match event {
        EngineEvent::Delta(event) => stream_progress(event),
        EngineEvent::Assistant { content, .. } => !content.is_empty(),
        EngineEvent::UsageDelta { usage } => usage.total_tokens() > 0,
        EngineEvent::UsageRequest { request } => request.delta().total_tokens() > 0,
        EngineEvent::ToolResult { .. }
        | EngineEvent::Permission { .. }
        | EngineEvent::Question { .. }
        | EngineEvent::Done(_) => true,
        _ => false,
    }
}

fn stream_progress(event: &StreamEvent) -> bool {
    match event {
        StreamEvent::TextDelta { text, .. } | StreamEvent::ThinkingDelta { text, .. } => {
            !text.is_empty()
        }
        StreamEvent::ToolInputDelta { partial_json, .. } => !partial_json.is_empty(),
        StreamEvent::CompactionDelta { content, .. } => !content.is_empty(),
        StreamEvent::BlockStart {
            block: BlockStart::Text { text } | BlockStart::Thinking { text, .. },
            ..
        } => !text.is_empty(),
        StreamEvent::BlockStart {
            block: BlockStart::ToolUse { .. } | BlockStart::Compaction { .. },
            ..
        } => true,
        _ => false,
    }
}

async fn drain_cancelled(run: &mut EngineRun, accounting: &mut Accounting) {
    let drained = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(event) = run.events.recv().await {
            match event {
                EngineEvent::Assistant {
                    usage: Some(usage), ..
                } => accounting.reported(usage),
                EngineEvent::UsageDelta { usage } => accounting.reported(usage),
                EngineEvent::UsageRequest { request } => accounting.reported(request.delta()),
                EngineEvent::Done(outcome) => accounting.close(&outcome),
                _ => {}
            }
        }
    })
    .await;
    accounting.finish(true);
    if drained.is_err() {
        tracing::warn!("a cancelled Noesis engine did not finish cleanup in time");
    }
}

fn stalled(cancel: &CancellationToken) -> ModelError {
    cancel.cancel();
    ModelError::new(ModelErrorClass::Timeout, STALLED)
}

async fn observe(
    event: EngineEvent,
    run: &EngineRun,
    client: &CompactionClient,
    accounting: &mut Accounting,
    output: &mut TextOutput,
    final_text: &mut String,
) -> Result<Option<String>, ModelError> {
    debug_assert!(
        !accounting.unsubmitted,
        "an engine event follows a submitted launch"
    );
    debug_assert!(!client.purpose.is_empty());
    match event {
        EngineEvent::Delta(StreamEvent::TextDelta { text, .. }) => output.append(&text).await?,
        EngineEvent::Delta(StreamEvent::BlockStart {
            block: BlockStart::Text { text },
            ..
        }) => output.append(&text).await?,
        EngineEvent::Assistant { content, usage, .. } => {
            let text = aether_core::model::text_of(&content);
            if !text.trim().is_empty() {
                *final_text = text;
            }
            if let Some(usage) = usage {
                accounting.reported(usage);
            }
        }
        EngineEvent::Session {
            model: Some(reported),
            ..
        } => accounting.repriced(
            client
                .session_pricing
                .as_ref()
                .and_then(|price| price(&reported)),
        ),
        EngineEvent::UsageDelta { usage } => accounting.reported(usage),
        EngineEvent::UsageRequest { request } => accounting.reported(request.delta()),
        EngineEvent::Permission { request_id, .. } | EngineEvent::Question { request_id, .. } => {
            refuse_checkpoint_input(run, request_id).await?
        }
        EngineEvent::Done(outcome) => {
            accounting.close(&outcome);
            if final_text.is_empty() {
                final_text.clone_from(&output.text);
            }
            return Ok(Some(checkpoint_result(
                outcome,
                std::mem::take(final_text),
                client.purpose,
            )?));
        }
        _ => {}
    }
    Ok(None)
}

struct TextOutput {
    sender: Sender,
    text: String,
}

impl TextOutput {
    async fn send(&self, event: StreamEvent) -> Result<(), ModelError> {
        self.sender.send(Ok(event)).await.map_err(|_| {
            ModelError::new(ModelErrorClass::Cancelled, "isolated engine stream closed")
        })
    }

    async fn append(&mut self, text: &str) -> Result<(), ModelError> {
        if text.is_empty() {
            return Ok(());
        }
        if self.text.is_empty() {
            self.send(StreamEvent::MessageStart {
                provider_message_id: None,
                model: None,
            })
            .await?;
            self.send(StreamEvent::BlockStart {
                index: 0,
                block: BlockStart::Text {
                    text: String::new(),
                },
            })
            .await?;
        }
        let previous = self.text.len();
        self.send(StreamEvent::TextDelta {
            index: 0,
            text: text.into(),
        })
        .await?;
        self.text.push_str(text);
        debug_assert_eq!(self.text.len(), previous + text.len());
        debug_assert!(self.text.ends_with(text));
        Ok(())
    }

    async fn finish(&mut self, text: &str) -> Result<(), ModelError> {
        let Some(tail) = text.strip_prefix(&self.text) else {
            return Err(failure(
                "isolated engine final text differs from its streamed prefix",
            ));
        };
        debug_assert!(text.is_char_boundary(self.text.len()));
        self.append(tail).await?;
        debug_assert_eq!(
            self.text, text,
            "the receipt never replays a streamed prefix"
        );
        self.send(StreamEvent::BlockStop { index: 0 }).await?;
        self.send(StreamEvent::MessageStop).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_do_not_reset_the_silence_clock() {
        assert!(!is_progress(&EngineEvent::Note("heartbeat".into())));
        assert!(!is_progress(&EngineEvent::ContextUsage { tokens: 42 }));
        assert!(is_progress(&EngineEvent::Delta(StreamEvent::TextDelta {
            index: 0,
            text: "x".into()
        })));
    }

    #[tokio::test]
    async fn noesis_receipt_appends_only_the_unstreamed_suffix() {
        let (sender, mut events) = mpsc::channel(16);
        let mut output = TextOutput {
            sender,
            text: String::new(),
        };
        output.append("First ").await.unwrap();
        output.append("second").await.unwrap();
        output.finish("First second.").await.unwrap();
        drop(output);
        let mut texts = Vec::new();
        while let Some(event) = events.recv().await {
            if let StreamEvent::TextDelta { text, .. } = event.unwrap() {
                texts.push(text);
            }
        }
        assert_eq!(texts, ["First ", "second", "."]);
        assert_eq!(texts.concat(), "First second.");
    }

    #[tokio::test]
    async fn noesis_receipt_cannot_rewrite_a_public_prefix() {
        let (sender, mut events) = mpsc::channel(16);
        let mut output = TextOutput {
            sender,
            text: String::new(),
        };
        output.append("Earlier").await.unwrap();
        let error = output.finish("REPLACEMENT-PRIVATE").await.unwrap_err();
        assert!(!error.message.contains("PRIVATE"));
        drop(output);
        while let Some(event) = events.recv().await {
            assert!(!matches!(event, Ok(StreamEvent::MessageStop)));
        }
    }
}
