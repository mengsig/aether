#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! One conformance suite over every protocol adapter.
//!
//! Each adapter is driven over real HTTP against a fixture server replaying a
//! recorded stream, and every adapter is held to the same table of outcomes.
//! `under_test` matches exhaustively on [`Protocol`], so a new protocol cannot
//! land until someone declares whether it is an engine or supplies fixtures.

mod support;

use std::sync::Arc;

use aether_core::model::{
    CapabilityProfile, ContentBlock, Message, ModelError, ModelErrorClass, ModelRef, ModelRequest,
    ModelResponse, StopReason, ThinkingSupport, ToolSpec,
};
use aether_model::adapters::openai_responses::Dialect;
use aether_model::adapters::{AdapterConfig, RequestAuth};
use aether_model::client::{ModelClient, collect};
use aether_model::http::RetryPolicy;
use aether_model::registry::Protocol;
use serde_json::{Value, json};
use support::{Canned, FixtureServer};
use tokio_util::sync::CancellationToken;

const CREDENTIAL: &str = "test-credential-not-a-real-key";

/// Body fields a protocol must send on every request, whatever the fixture,
/// with the value each must carry.
type RequiredBody = &'static [(&'static str, fn() -> Value)];

/// One adapter and the wire facts the suite pins for it.
struct AdapterUnderTest {
    protocol: Protocol,
    /// What `ModelClient::adapter()` must report.
    adapter: &'static str,
    /// Fixture directory under `tests/fixtures/conformance`.
    fixtures: &'static str,
    /// Path the adapter must POST to, relative to the provider base URL.
    path: &'static str,
    /// Header the resolved credential must travel in.
    auth_header: &'static str,
    /// Reasoning tokens the `usage.sse` fixture reports, normalized. `None`
    /// where the protocol has no such field to report.
    reasoning_tokens: Option<u64>,
    /// Body fields this protocol must send, whatever the fixture. The two
    /// Responses dialects share one recorded stream, so this is the only
    /// place the narrower Codex request - the dialect's whole reason to
    /// exist - is actually checked on the wire.
    required_body: RequiredBody,
}

/// The adapter for a protocol, or `None` for an engine the runtime drives
/// through a CLI rather than an HTTP adapter.
fn under_test(protocol: Protocol) -> Option<AdapterUnderTest> {
    fn empty_array() -> Value {
        json!([])
    }
    fn no_store() -> Value {
        json!(false)
    }
    let facts: (&str, &str, &str, &str, Option<u64>, RequiredBody) = match protocol {
        // The Messages API reports no reasoning-token count.
        Protocol::AnthropicMessages => (
            "anthropic-messages/1",
            "anthropic-messages",
            "/v1/messages",
            "x-api-key",
            None,
            &[][..],
        ),
        Protocol::OpenaiChatCompletions => (
            "openai-chat-completions/1",
            "openai-chat-completions",
            "/chat/completions",
            "authorization",
            Some(4),
            &[][..],
        ),
        // Both Responses dialects speak one wire protocol, so they share the
        // recorded stream; only the request body differs.
        Protocol::OpenaiResponses => (
            "openai-responses/1",
            "openai-responses",
            "/responses",
            "authorization",
            Some(4),
            &[("store", no_store as fn() -> Value)][..],
        ),
        // The Codex backend answers only for its own account scope and
        // rejects a stored response; `include: []` is the narrowing.
        Protocol::OpenaiCodexResponses => (
            "openai-codex-responses/1",
            "openai-responses",
            "/responses",
            "authorization",
            Some(4),
            &[
                ("store", no_store as fn() -> Value),
                ("include", empty_array as fn() -> Value),
            ][..],
        ),
        Protocol::ClaudeCode | Protocol::Codex | Protocol::Antigravity => return None,
    };
    let (adapter, fixtures, path, auth_header, reasoning_tokens, required_body) = facts;
    Some(AdapterUnderTest {
        protocol,
        adapter,
        fixtures,
        path,
        auth_header,
        reasoning_tokens,
        required_body,
    })
}

