//! The delivery worker: takes due notices from the outbox, encrypts, signs and posts each, and
//! reports how it went. The event bus never waits on it.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use aether_protocol::remote::DeviceId;
use tokio::task::JoinSet;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::notice::Notice;
use super::notifier::Inner;
use super::outbox::Settled;
use super::transport::{PushRequest, TransportError};
use super::{PushError, StoredSubscription, TTL_SECS, ece, subscription};

/// Sends under way at once, each to a different device.
const MAX_FLIGHTS: usize = 4;
/// A send that has not ended by now is abandoned, whatever the transport
/// does: a device is never left waiting on a request that hangs.
pub(super) const SEND_DEADLINE: Duration = Duration::from_secs(30);

/// How one attempt to deliver ended.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Attempt {
    Delivered,
    /// The push service said 404 or 410: the subscription sent to is dead.
    Gone {
        dead: StoredSubscription,
        why: String,
    },
    /// The device has no subscription to send to.
    Unsubscribed,
    /// The device is revoked or unknown, and must not keep a subscription.
    Revoked,
    /// 429, 5xx, a timeout or no connection: worth another try.
    Retry {
        why: String,
        retry_after: Option<Duration>,
    },
    /// This message will never be accepted, or must not be sent.
    Dropped(String),
}

/// Who a notice for a device goes to.
#[derive(Debug)]
enum Recipient {
    Subscribed(StoredSubscription),
    Unsubscribed,
    Revoked,
}

/// What a push service's status means for the subscription `sent_to`
/// (RFC 8030 section 5 and 7.3).
fn judge(status: u16, retry_after: Option<Duration>, sent_to: StoredSubscription) -> Attempt {
    match status {
        200..=299 => Attempt::Delivered,
        404 | 410 => Attempt::Gone {
            dead: sent_to,
            why: format!("status {status}"),
        },
        408 | 429 | 500..=599 => Attempt::Retry {
            why: format!("status {status}"),
            retry_after,
        },
        _ => Attempt::Dropped(format!("status {status}")),
    }
}

/// Deliver what is queued until `shutdown`; sends in flight are abandoned then.
pub(super) async fn run(inner: Arc<Inner>, shutdown: CancellationToken) {
    let mut flights: JoinSet<()> = JoinSet::new();
    loop {
        launch(&inner, &mut flights);
        let due = next_look(&inner, &flights);
        tokio::select! {
            () = shutdown.cancelled() => break,
            () = inner.wake.notified() => {}
            () = sleep_until(due) => {}
            Some(landed) = flights.join_next() => {
                if let Err(e) = landed {
                    tracing::error!(error = %e, "a push delivery task did not finish");
                }
            }
        }
    }
    flights.shutdown().await;
}

/// When the worker should next look at the outbox unprompted. With every flight taken nothing can
/// leave, so no clock is set: a landing, a change or shutdown wakes it.
fn next_look(inner: &Inner, flights: &JoinSet<()>) -> Option<Instant> {
    assert!(
        flights.len() <= MAX_FLIGHTS,
        "flights stay within their limit"
    );
    if flights.len() == MAX_FLIGHTS {
        return None;
    }
    inner.outbox.lock().next_due()
}

async fn sleep_until(due: Option<Instant>) {
    match due {
        Some(due) => tokio::time::sleep_until(due).await,
        None => std::future::pending().await,
    }
}

/// Start a send for every due notice, up to the flight limit.
fn launch(inner: &Arc<Inner>, flights: &mut JoinSet<()>) {
    while flights.len() < MAX_FLIGHTS {
        let Some(job) = inner.outbox.lock().take_due(Instant::now()) else {
            return;
        };
        let inner = Arc::clone(inner);
        flights.spawn(async move {
            let attempt = attempt(&inner, job.device, &job.notice).await;
            let settled = settle(&inner, job.device, attempt);
            inner.outbox.lock().settle(job, settled, Instant::now());
            inner.wake.notify_one();
        });
    }
}

