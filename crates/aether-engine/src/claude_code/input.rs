//! Relay user steering separately from the native control-plane stop request.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::mpsc;

use super::stream::Steered;
use crate::EngineInput;

pub(super) fn interrupt_line() -> String {
    format!(
        "{}\n",
        serde_json::json!({
            "type": "control_request",
            "request_id": crate::new_session_id(),
            "request": {"subtype": "interrupt"},
        })
    )
}

pub(super) fn spawn(
    stdin: mpsc::Sender<String>,
    outstanding: Arc<Mutex<VecDeque<Steered>>>,
) -> (mpsc::Sender<EngineInput>, mpsc::Sender<Duration>) {
    let (input_tx, mut input_rx) = mpsc::channel::<EngineInput>(16);
    let (stop_tx, mut stop_rx) = mpsc::channel::<Duration>(1);
    tokio::spawn(async move {
        let mut stop_open = true;
        loop {
            tokio::select! {
                // The runtime queues stop before closing user input. If both
                // are ready, interrupt before EOF; do not forward queued
                // steering to a model whose turn has already parked.
                biased;
                grace = stop_rx.recv(), if stop_open => {
                    let Some(grace) = grace else {
                        stop_open = false;
                        continue;
                    };
                    match tokio::time::timeout(grace.min(Duration::from_secs(2)),
                        stdin.send(interrupt_line())).await {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => tracing::warn!(%error, "Claude park interrupt input closed"),
                        Err(_) => tracing::warn!("Claude park interrupt input timed out"),
                    }
                    break;
                }
                message = input_rx.recv() => {
                    let Some(message) = message else { break; };
                    let uuid = crate::new_session_id();
                    let line = super::steered_message_line(&uuid, &message.text, &message.images);
                    // Remembered before it is written, so its echo cannot come first.
                    outstanding
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push_back(Steered { uuid, text: message.text });
                    if stdin.send(line).await.is_err() {
                        break;
                    }
                }
            }
        }
        // The process writer drains the control frame before closing stdin.
        // The process reader still collects native result and usage events.
    });
    (input_tx, stop_tx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn parking_interrupt_precedes_eof_and_discards_unacknowledged_steering() {
        let (stdin, mut lines) = mpsc::channel(16);
        let outstanding = Arc::new(Mutex::new(VecDeque::new()));
        let (input, stop) = spawn(stdin, outstanding.clone());
        // No await before both are queued, so the relay sees both ready.
        input.try_send("next request".into()).unwrap();
        stop.try_send(Duration::from_secs(1)).unwrap();
        drop(input);
        let line = tokio::time::timeout(Duration::from_secs(2), lines.recv())
            .await
            .unwrap()
            .unwrap();
        let message: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(message["type"], "control_request");
        assert_eq!(message["request"]["subtype"], "interrupt");
        assert!(
            tokio::time::timeout(Duration::from_secs(2), lines.recv())
                .await
                .unwrap()
                .is_none()
        );
        assert!(outstanding.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn normal_input_closure_sends_no_interrupt() {
        let (stdin, mut lines) = mpsc::channel(16);
        let (input, _stop) = spawn(stdin, Arc::new(Mutex::new(VecDeque::new())));
        drop(input);
        assert!(
            tokio::time::timeout(Duration::from_secs(2), lines.recv())
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn user_steering_keeps_its_echo_tracking_without_a_stop_channel() {
        let (stdin, mut lines) = mpsc::channel(16);
        let outstanding = Arc::new(Mutex::new(VecDeque::new()));
        let (input, stop) = spawn(stdin, outstanding.clone());
        drop(stop);
        input.send("new direction".into()).await.unwrap();
        let line = tokio::time::timeout(Duration::from_secs(2), lines.recv())
            .await
            .unwrap()
            .unwrap();
        let steered = outstanding.lock().unwrap().front().cloned().unwrap();
        assert_eq!(steered.text, "new direction");
        assert_eq!(
            line,
            super::super::steered_message_line(&steered.uuid, "new direction", &[])
        );
        drop(input);
        assert!(
            tokio::time::timeout(Duration::from_secs(2), lines.recv())
                .await
                .unwrap()
                .is_none()
        );
    }
}
