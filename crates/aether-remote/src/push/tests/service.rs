//! A local fake push service speaking real HTTP: what a push service sees
//! on the wire, checked the way a push service and a browser would.

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aether_protocol::remote::AttentionKind;
use parking_lot::Mutex;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

use super::harness::{ENDPOINT, Harness, INSTANCE, SUBJECT, failed, settings};
use crate::push::PushError;
use crate::push::notice::Urgency;
use crate::push::testing::{RecordingTransport, verify_authorization};
use crate::push::transport::{PushRequest, PushResponse, Transport, TransportError, send};
use crate::push::vapid::TOKEN_LIFETIME;

/// One request as the push service received it.
#[derive(Debug)]
struct Received {
    request_line: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

type Script = Arc<Mutex<VecDeque<(u16, Option<&'static str>)>>>;

/// Answers `201 Created` unless scripted otherwise.
struct FakePushService {
    address: SocketAddr,
    received: mpsc::UnboundedReceiver<Received>,
    script: Script,
}

impl FakePushService {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a loopback port");
        let address = listener.local_addr().expect("an address");
        let (sender, received) = mpsc::unbounded_channel();
        let script = Script::default();
        let answers = script.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let answer = answers.lock().pop_front().unwrap_or((201, None));
                let request = serve(stream, answer).await.expect("a well-formed request");
                if sender.send(request).is_err() {
                    return;
                }
            }
        });
        Self {
            address,
            received,
            script,
        }
    }

    async fn next(&mut self) -> Received {
        tokio::time::timeout(Duration::from_secs(10), self.received.recv())
            .await
            .expect("a request within ten seconds")
            .expect("the service is running")
    }
}