/// Act on how an attempt ended and translate it for the outbox.
fn settle(inner: &Inner, device: DeviceId, attempt: Attempt) -> Settled {
    match attempt {
        Attempt::Delivered => Settled::Delivered,
        Attempt::Gone { dead, why } => retire(inner, &dead, &why),
        Attempt::Unsubscribed => Settled::Gone,
        Attempt::Revoked => {
            tracing::info!(%device, "a revoked device had a push subscription; removing it");
            if let Err(e) = inner.store.remove_subscription(device) {
                tracing::error!(%device, error = %e, "removing a revoked device's push subscription failed");
            }
            Settled::Gone
        }
        Attempt::Retry { why, retry_after } => {
            tracing::debug!(%device, %why, "push delivery will be retried");
            Settled::Retry {
                retry_after,
                jitter: jitter(),
            }
        }
        Attempt::Dropped(why) => {
            tracing::warn!(%device, %why, "push notification was not delivered");
            Settled::Dropped
        }
    }
}

/// Remove the subscription a push service disowned. One the device replaced
/// while it was being sent to stays, and the notice goes to that instead.
fn retire(inner: &Inner, dead: &StoredSubscription, why: &str) -> Settled {
    let device = dead.device;
    match inner.store.remove_exact_subscription(dead) {
        Ok(true) => {
            tracing::info!(%device, %why, "push subscription is gone; it was removed");
            Settled::Gone
        }
        Ok(false) => {
            tracing::info!(%device, %why, "a push subscription died after it was replaced");
            Settled::Resend
        }
        Err(e) => {
            tracing::error!(%device, error = %e, "removing a dead push subscription failed");
            Settled::Gone
        }
    }
}

/// A uniform fraction in `[0, 1]`. A failed generator means no spread, which
/// only makes retries wait the full step.
fn jitter() -> f64 {
    let mut bytes = [0u8; 4];
    match aws_lc_rs::rand::fill(&mut bytes) {
        Ok(()) => f64::from(u32::from_le_bytes(bytes)) / f64::from(u32::MAX),
        Err(_) => 1.0,
    }
}

/// `test_push`: one attempt, reported to the caller instead of retried.
pub(super) async fn send_now(
    inner: &Inner,
    device: DeviceId,
    notice: &Notice,
) -> Result<(), PushError> {
    match attempt(inner, device, notice).await {
        Attempt::Delivered => Ok(()),
        Attempt::Gone { dead, why } => {
            if !inner.store.remove_exact_subscription(&dead)? {
                return Err(PushError::Service(format!(
                    "{why}, for a subscription the device has replaced since; test again"
                )));
            }
            inner.outbox.lock().forget(device);
            Err(PushError::Service(format!(
                "{why}; the subscription was removed"
            )))
        }
        Attempt::Unsubscribed => Err(PushError::NotSubscribed),
        Attempt::Revoked => {
            inner.store.remove_subscription(device)?;
            inner.outbox.lock().forget(device);
            Err(PushError::Service(
                "the device is revoked; its subscription was removed".into(),
            ))
        }
        Attempt::Retry { why, .. } | Attempt::Dropped(why) => Err(PushError::Service(why)),
    }
}

/// Try once to deliver `notice` to `device` as it is subscribed right now.
async fn attempt(inner: &Inner, device: DeviceId, notice: &Notice) -> Attempt {
    let stored = match recipient_of(inner, device) {
        Ok(Recipient::Subscribed(stored)) => stored,
        Ok(Recipient::Unsubscribed) => return Attempt::Unsubscribed,
        Ok(Recipient::Revoked) => return Attempt::Revoked,
        Err(e) => {
            return Attempt::Retry {
                why: e.to_string(),
                retry_after: None,
            };
        }
    };
    if !stored.wants(notice.payload.kind) {
        return Attempt::Dropped("the device no longer asks for this kind".into());
    }
    let request = match request(inner, &stored, notice) {
        Ok(request) => request,
        Err(e) => return Attempt::Dropped(e.to_string()),
    };
    let Ok(answer) = tokio::time::timeout(SEND_DEADLINE, inner.transport.post(request)).await
    else {
        return Attempt::Retry {
            why: "the push service did not answer in time".into(),
            retry_after: None,
        };
    };
    match answer {
        Ok(response) => judge(response.status, response.retry_after, stored),
        Err(TransportError::Refused(why)) => Attempt::Dropped(why),
        Err(TransportError::Unreachable(why)) => Attempt::Retry {
            why,
            retry_after: None,
        },
    }
}

