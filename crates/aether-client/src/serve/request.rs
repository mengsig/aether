//! Authenticate and dispatch one inbound delivery, logging every refusal.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use aether_core::id::AgentId;
use aether_protocol::{AgentQuery, Command, Reply};
use http_body_util::{BodyExt as _, Full, LengthLimitError, Limited};
use hyper::body::{Bytes, Incoming};
use hyper::{Method, Request, Response, StatusCode};
use tokio::sync::Semaphore;

use super::headers::SignedHeaders;
use super::inbound::Webhook;
use super::replay::{Refused, ReplayGuard};
use super::{BODY_READ_TIMEOUT, MAX_BODY_BYTES, signature};
use crate::sink::{CommandSink, DaemonUnavailable};

const BAD_SIGNATURE_DELAY: Duration = Duration::from_millis(250);

#[derive(Clone)]
struct RejectionReason(String);

fn refuse(status: StatusCode, why: &str, reason: &str) -> Response<Full<Bytes>> {
    let body = serde_json::json!({ "error": why }).to_string();
    let mut response = json_response(status, body);
    response
        .extensions_mut()
        .insert(RejectionReason(reason.to_owned()));
    // Explicit close lets Hyper flush a refusal without consuming an oversized upload.
    response.headers_mut().insert(
        hyper::header::CONNECTION,
        hyper::header::HeaderValue::from_static("close"),
    );
    response
}

fn json_response(status: StatusCode, body: String) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::new(Bytes::from(body)));
    *response.status_mut() = status;
    response.headers_mut().insert(
        hyper::header::CONTENT_TYPE,
        hyper::header::HeaderValue::from_static("application/json"),
    );
    response
}

pub(super) async fn answer(
    sink: &dyn CommandSink,
    routes: &HashMap<String, Webhook>,
    guard: Arc<ReplayGuard>,
    pre_auth: &Semaphore,
    request: Request<Incoming>,
) -> Response<Full<Bytes>> {
    let route = request.uri().path().to_owned();
    let response = answer_request(sink, routes, guard, pre_auth, request).await;
    if let Some(reason) = response.extensions().get::<RejectionReason>() {
        tracing::warn!(
            route,
            reason = %reason.0,
            status = response.status().as_u16(),
            "webhook rejected"
        );
    }
    response
}

async fn answer_request(
    sink: &dyn CommandSink,
    routes: &HashMap<String, Webhook>,
    guard: Arc<ReplayGuard>,
    pre_auth: &Semaphore,
    request: Request<Incoming>,
) -> Response<Full<Bytes>> {
    if request.method() != Method::POST {
        return refuse(
            StatusCode::METHOD_NOT_ALLOWED,
            "a webhook is a POST",
            "method is not POST",
        );
    }
    let Some(hook) = routes.get(request.uri().path()).cloned() else {
        return refuse(
            StatusCode::NOT_FOUND,
            "no webhook is configured on this path",
            "unknown route",
        );
    };
    let Ok(unverified) = pre_auth.try_acquire() else {
        return refuse(
            StatusCode::SERVICE_UNAVAILABLE,
            "too many unverified deliveries are in flight; retry shortly",
            "pre-auth budget exhausted",
        );
    };
    let (headers, body) = match authenticate(&hook, request).await {
        Ok(delivery) => delivery,
        Err(response) => return *response,
    };
    let route = hook.path.clone();
    let admitted = tokio::task::spawn_blocking(move || {
        guard.admit(
            &route,
            &headers.stamp,
            &headers.signature,
            headers.delivery_id.as_deref(),
            aether_core::time::now().timestamp(),
        )
    })
    .await;
    if let Err(refused) = admitted.unwrap_or(Err(Refused::Unavailable)) {
        return replay_refusal(refused);
    }
    drop(unverified);
    dispatch(sink, &hook, &body).await
}

