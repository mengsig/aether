//! Forward isolated engine text while retaining final receipt accounting.

use aether_core::model::{BlockStart, ModelError, ModelErrorClass, ModelRequest, StreamEvent};
use aether_engine::{EngineEvent, EngineRun, EngineTurn};
use aether_model::{ModelClient, ModelStream};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{
    Accounting, CompactionClient, checkpoint_result, checkpoint_turn, failure,
    refuse_checkpoint_input,
};

type Sender = mpsc::Sender<Result<StreamEvent, ModelError>>;

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
        result = run => result.unwrap_or_else(|_| Err(failure(format!("{} engine timed out", client.purpose)))),
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
    if let Err(error) = result {
        // A closed receiver has abandoned this invocation and its error.
        if output.sender.send(Err(error)).await.is_err() {
            cancel.cancel();
        }
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
    let mut run = client
        .engine
        .run_with_context(turn, client.context.clone(), cancel.clone())
        .await
        .map_err(|error| {
            accounting.unsubmitted = error.proves_no_submission();
            failure(error.to_string())
        })?;
    let mut final_text = String::new();
    while let Some(event) = run.events.recv().await {
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