/// The device's subscription as stored right now, unless the device is
/// revoked or unknown.
fn recipient_of(inner: &Inner, device: DeviceId) -> Result<Recipient, PushError> {
    let Some(stored) = inner.store.subscription(device)? else {
        return Ok(Recipient::Unsubscribed);
    };
    if stored.device != device {
        return Err(PushError::Store(format!(
            "Push subscription for {device} contains a different device; run `aether doctor`"
        )));
    }
    let Some(devices) = inner.devices.get() else {
        return Ok(Recipient::Subscribed(stored));
    };
    let record = devices
        .device(device)
        .map_err(|e| PushError::Store(e.to_string()))?;
    if record.is_some_and(|record| record.revoked_at.is_none()) {
        return Ok(Recipient::Subscribed(stored));
    }
    Ok(Recipient::Revoked)
}

/// Encrypt `notice` for `stored` and sign the request to its push service.
fn request(
    inner: &Inner,
    stored: &StoredSubscription,
    notice: &Notice,
) -> Result<PushRequest, PushError> {
    if !inner.enabled() {
        return Err(PushError::Disabled);
    }
    let endpoint = inner.config.hosts.check(&stored.subscription.endpoint)?;
    let recipient = subscription::recipient(&stored.subscription)?;
    let key = inner
        .key(false)?
        .ok_or_else(|| PushError::Key("push is enabled but no VAPID key exists".into()))?;
    let plaintext =
        serde_json::to_vec(&notice.payload).map_err(|e| PushError::Message(e.to_string()))?;
    let body = ece::seal(&recipient, &plaintext).map_err(|e| PushError::Message(e.to_string()))?;
    let authorization = key
        .authorization(
            &subscription::audience(&endpoint),
            inner.subject().as_deref(),
            SystemTime::now(),
        )
        .map_err(|e| PushError::Key(e.to_string()))?;
    Ok(PushRequest {
        endpoint,
        ttl: TTL_SECS,
        urgency: notice.urgency,
        topic: notice.topic.clone(),
        authorization,
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::push::testing::UserAgent;

    #[test]
    fn statuses_are_judged_as_the_contract_says() {
        let sent_to = StoredSubscription {
            device: DeviceId::new(),
            subscription: UserAgent::generate().subscription("https://push.example/a"),
            kinds: Vec::new(),
        };
        let judge = |status, retry_after| judge(status, retry_after, sent_to.clone());
        for ok in [200, 201, 202, 204] {
            assert_eq!(judge(ok, None), Attempt::Delivered, "{ok}");
        }
        for gone in [404, 410] {
            assert!(
                matches!(judge(gone, None), Attempt::Gone { dead, .. } if dead == sent_to),
                "{gone}: it names the subscription that was sent to"
            );
        }
        let wait = Some(Duration::from_secs(30));
        for retried in [408, 429, 500, 502, 503, 504, 599] {
            assert!(
                matches!(judge(retried, wait), Attempt::Retry { retry_after, .. } if retry_after == wait),
                "{retried}"
            );
        }
        for dropped in [301, 400, 401, 403, 413] {
            assert!(
                matches!(judge(dropped, wait), Attempt::Dropped(_)),
                "{dropped}"
            );
        }
    }

    #[test]
    fn jitter_is_a_fraction() {
        for _ in 0..64 {
            let sample = jitter();
            assert!((0.0..=1.0).contains(&sample), "{sample}");
        }
    }
}