/// Read one HTTP/1.1 request and answer it.
async fn serve(
    mut stream: TcpStream,
    answer: (u16, Option<&'static str>),
) -> std::io::Result<Received> {
    let mut buffer = Vec::new();
    let head_end = loop {
        if let Some(at) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break at;
        }
        assert_ne!(
            stream.read_buf(&mut buffer).await?,
            0,
            "the request head is complete"
        );
    };
    let head = String::from_utf8(buffer[..head_end].to_vec()).expect("an ASCII head");
    let mut lines = head.split("\r\n");
    let request_line = lines.next().expect("a request line").to_owned();
    let headers: HashMap<String, String> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    let length: usize = headers["content-length"].parse().expect("a content length");
    let mut body = buffer[head_end + 4..].to_vec();
    while body.len() < length {
        assert_ne!(stream.read_buf(&mut body).await?, 0, "the body is complete");
    }
    let (status, retry_after) = answer;
    let retry_after =
        retry_after.map_or(String::new(), |value| format!("Retry-After: {value}\r\n"));
    let response = format!(
        "HTTP/1.1 {status} Answer\r\n{retry_after}Content-Length: 0\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await?;
    Ok(Received {
        request_line,
        headers,
        body,
    })
}

/// Sends what the real transport would send, to the fake service instead:
/// the same request code, minus the address check that forbids loopback.
#[derive(Debug)]
struct ToFakeService {
    address: SocketAddr,
    client: reqwest::Client,
}

impl ToFakeService {
    fn new(address: SocketAddr) -> Self {
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("a client");
        Self { address, client }
    }
}

#[async_trait::async_trait]
impl Transport for ToFakeService {
    async fn post(&self, request: PushRequest) -> Result<PushResponse, TransportError> {
        let mut url = request.endpoint.clone();
        url.set_scheme("http").expect("http");
        url.set_ip_host(self.address.ip()).expect("an address");
        url.set_port(Some(self.address.port())).expect("a port");
        send(&self.client, url, request).await
    }
}

async fn harness_over(service: &FakePushService, previews: bool) -> Harness {
    let carrier = Arc::new(ToFakeService::new(service.address));
    let harness =
        Harness::with_transport(previews, Arc::new(RecordingTransport::default()), carrier);
    harness
        .notifier
        .apply_settings(&settings(true))
        .expect("enabling");
    harness
}

/// The token is ES256 under `key`, for the endpoint's origin, and expires
/// within the day RFC 8292 allows.
fn assert_vapid(authorization: &str, key: &str, sent_at: u64) {
    let token = verify_authorization(authorization).expect("a valid VAPID token");
    assert_eq!(token.key, key, "signed by the key devices subscribe with");
    assert_eq!(
        token.header,
        serde_json::json!({"typ": "JWT", "alg": "ES256"})
    );
    assert_eq!(token.claims["aud"], "https://fcm.googleapis.com");
    assert_eq!(token.claims["sub"], SUBJECT);
    let expiry = token.claims["exp"].as_u64().expect("an expiry");
    assert!(
        expiry > sent_at && expiry <= sent_at + 24 * 3600,
        "within a day: {expiry}"
    );
    assert!(expiry >= sent_at + TOKEN_LIFETIME.as_secs() - 60);
}

#[tokio::test]
async fn an_approval_arrives_signed_encrypted_and_addressed() {
    let mut service = FakePushService::start().await;
    let harness = harness_over(&service, false).await;
    let (_device, agent) = harness.subscribe();
    let root = harness.agent("builder");
    let (approval, seq) = harness.ask_approval(root);
    let sent_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("a clock")
        .as_secs();

    let request = service.next().await;
    assert_eq!(request.request_line, "POST /fcm/send/abc123 HTTP/1.1");
    assert_eq!(request.headers["ttl"], "3600");
    assert_eq!(request.headers["urgency"], "high");
    assert_eq!(request.headers["content-encoding"], "aes128gcm");
    assert_eq!(request.headers["content-type"], "application/octet-stream");
    let topic = &request.headers["topic"];
    assert_eq!(topic.len(), 32);
    assert!(
        topic
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    );
    assert!(request.body.len() <= 4096, "{} bytes", request.body.len());

    let config = harness.notifier.config(None).expect("config");
    let key = config.vapid_public_key.expect("a key exists once enabled");
    assert_vapid(&request.headers["authorization"], &key, sent_at);

    let payload = agent.payload(&request.body);
    assert_eq!(payload.v, 1);
    assert_eq!(payload.kind, AttentionKind::Approval);
    assert_eq!(payload.instance_id, INSTANCE);
    assert_eq!(payload.seq, seq);
    assert_eq!(payload.title, "Approval needed");
    assert_eq!(payload.url, format!("#attention?item={approval}"));
    assert_eq!(payload.agent, Some(root));
    assert_eq!(payload.agent_name.as_deref(), Some("builder"));
    assert_eq!(payload.approval, Some(approval));
    assert_eq!(payload.preview, None, "no content without push_previews");
    let plaintext = String::from_utf8(agent.open(&request.body).expect("ours")).expect("utf-8");
    assert!(!plaintext.contains("secret"), "{plaintext}");
}

#[tokio::test]
async fn a_status_notice_is_normal_urgency_and_names_the_first_origin_without_a_subject() {
    let mut service = FakePushService::start().await;
    let dir = tempfile::tempdir().expect("a directory");
    let mut config = super::harness::config(&dir, true);
    config.subject = None;
    let store = Arc::new(crate::push::testing::MemoryPushStore::default());
    let fleet = Arc::new(crate::push::testing::StaticFleet::default());
    let carrier = Arc::new(ToFakeService::new(service.address));
    let notifier = crate::push::Notifier::new(config, store, fleet, carrier);
    notifier.apply_settings(&settings(true)).expect("enabling");
    let device = aether_protocol::remote::DeviceId::new();
    let agent = crate::push::testing::UserAgent::generate();
    let mozilla = "https://updates.push.services.mozilla.com/wpush/v2/token?x=1";
    notifier
        .subscribe(device, agent.subscription(mozilla), Vec::new())
        .expect("subscribing");

    notifier
        .test(device)
        .await
        .expect("the service accepted it");
    let request = service.next().await;
    assert_eq!(request.request_line, "POST /wpush/v2/token?x=1 HTTP/1.1");
    assert_eq!(request.headers["urgency"], "normal");
    let token = verify_authorization(&request.headers["authorization"]).expect("a valid token");
    assert_eq!(
        token.claims["aud"],
        "https://updates.push.services.mozilla.com"
    );
    assert_eq!(token.claims["sub"], "https://box.example.ts.net");
    assert_eq!(agent.payload(&request.body).kind, AttentionKind::Test);
}

#[tokio::test]
async fn the_service_answers_reach_the_caller_and_a_gone_subscription_is_removed() {
    let mut service = FakePushService::start().await;
    let harness = harness_over(&service, false).await;
    let (device, _agent) = harness.subscribe();

    service.script.lock().push_back((503, None));
    let unavailable = harness.notifier.test(device).await.expect_err("a 503");
    assert_eq!(unavailable, PushError::Service("status 503".into()));
    service.next().await;

    service.script.lock().push_back((410, None));
    harness.emit(harness.agent("root"), failed());
    service.next().await;
    for _ in 0..200 {
        if !harness
            .notifier
            .config(Some(device))
            .expect("config")
            .subscribed
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the 410 did not remove the subscription");
}

#[tokio::test]
async fn retry_after_is_read_off_the_wire() {
    let mut service = FakePushService::start().await;
    let carrier = ToFakeService::new(service.address);
    let request = || PushRequest {
        endpoint: url::Url::parse(ENDPOINT).expect("a url"),
        ttl: 3600,
        urgency: Urgency::Normal,
        topic: "topic".into(),
        authorization: "vapid t=a.b.c, k=d".into(),
        body: vec![1, 2, 3],
    };
    service.script.lock().push_back((429, Some("120")));
    let limited = carrier.post(request()).await.expect("an answer");
    assert_eq!(
        limited,
        PushResponse {
            status: 429,
            retry_after: Some(Duration::from_secs(120))
        }
    );
    assert_eq!(service.next().await.body, [1, 2, 3]);

    let accepted = carrier.post(request()).await.expect("an answer");
    assert_eq!(
        accepted,
        PushResponse {
            status: 201,
            retry_after: None
        }
    );

    let closed = ToFakeService::new("127.0.0.1:1".parse().expect("an address"));
    let unreachable = closed
        .post(request())
        .await
        .expect_err("nothing listens there");
    assert!(
        matches!(&unreachable, TransportError::Unreachable(_)),
        "{unreachable:?}"
    );
    assert!(
        !unreachable.to_string().contains("abc123"),
        "the endpoint stays out of errors"
    );
}

#[derive(Clone, Default)]
struct CapturedLogs(Arc<std::sync::Mutex<Vec<u8>>>);

impl CapturedLogs {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).expect("logs are text")
    }
}