fn client_for(a: &AdapterUnderTest, base_url: &str) -> Arc<dyn ModelClient> {
    let auth = match a.protocol {
        Protocol::AnthropicMessages => RequestAuth::ApiKey(CREDENTIAL.into()),
        _ => RequestAuth::Bearer(CREDENTIAL.into()),
    };
    // A provider definition may add headers; replacing the credential the
    // runtime resolved is not one of the things it may do. Every adapter
    // carries this, so every fixture run also proves the credential survived.
    let mut headers = std::collections::BTreeMap::new();
    headers.insert(
        "authorization".to_owned(),
        "Bearer not-the-credential".to_owned(),
    );
    headers.insert("x-api-key".to_owned(), "not-the-credential".to_owned());
    let config = AdapterConfig {
        // No retry: the suite pins how a failure is mapped, not how often it
        // is re-sent. Retry has its own tests.
        retry: RetryPolicy::none(),
        headers,
        ..AdapterConfig::new(base_url, auth)
    }
    .with_capability("conformance-model", profile());
    match a.protocol {
        Protocol::AnthropicMessages => Arc::new(
            aether_model::adapters::anthropic::AnthropicClient::new(config).expect("client"),
        ),
        Protocol::OpenaiChatCompletions => Arc::new(
            aether_model::adapters::openai_chat::OpenAiChatClient::new(config).expect("client"),
        ),
        Protocol::OpenaiResponses => Arc::new(
            aether_model::adapters::openai_responses::OpenAiResponsesClient::new(
                config,
                Dialect::OpenAi,
            )
            .expect("client"),
        ),
        Protocol::OpenaiCodexResponses => Arc::new(
            aether_model::adapters::openai_responses::OpenAiResponsesClient::new(
                config,
                Dialect::Codex,
            )
            .expect("client"),
        ),
        Protocol::ClaudeCode | Protocol::Codex | Protocol::Antigravity => {
            unreachable!("engines have no adapter")
        }
    }
}

fn profile() -> CapabilityProfile {
    CapabilityProfile {
        context_window: 32_768,
        max_output_tokens: 4_096,
        tools: true,
        vision: true,
        thinking: ThinkingSupport::Adaptive,
        effort: true,
        ..Default::default()
    }
}

fn request() -> ModelRequest {
    let mut req = ModelRequest::new(
        ModelRef::new("conformance", "conformance-model"),
        vec![Message::user_text("what is the weather in Paris?")],
    );
    req.tools = vec![ToolSpec {
        name: "get_weather".into(),
        description: "Look up the weather".into(),
        input_schema: json!({"type": "object", "properties": {"location": {"type": "string"}}}),
        strict: false,
    }];
    req.max_output_tokens = Some(1024);
    // Every fixture run carries an extension that tries to take over the
    // adapter's own keys, so `stream` and `model` - asserted below - and
    // `store` double as the proof it cannot.
    req.extensions.insert(
        "openai.extra_body".into(),
        json!({"model": "hijacked", "stream": false, "store": true, "messages": []}),
    );
    req.extensions.insert(
        "anthropic.extra_body".into(),
        json!({"model": "hijacked", "stream": false, "messages": []}),
    );
    req
}

fn fixture(a: &AdapterUnderTest, name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/conformance")
        .join(a.fixtures)
        .join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("missing conformance fixture {}: {e}", path.display()))
}

/// Replay one fixture through the adapter's real HTTP path.
async fn run(a: &AdapterUnderTest, canned: Canned) -> (Result<ModelResponse, ModelError>, Value) {
    let server = FixtureServer::start(canned).await;
    let client = client_for(a, &server.base_url());
    assert_eq!(client.adapter(), a.adapter, "{}", a.protocol.as_str());
    let outcome = match client.stream(request(), CancellationToken::new()).await {
        Ok(stream) => collect(stream, |_| {}).await,
        Err(e) => Err(e),
    };
    let recorded = server.take_request();
    assert_eq!(
        recorded.path,
        a.path,
        "{} must post to its own endpoint",
        a.protocol.as_str()
    );
    assert!(
        recorded
            .header(a.auth_header)
            .is_some_and(|v| v.contains(CREDENTIAL)),
        "{} let a configured header shadow the resolved credential in {}",
        a.protocol.as_str(),
        a.auth_header
    );
    let body: Value = serde_json::from_str(&recorded.body).expect("request body is JSON");
    for (field, expected) in a.required_body {
        assert_eq!(
            body.get(*field),
            Some(&expected()),
            "{} must send {field} on every request",
            a.protocol.as_str()
        );
    }
    // The complementary half: in exactly one header, and nowhere else. A key
    // in a query string or a body field would otherwise pass unnoticed.
    let carrying: Vec<&str> = recorded
        .headers
        .iter()
        .filter(|(_, v)| v.contains(CREDENTIAL))
        .map(|(k, _)| k.as_str())
        .collect();
    assert_eq!(
        carrying,
        vec![a.auth_header],
        "{} must carry the credential in one header and no other",
        a.protocol.as_str()
    );
    assert!(
        !recorded.body.contains(CREDENTIAL) && !recorded.path.contains(CREDENTIAL),
        "{} put the credential in the request body or path",
        a.protocol.as_str()
    );
    (outcome, body)
}

fn ok(outcome: Result<ModelResponse, ModelError>, protocol: Protocol) -> ModelResponse {
    outcome.unwrap_or_else(|e| panic!("{} must stream cleanly: {e}", protocol.as_str()))
}

fn every_adapter() -> Vec<AdapterUnderTest> {
    Protocol::ALL
        .iter()
        .copied()
        .filter_map(under_test)
        .collect()
}

#[path = "conformance/cases.rs"]
mod cases;
