//! Stream public JSON text now; validate and return tool calls only at EOF.

use std::collections::VecDeque;

use aether_core::model::{
    BlockStart, ModelError, ModelErrorClass, StreamAccumulator, StreamEvent, Usage,
};
use aether_model::ModelStream;
use futures::StreamExt;

use super::text::PublicText;
use super::{MALFORMED_REPLY, Reply, failed_events, parse_reply, reply_events};

struct ParsedStream {
    source: ModelStream,
    reply: StreamAccumulator,
    public: PublicText,
    pending: VecDeque<Result<StreamEvent, ModelError>>,
    done: bool,
    decoding_error: Option<&'static str>,
}

pub(super) fn parsed_events(source: ModelStream) -> ModelStream {
    let state = ParsedStream {
        source,
        reply: StreamAccumulator::new(),
        public: PublicText::default(),
        pending: VecDeque::new(),
        done: false,
        decoding_error: None,
    };
    Box::pin(futures::stream::unfold(state, |mut state| async move {
        loop {
            if let Some(event) = state.pending.pop_front() {
                return Some((event, state));
            }
            if state.done {
                return None;
            }
            match state.source.next().await {
                Some(Ok(event)) => {
                    if let Err(error) = state.accept(event) {
                        state.fail(error);
                    }
                }
                Some(Err(error)) => state.fail(error),
                None => state.finish(),
            }
        }
    }))
}

impl ParsedStream {
    fn accept(&mut self, event: StreamEvent) -> Result<(), ModelError> {
        debug_assert!(!self.done);
        self.reply
            .apply(&event)
            .map_err(|_| malformed("invalid engine event stream"))?;
        let text = match event {
            StreamEvent::TextDelta { text, .. }
            | StreamEvent::BlockStart {
                block: BlockStart::Text { text },
                ..
            } => text,
            StreamEvent::Usage { usage } => {
                self.pending.push_back(Ok(StreamEvent::Usage { usage }));
                return Ok(());
            }
            _ => return Ok(()),
        };
        if self.decoding_error.is_some() {
            return Ok(());
        }
        let opened = !self.public.emitted.is_empty();
        let text = match self.public.push(&text) {
            Ok(text) => text,
            Err(reason) => {
                self.decoding_error = Some(reason);
                return Ok(());
            }
        };
        if text.is_empty() {
            return Ok(());
        }
        if !opened {
            self.pending.extend([
                Ok(StreamEvent::MessageStart {
                    provider_message_id: None,
                    model: None,
                }),
                Ok(StreamEvent::BlockStart {
                    index: 0,
                    block: BlockStart::Text {
                        text: String::new(),
                    },
                }),
            ]);
        }
        debug_assert!(!self.public.emitted.is_empty());
        self.pending
            .push_back(Ok(StreamEvent::TextDelta { index: 0, text }));
        Ok(())
    }

    fn fail(&mut self, error: ModelError) {
        debug_assert!(!self.done);
        self.pending
            .extend(failed_events(self.reply.usage(), error));
        self.done = true;
        debug_assert!(matches!(self.pending.back(), Some(Err(_))));
    }

    fn finish(&mut self) {
        debug_assert!(!self.done);
        if let Some(reason) = self.decoding_error {
            self.fail(malformed(reason));
            return;
        }
        let usage = self.reply.usage();
        let result = std::mem::take(&mut self.reply)
            .finish()
            .map_err(|_| "invalid engine event stream".to_string())
            .and_then(|reply| parse_reply(&reply.text()))
            .and_then(|reply| remaining_events(usage, reply, &self.public.emitted));
        match result {
            Ok(events) => self.pending.extend(events),
            Err(reason) => self
                .pending
                .extend(failed_events(usage, malformed(&reason))),
        }
        self.done = true;
        debug_assert!(matches!(
            self.pending.back(),
            Some(Ok(StreamEvent::MessageStop)) | Some(Err(_))
        ));
    }
}

fn malformed(reason: &str) -> ModelError {
    ModelError::new(
        ModelErrorClass::Server,
        format!("{MALFORMED_REPLY}: {reason}"),
    )
}

fn remaining_events(
    usage: Usage,
    mut reply: Reply,
    emitted: &str,
) -> Result<Vec<Result<StreamEvent, ModelError>>, String> {
    if emitted.is_empty() {
        return Ok(reply_events(usage, reply, false));
    }
    let text = reply.text.as_deref().unwrap_or("");
    if text != emitted && !(text.is_empty() && emitted.trim().is_empty()) {
        return Err("the final public text differs from its streamed prefix".into());
    }
    debug_assert!(!emitted.is_empty());
    reply.text.get_or_insert_default();
    let events = reply_events(usage, reply, true);
    debug_assert!(
        events
            .iter()
            .all(|event| !matches!(event, Ok(StreamEvent::TextDelta { .. })))
    );
    Ok(events)
}
