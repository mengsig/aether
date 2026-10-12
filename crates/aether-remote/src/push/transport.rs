//! The one HTTP request a notification is: RFC 8030's `POST` to the
//! subscription's endpoint, through `aether-net`'s vetted and pinned client.

use std::time::{Duration, SystemTime};

use super::notice::Urgency;

/// How long one request may take, connection included.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const USER_AGENT: &str = concat!("aether-remote/", env!("CARGO_PKG_VERSION"));

/// One push message, encrypted and signed.
#[derive(Clone, PartialEq, Eq)]
pub struct PushRequest {
    pub endpoint: url::Url,
    /// Seconds the push service may hold the message.
    pub ttl: u32,
    pub urgency: Urgency,
    pub topic: String,
    /// The VAPID `Authorization` header value.
    pub authorization: String,
    /// The `aes128gcm` body.
    pub body: Vec<u8>,
}

/// Manual: the endpoint is a capability and the token a credential.
impl std::fmt::Debug for PushRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PushRequest")
            .field("host", &self.endpoint.host_str())
            .field("urgency", &self.urgency)
            .field("topic", &self.topic)
            .field("body_bytes", &self.body.len())
            .finish_non_exhaustive()
    }
}

/// What the push service answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PushResponse {
    pub status: u16,
    /// The service's `Retry-After`, as a delay from now.
    pub retry_after: Option<Duration>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransportError {
    /// Policy forbids this endpoint. Trying again cannot change that.
    #[error("refused: {0}")]
    Refused(String),
    /// The service could not be reached or did not answer. Worth a retry.
    #[error("unreachable: {0}")]
    Unreachable(String),
}

/// Carries a push request to its service.
#[async_trait::async_trait]
pub trait Transport: Send + Sync + std::fmt::Debug + 'static {
    async fn post(&self, request: PushRequest) -> Result<PushResponse, TransportError>;
}

/// The real transport: https only, public addresses only, the connection
/// pinned to the addresses that were checked, no redirect and no proxy.
#[derive(Debug, Clone, Copy, Default)]
pub struct NetTransport;

#[async_trait::async_trait]
impl Transport for NetTransport {
    async fn post(&self, request: PushRequest) -> Result<PushResponse, TransportError> {
        if request.endpoint.scheme() != "https" {
            return Err(TransportError::Refused(failure_reason(
                &request.endpoint,
                "https_required",
            )));
        }
        let target = aether_net::vet(
            request.endpoint.as_str(),
            aether_net::AddressPolicy::PublicOnly,
        )
        .await
        .map_err(|refusal| match refusal {
            aether_net::NetError::Denied(_) => {
                TransportError::Refused(failure_reason(&request.endpoint, "address_policy"))
            }
            // A name that does not resolve right now may resolve later.
            aether_net::NetError::Invalid(_) | aether_net::NetError::Internal(_) => {
                TransportError::Unreachable(failure_reason(&request.endpoint, "resolution"))
            }
        })?;
        let client =
            aether_net::pinned_client(&target, REQUEST_TIMEOUT, USER_AGENT).map_err(|_| {
                TransportError::Unreachable(failure_reason(
                    &request.endpoint,
                    "client_configuration",
                ))
            })?;
        send(&client, target.url, request).await
    }
}

fn failure_reason(endpoint: &url::Url, category: &str) -> String {
    format!(
        "{}: {category}",
        endpoint.host_str().unwrap_or("unknown_host")
    )
}

/// Send `request` to `url` over `client` and read the answer's status.
pub(crate) async fn send(
    client: &reqwest::Client,
    url: url::Url,
    request: PushRequest,
) -> Result<PushResponse, TransportError> {
    assert!(!request.body.is_empty(), "a push message has a body");
    assert!(!request.topic.is_empty(), "a push message has a topic");
    let host = failure_reason(&url, "request_failed");
    let response = client
        .post(url)
        .header("TTL", request.ttl)
        .header("Urgency", request.urgency.header())
        .header("Topic", &request.topic)
        .header("Content-Encoding", "aes128gcm")
        .header("Content-Type", "application/octet-stream")
        .header("Authorization", &request.authorization)
        .body(request.body)
        .send()
        .await
        // The URL is the subscription's capability; keep it out of the error.
        .map_err(|_| TransportError::Unreachable(host))?;
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| retry_after(value, SystemTime::now()));
    Ok(PushResponse {
        status: response.status().as_u16(),
        retry_after,
    })
}

/// A `Retry-After` value as a delay: seconds, or an HTTP date measured from `now`.
pub(crate) fn retry_after(value: &str, now: SystemTime) -> Option<Duration> {
    let value = value.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let at: SystemTime = chrono::DateTime::parse_from_rfc2822(value).ok()?.into();
    Some(at.duration_since(now).unwrap_or(Duration::ZERO))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_reads_seconds_and_dates() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_445_412_480);
        assert_eq!(retry_after("120", now), Some(Duration::from_secs(120)));
        assert_eq!(retry_after(" 0 ", now), Some(Duration::ZERO));
        let later = "Wed, 21 Oct 2015 07:29:00 GMT";
        assert_eq!(retry_after(later, now), Some(Duration::from_secs(60)));
        let earlier = "Wed, 21 Oct 2015 07:27:00 GMT";
        assert_eq!(retry_after(earlier, now), Some(Duration::ZERO));
        for bad in ["", "soon", "-5", "1.5"] {
            assert_eq!(retry_after(bad, now), None, "{bad}");
        }
    }

    fn request(endpoint: &str) -> PushRequest {
        PushRequest {
            endpoint: url::Url::parse(endpoint).expect("a url"),
            ttl: 60,
            urgency: Urgency::Normal,
            topic: "topic".into(),
            authorization: "vapid t=x, k=y".into(),
            body: vec![1],
        }
    }

    #[tokio::test]
    async fn the_real_transport_refuses_plaintext_and_private_addresses() {
        for endpoint in [
            "http://fcm.googleapis.com/fcm/send/abc",
            "https://127.0.0.1/x",
            "https://localhost/x",
            "https://10.0.0.7/x",
            "https://192.168.1.10:8443/x",
            "https://169.254.169.254/latest/meta-data",
            "https://100.64.0.9/x",
            "https://[::1]/x",
            "https://[fd00::1]/x",
        ] {
            let refused = NetTransport.post(request(endpoint)).await;
            assert!(
                matches!(refused, Err(TransportError::Refused(_))),
                "{endpoint}: {refused:?}"
            );
        }
    }

    #[tokio::test]
    async fn failed_push_transport_errors_never_include_endpoint_capabilities() {
        for endpoint in [
            "https://127.0.0.1/SECRET_PATH?SECRET_QUERY=token",
            "https://missing.invalid/SECRET_PATH?SECRET_QUERY=token",
        ] {
            let error = NetTransport.post(request(endpoint)).await.unwrap_err();
            let printed = format!("{error} {error:?}");
            assert!(printed.contains(request(endpoint).endpoint.host_str().unwrap()));
            assert!(!printed.contains("SECRET_PATH"), "{printed}");
            assert!(!printed.contains("SECRET_QUERY"), "{printed}");
            assert!(!printed.contains("token"), "{printed}");
        }
    }

    #[test]
    fn a_request_does_not_print_its_secrets() {
        let printed = format!(
            "{:?}",
            request("https://fcm.googleapis.com/fcm/send/SECRET")
        );
        assert!(printed.contains("fcm.googleapis.com"), "{printed}");
        assert!(!printed.contains("SECRET"), "{printed}");
        assert!(!printed.contains("vapid"), "{printed}");
    }
}