async fn signed_headers(
    request: &Request<Incoming>,
) -> Result<SignedHeaders, Box<Response<Full<Bytes>>>> {
    let length = request
        .headers()
        .get(hyper::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    if length.is_some_and(|length| length > MAX_BODY_BYTES as u64) {
        return Err(Box::new(refuse(
            StatusCode::PAYLOAD_TOO_LARGE,
            "the body is too large",
            "body exceeds limit",
        )));
    }
    match SignedHeaders::parse(request.headers()) {
        Ok(headers) => Ok(headers),
        Err(why) => {
            tokio::time::sleep(BAD_SIGNATURE_DELAY).await;
            Err(Box::new(refuse(StatusCode::UNAUTHORIZED, &why, &why)))
        }
    }
}

async fn authenticate(
    hook: &Webhook,
    request: Request<Incoming>,
) -> Result<(SignedHeaders, Bytes), Box<Response<Full<Bytes>>>> {
    let headers = signed_headers(&request).await?;
    let body = read_body(request.into_body()).await?;
    if !signature::verify(
        &hook.secret,
        &signature::signed_payload(&headers.stamp, &body),
        &headers.signature,
    ) {
        tokio::time::sleep(BAD_SIGNATURE_DELAY).await;
        return Err(Box::new(refuse(
            StatusCode::UNAUTHORIZED,
            "the signature does not match the stamp and body",
            "signature mismatch",
        )));
    }
    Ok((headers, body))
}

async fn read_body(body: Incoming) -> Result<Bytes, Box<Response<Full<Bytes>>>> {
    match tokio::time::timeout(
        BODY_READ_TIMEOUT,
        Limited::new(body, MAX_BODY_BYTES).collect(),
    )
    .await
    {
        Ok(Ok(collected)) => Ok(collected.to_bytes()),
        Ok(Err(error)) if error.downcast_ref::<LengthLimitError>().is_some() => {
            Err(Box::new(refuse(
                StatusCode::PAYLOAD_TOO_LARGE,
                "the body is too large",
                "body exceeds limit",
            )))
        }
        Ok(Err(_)) => Err(Box::new(refuse(
            StatusCode::BAD_REQUEST,
            "the body did not arrive",
            "body read failed",
        ))),
        Err(_) => Err(Box::new(refuse(
            StatusCode::REQUEST_TIMEOUT,
            "the body did not arrive inside the read timeout",
            "body read timed out",
        ))),
    }
}

fn replay_refusal(refused: Refused) -> Response<Full<Bytes>> {
    let status = match refused {
        Refused::Replayed | Refused::Stale { .. } | Refused::Ahead { .. } => StatusCode::CONFLICT,
        Refused::Saturated | Refused::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    };
    let reason = match refused {
        Refused::Replayed => "replayed delivery",
        Refused::Stale { .. } => "stale timestamp",
        Refused::Ahead { .. } => "future timestamp",
        Refused::Saturated => "replay capacity exhausted",
        Refused::Unavailable => "replay storage unavailable",
    };
    refuse(status, &refused.to_string(), reason)
}

async fn dispatch(sink: &dyn CommandSink, hook: &Webhook, body: &[u8]) -> Response<Full<Bytes>> {
    let payload = match serde_json::from_slice::<serde_json::Value>(body) {
        Ok(payload) => payload,
        Err(e) => {
            return refuse(
                StatusCode::BAD_REQUEST,
                &format!("body is not JSON: {e}"),
                "invalid JSON",
            );
        }
    };
    let agent = match resolve(sink, &hook.agent).await {
        Ok(agent) => agent,
        Err(Refusal { status, why }) => return refuse(status, &why, "agent resolution failed"),
    };
    match sink
        .call(Command::Trigger {
            agent,
            source: hook.source.clone(),
            payload,
            idempotency_key: None,
        })
        .await
    {
        Ok(_) => json_response(
            StatusCode::ACCEPTED,
            serde_json::json!({ "triggered": agent.to_string() }).to_string(),
        ),
        Err(e) if is_unavailable(&e) => unavailable(&e),
        Err(e) => refuse(
            StatusCode::BAD_GATEWAY,
            &format!("the daemon refused it: {e}"),
            "daemon refused trigger",
        ),
    }
}

struct Refusal {
    status: StatusCode,
    why: String,
}

fn is_unavailable(error: &anyhow::Error) -> bool {
    error.downcast_ref::<DaemonUnavailable>().is_some()
}

/// Retryable, unlike an unknown path: the sender should try again shortly.
fn unavailable(error: &anyhow::Error) -> Response<Full<Bytes>> {
    refuse(
        StatusCode::SERVICE_UNAVAILABLE,
        &format!("the daemon is unavailable; retry shortly ({error})"),
        "daemon unavailable",
    )
}

/// An agent id, or the name of exactly one live agent. A name that is not
/// unique is refused rather than guessed at.
async fn resolve(sink: &dyn CommandSink, named: &str) -> Result<AgentId, Refusal> {
    if let Ok(id) = named.parse::<AgentId>() {
        return Ok(id);
    }
    let not_found = |why: String| Refusal {
        status: StatusCode::NOT_FOUND,
        why,
    };
    let listed = sink
        .call(Command::ListAgents {
            // Live only: a hook must not wake an agent that has already
            // finished, and once one has, a replacement with the same name
            // would otherwise make every POST ambiguous for the life of the
            // store.
            query: AgentQuery {
                live_only: true,
                ..Default::default()
            },
        })
        .await
        .map_err(|e| {
            if is_unavailable(&e) {
                return Refusal {
                    status: StatusCode::SERVICE_UNAVAILABLE,
                    why: format!("the daemon is unavailable; retry shortly ({e})"),
                };
            }
            not_found(format!("could not list agents: {e}"))
        })?;
    let Reply::Agents(agents) = listed else {
        return Err(not_found(
            "the daemon answered the agent list with something else".into(),
        ));
    };
    let mut matching = agents.iter().filter(|agent| agent.name == named);
    let Some(first) = matching.next() else {
        return Err(not_found(format!("no live agent is named `{named}`")));
    };
    if matching.next().is_some() {
        return Err(not_found(format!(
            "`{named}` names more than one live agent; configure the hook with an agent id"
        )));
    }
    Ok(first.id)
}
