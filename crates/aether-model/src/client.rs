//! The one contract every provider adapter implements.

use std::pin::Pin;

use aether_core::model::{
    AccumulateError, Effort, ModelError, ModelErrorClass, ModelRequest, ModelResponse,
    StreamAccumulator, StreamEvent,
};
use futures::{Stream, StreamExt};
use tokio_util::sync::CancellationToken;

/// A stream of normalized events. An `Err` item ends the stream.
pub type ModelStream = Pin<Box<dyn Stream<Item = Result<StreamEvent, ModelError>> + Send>>;

#[async_trait::async_trait]
pub trait ModelClient: Send + Sync {
    /// Adapter name and version, e.g. `anthropic-messages/1`. Recorded in snapshots.
    fn adapter(&self) -> &str;

    /// The effort override this client will encode for the resolved model.
    fn effective_effort(
        &self,
        model: &crate::ResolvedModel,
        effort: Option<Effort>,
    ) -> Option<Effort> {
        model
            .provider
            .protocol
            .effective_effort(&model.model.capabilities, effort)
    }

    /// Send a request and return its event stream. Errors before the first
    /// byte are returned here; errors mid-stream arrive as stream items.
    async fn stream(
        &self,
        request: ModelRequest,
        cancel: CancellationToken,
    ) -> Result<ModelStream, ModelError>;
}

/// Fold a stream into a complete response, calling `on_event` for each event
/// as it arrives (for live display).
pub async fn collect(
    mut stream: ModelStream,
    mut on_event: impl FnMut(&StreamEvent),
) -> Result<ModelResponse, ModelError> {
    let mut acc = StreamAccumulator::new();
    while let Some(item) = stream.next().await {
        let event = item?;
        on_event(&event);
        acc.apply(&event).map_err(accumulate_error)?;
    }
    acc.finish().map_err(accumulate_error)
}

/// A stream that failed part-way, with everything accumulated before the failure.
#[derive(Debug, Clone, PartialEq)]
pub struct PartialFailure {
    pub error: ModelError,
    pub partial: ModelResponse,
}

/// Fold a stream, returning what accumulated so far alongside a mid-stream error.
pub async fn collect_partial(
    mut stream: ModelStream,
    mut on_event: impl FnMut(&StreamEvent),
) -> Result<ModelResponse, Box<PartialFailure>> {
    let mut acc = StreamAccumulator::new();
    let fail = |error: ModelError, acc: StreamAccumulator| {
        Box::new(PartialFailure {
            error,
            partial: acc.finish().unwrap_or_default(),
        })
    };
    while let Some(item) = stream.next().await {
        match item {
            Ok(event) => {
                on_event(&event);
                if let Err(e) = acc.apply(&event) {
                    return Err(fail(accumulate_error(e), acc));
                }
            }
            Err(e) => return Err(fail(e, acc)),
        }
    }
    acc.finish().map_err(|e| {
        Box::new(PartialFailure {
            error: accumulate_error(e),
            partial: ModelResponse::default(),
        })
    })
}

fn accumulate_error(err: AccumulateError) -> ModelError {
    let class = match err {
        AccumulateError::InvalidToolInput { .. } => ModelErrorClass::InvalidRequest,
        _ => ModelErrorClass::Unknown,
    };
    ModelError::new(class, format!("malformed model stream: {err}"))
}