impl std::io::Write for CapturedLogs {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

// Process-wide: a thread-local subscriber races other test threads registering the same callsites.
fn capture_logs() -> CapturedLogs {
    let logs = CapturedLogs::default();
    let writer = logs.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(tracing::Level::DEBUG)
        .with_writer(move || writer.clone())
        .finish();
    tracing::subscriber::set_global_default(subscriber)
        .expect("the only subscriber in this binary");
    logs
}

#[tokio::test]
async fn failed_delivery_logs_and_errors_redact_endpoint_path_and_query() {
    let dir = tempfile::tempdir().unwrap();
    let logs = capture_logs();
    let (notifier, device) = refusing_push_notifier(&dir);
    let error = notifier.test(device).await.unwrap_err().to_string();
    let event = failed_delivery_event();
    notifier.observe(&event).unwrap();
    let stop = tokio_util::sync::CancellationToken::new();
    let running = notifier.clone();
    let stopping = stop.clone();
    let worker = tokio::spawn(async move { running.run(stopping).await });
    tokio::time::timeout(Duration::from_secs(5), async {
        while own_retry_line(&logs.text(), device).is_none() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("failed worker delivery was logged");
    stop.cancel();
    worker.await.unwrap();
    let logs = own_retry_line(&logs.text(), device).expect("this device's retry was logged");
    assert!(logs.contains("missing.invalid"), "{logs}");
    assert!(error.contains("missing.invalid"), "{error}");
    for printed in [&logs, &error] {
        assert!(!printed.contains("SECRET_PATH"), "{printed}");
        assert!(!printed.contains("SECRET_QUERY"), "{printed}");
    }
}

// Other tests log through the same subscriber, so only this device's line is this test's.
fn own_retry_line(logs: &str, device: aether_protocol::remote::DeviceId) -> Option<String> {
    let device = device.to_string();
    logs.lines()
        .find(|line| line.contains("push delivery will be retried") && line.contains(&device))
        .map(str::to_owned)
}

fn refusing_push_notifier(
    dir: &tempfile::TempDir,
) -> (crate::push::Notifier, aether_protocol::remote::DeviceId) {
    use crate::push::testing::{MemoryPushStore, StaticFleet, UserAgent};
    use crate::push::{HostPolicy, NetTransport, Notifier};
    let mut config = super::harness::config(dir, false);
    config.hosts = HostPolicy::new(&["missing.invalid".into()]).unwrap();
    let notifier = Notifier::new(
        config,
        Arc::new(MemoryPushStore::default()),
        Arc::new(StaticFleet::default()),
        Arc::new(NetTransport),
    );
    notifier.apply_settings(&settings(true)).unwrap();
    let device = aether_protocol::remote::DeviceId::new();
    let subscription = UserAgent::generate()
        .subscription("https://missing.invalid/SECRET_PATH?SECRET_QUERY=secret");
    notifier.subscribe(device, subscription, vec![]).unwrap();
    (notifier, device)
}

fn failed_delivery_event() -> aether_core::event::Event {
    aether_core::event::Event::from_draft(
        aether_core::event::EventDraft::new(
            aether_core::event::Actor::System,
            aether_core::event::EventKind::AgentFailed {
                error: "failure".into(),
            },
        ),
        aether_core::id::EventId::new(),
        1,
        aether_core::time::now(),
    )
}
