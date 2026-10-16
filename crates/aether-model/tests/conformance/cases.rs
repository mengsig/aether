use super::*;

#[tokio::test]
async fn anthropic_budget_validation_respects_the_effective_body() {
    use aether_core::model::ThinkingMode;
    use aether_model::adapters::anthropic::AnthropicClient;

    let a = under_test(Protocol::AnthropicMessages).unwrap();
    let server = FixtureServer::start(Canned::sse(fixture(&a, "text.sse"))).await;
    let config = AdapterConfig::new(server.base_url(), RequestAuth::None).with_capability(
        "conformance-model",
        CapabilityProfile {
            thinking: ThinkingSupport::Budget,
            ..profile()
        },
    );
    let client = AnthropicClient::new(config).unwrap();
    let mut req = request();
    req.thinking = Some(ThinkingMode::Budget { tokens: 1024 });
    let error = match client.stream(req.clone(), CancellationToken::new()).await {
        Err(error) => error,
        Ok(_) => panic!("an impossible budget must fail before sending"),
    };
    assert_eq!(error.class, ModelErrorClass::InvalidRequest);

    req.extensions
        .insert("anthropic.extra_body".into(), json!({"max_tokens": 2048}));
    let stream = client.stream(req, CancellationToken::new()).await.unwrap();
    assert_eq!(
        collect(stream, |_| {}).await.unwrap().text(),
        "Hello, world."
    );
    let body: Value = serde_json::from_str(&server.take_request().body).unwrap();
    assert_eq!(body["max_tokens"], json!(2048));
    assert_eq!(body["thinking"]["budget_tokens"], json!(1024));
}

#[tokio::test]
async fn anthropic_interleaved_budget_accepts_request_and_configured_betas() {
    use aether_core::model::ThinkingMode;
    use aether_model::adapters::anthropic::AnthropicClient;

    let a = under_test(Protocol::AnthropicMessages).unwrap();
    for source in ["request-string", "request-array", "configured"] {
        let server = FixtureServer::start(Canned::sse(fixture(&a, "text.sse"))).await;
        let mut config = AdapterConfig::new(server.base_url(), RequestAuth::None).with_capability(
            "conformance-model",
            CapabilityProfile {
                thinking: ThinkingSupport::Budget,
                ..profile()
            },
        );
        let mut req = request();
        req.thinking = Some(ThinkingMode::Budget { tokens: 1024 });
        req.max_output_tokens = Some(512);
        match source {
            "request-string" => {
                req.extensions.insert(
                    "anthropic.betas".into(),
                    json!("interleaved-thinking-2025-05-14"),
                );
            }
            "request-array" => {
                req.extensions.insert(
                    "anthropic.betas".into(),
                    json!(["other-beta", "interleaved-thinking-2025-05-14"]),
                );
            }
            "configured" => {
                config.headers.insert(
                    "Anthropic-Beta".into(),
                    "other-beta, interleaved-thinking-2025-05-14 ".into(),
                );
            }
            _ => unreachable!(),
        }
        let client = AnthropicClient::new(config).unwrap();
        let stream = client.stream(req, CancellationToken::new()).await.unwrap();
        assert_eq!(
            collect(stream, |_| {}).await.unwrap().text(),
            "Hello, world."
        );
        let recorded = server.take_request();
        assert!(
            recorded
                .header("anthropic-beta")
                .unwrap()
                .contains("interleaved-thinking-2025-05-14")
        );
        let body: Value = serde_json::from_str(&recorded.body).unwrap();
        assert_eq!(body["max_tokens"], json!(512));
        assert_eq!(body["thinking"]["budget_tokens"], json!(1024));
    }
}

#[tokio::test]
async fn every_adapter_has_a_conformance_entry_or_is_an_engine() {
    let adapters = every_adapter();
    assert_eq!(
        adapters.len(),
        Protocol::ALL.iter().filter(|p| !p.is_engine()).count(),
        "every non-engine protocol needs conformance fixtures"
    );
    assert!(adapters.len() >= 4, "VISION §17 asks for four adapters");
}

#[tokio::test]
async fn text_streams_as_one_answer() {
    for a in every_adapter() {
        let (outcome, body) = run(&a, Canned::sse(fixture(&a, "text.sse"))).await;
        let resp = ok(outcome, a.protocol);
        assert_eq!(resp.text(), "Hello, world.", "{}", a.protocol.as_str());
        assert_eq!(
            resp.stop_reason,
            Some(StopReason::EndTurn),
            "{}",
            a.protocol.as_str()
        );
        assert_eq!(
            body["model"],
            json!("conformance-model"),
            "{} must send the model id",
            a.protocol.as_str()
        );
        assert_eq!(
            body["stream"],
            json!(true),
            "{} must ask for a stream",
            a.protocol.as_str()
        );
        // The Responses adapters send `store: false` because Aether keeps the
        // transcript; an extension must not turn that server-side copy of
        // every turn back on. The other protocols do not set the key at all,
        // so it is not theirs to protect.
        if matches!(
            a.protocol,
            Protocol::OpenaiResponses | Protocol::OpenaiCodexResponses
        ) {
            assert_eq!(
                body["store"],
                json!(false),
                "{} let an extension re-enable the server-side copy",
                a.protocol.as_str()
            );
        }
    }
}

#[tokio::test]
async fn a_tool_call_arrives_assembled() {
    for a in every_adapter() {
        let (outcome, body) = run(&a, Canned::sse(fixture(&a, "tool_call.sse"))).await;
        let resp = ok(outcome, a.protocol);
        assert_eq!(resp.text(), "Let me look.", "{}", a.protocol.as_str());
        assert_eq!(
            resp.tool_uses().collect::<Vec<_>>(),
            vec![("call_abc", "get_weather", &json!({"location": "Paris"}))],
            "{}",
            a.protocol.as_str()
        );
        assert_eq!(
            resp.stop_reason,
            Some(StopReason::ToolUse),
            "{}",
            a.protocol.as_str()
        );
        let tools = body["tools"].as_array().unwrap_or_else(|| {
            panic!("{} must forward the tool spec", a.protocol.as_str());
        });
        assert_eq!(tools.len(), 1, "{}", a.protocol.as_str());
        assert!(
            tools[0].to_string().contains("get_weather"),
            "{} must name the tool",
            a.protocol.as_str()
        );
    }
}

#[tokio::test]
async fn usage_and_reasoning_normalize_to_the_same_numbers() {
    for a in every_adapter() {
        let (outcome, _) = run(&a, Canned::sse(fixture(&a, "usage.sse"))).await;
        let resp = ok(outcome, a.protocol);
        let label = a.protocol.as_str();
        assert_eq!(resp.text(), "Counted.", "{label}");
        assert_eq!(resp.usage.input_tokens, 20, "{label}: uncached input only");
        assert_eq!(resp.usage.cache_read_tokens, 10, "{label}");
        assert_eq!(resp.usage.output_tokens, 12, "{label}");
        assert_eq!(
            resp.usage.reasoning_tokens, a.reasoning_tokens,
            "{label} must normalize the reasoning-token count its protocol reports"
        );
        assert!(
            resp.content
                .iter()
                .any(|b| matches!(b, ContentBlock::Thinking { text, .. } if text == "weighing it")),
            "{label} must surface the model's reasoning"
        );
    }
}

#[tokio::test]
async fn a_refusal_is_a_refusal_everywhere() {
    for a in every_adapter() {
        let (outcome, _) = run(&a, Canned::sse(fixture(&a, "refusal.sse"))).await;
        let resp = ok(outcome, a.protocol);
        assert!(
            matches!(resp.stop_reason, Some(StopReason::Refusal { .. })),
            "{} refused but reported {:?}",
            a.protocol.as_str(),
            resp.stop_reason
        );
        assert_eq!(
            resp.text(),
            "I can't help with that.",
            "{} must keep the refusal text",
            a.protocol.as_str()
        );
    }
}

#[tokio::test]
async fn a_rate_limit_maps_to_one_error_class_with_its_retry_after() {
    for a in every_adapter() {
        let (outcome, _) = run(
            &a,
            Canned::error(429, fixture(&a, "rate_limit.json"), Some(2)),
        )
        .await;
        let err = outcome.err().unwrap_or_else(|| {
            panic!("{} must fail on a 429", a.protocol.as_str());
        });
        let label = a.protocol.as_str();
        assert_eq!(err.class, ModelErrorClass::RateLimit, "{label}");
        assert_eq!(err.status, Some(429), "{label}");
        assert_eq!(
            err.retry_after,
            Some(std::time::Duration::from_secs(2)),
            "{label} must carry the server's retry hint"
        );
        assert!(err.is_retryable(), "{label}");
    }
}

/// A server-side failure that arrives *inside* the stream is the same kind of
/// transient fault as a 500, so every adapter must class it retryably - the
/// turn's retry and its fallback chain both hang off `is_retryable`. None of
/// these fixtures carries an error `code`, so the classification has to come
/// from the error object's `type`.
#[tokio::test]
async fn an_in_stream_server_error_is_retryable_everywhere() {
    for a in every_adapter() {
        let (outcome, _) = run(&a, Canned::sse(fixture(&a, "stream_error.sse"))).await;
        let label = a.protocol.as_str();
        let err = outcome
            .err()
            .unwrap_or_else(|| panic!("{label} accepted a failed response as an answer"));
        assert!(
            matches!(
                err.class,
                ModelErrorClass::Server | ModelErrorClass::Overloaded
            ),
            "{label} classed a server-side stream failure as {:?}",
            err.class
        );
        assert!(
            err.is_retryable(),
            "{label} must let the turn retry and walk its chain"
        );
    }
}

/// VISION §10: events, snapshots and logs carry credential labels, never
/// secrets. OpenAI's 401 body quotes the key it rejected, so the adapter has
/// to scrub before the error escapes it - every durable surface downstream
/// (the invocation row, `invocation.failed`, the timeline, the log) is built
/// from exactly these fields.
#[tokio::test]
async fn a_rejected_key_never_survives_into_the_error() {
    for a in every_adapter() {
        let body = fixture(&a, "bad_key.json");
        let planted = "sk-ant-api03-NOTAREALKEYbutkeyshaped0123456789";
        let planted = if body.contains(planted) {
            planted
        } else {
            "sk-proj-NOTAREALKEYbutkeyshaped0123456789"
        };
        assert!(
            body.contains(planted),
            "the fixture must actually plant a key"
        );
        let (outcome, _) = run(&a, Canned::error(401, body, None)).await;
        let label = a.protocol.as_str();
        let err = outcome
            .err()
            .unwrap_or_else(|| panic!("{label} must fail on a 401"));
        assert_eq!(err.class, ModelErrorClass::Auth, "{label}");
        for (what, text) in [
            ("message", err.message.clone()),
            (
                "provider code",
                err.provider_code.clone().unwrap_or_default(),
            ),
            // Display is what record_failure writes to the invocation row and
            // to the invocation.failed event.
            ("display", err.to_string()),
        ] {
            assert!(
                !text.contains(planted),
                "{label} leaked the rejected key into its {what}: {text}"
            );
        }
        assert!(
            err.message.contains("REDACTED"),
            "{label} must say the secret was masked, not silently drop it: {}",
            err.message
        );
    }
}

#[tokio::test]
async fn a_truncated_stream_fails_visibly_instead_of_hanging() {
    for a in every_adapter() {
        let (outcome, _) = run(&a, Canned::sse(fixture(&a, "malformed_stream.sse"))).await;
        let err = outcome.err().unwrap_or_else(|| {
            panic!(
                "{} accepted a malformed stream as a complete answer",
                a.protocol.as_str()
            );
        });
        assert_ne!(
            err.class,
            ModelErrorClass::Cancelled,
            "{} must report the malformed stream, not a cancellation",
            a.protocol.as_str()
        );
    }
}

/// A stream that stops at a clean event boundary, before its protocol's
/// terminal event, is an interrupted turn - not a short but finished answer.
/// The adapters used to disagree here (two fabricated `EndTurn`, one reported
/// no stop reason at all), which is exactly what this suite exists to settle:
/// a retryable error, so the turn retries or walks its chain, and whatever
/// did arrive is kept for it.
#[tokio::test]
async fn a_stream_cut_at_an_event_boundary_is_interrupted_everywhere() {
    for a in every_adapter() {
        let server = FixtureServer::start(Canned::sse(fixture(&a, "interrupted_stream.sse"))).await;
        let client = client_for(&a, &server.base_url());
        let label = a.protocol.as_str();
        let stream = client
            .stream(request(), CancellationToken::new())
            .await
            .unwrap_or_else(|e| panic!("{label} must open the stream: {e}"));
        let failure = aether_model::client::collect_partial(stream, |_| {})
            .await
            .err()
            .unwrap_or_else(|| panic!("{label} reported a cut stream as a finished turn"));
        assert!(
            failure.error.is_retryable(),
            "{label} reported a cut stream as {:?}, which never retries",
            failure.error.class
        );
        assert_eq!(
            failure.partial.text(),
            "Hello, ",
            "{label} must keep what did arrive"
        );
        assert_eq!(
            failure.partial.stop_reason, None,
            "{label} must not invent a stop reason for an unfinished answer"
        );
    }
}

#[tokio::test]
async fn a_cancelled_request_ends_the_stream_everywhere() {
    for a in every_adapter() {
        let server = FixtureServer::start(Canned::sse(fixture(&a, "text.sse"))).await;
        let client = client_for(&a, &server.base_url());
        let cancel = CancellationToken::new();
        cancel.cancel();
        let outcome = match client.stream(request(), cancel).await {
            Ok(stream) => collect(stream, |_| {}).await,
            Err(e) => Err(e),
        };
        let err = outcome.err().unwrap_or_else(|| {
            panic!("{} ignored cancellation", a.protocol.as_str());
        });
        assert_eq!(
            err.class,
            ModelErrorClass::Cancelled,
            "{}",
            a.protocol.as_str()
        );
    }
}

/// The suite drives adapters directly; this pins that the registry hands the
/// runtime the same adapter for a provider that declares the protocol.
#[tokio::test]
async fn the_registry_routes_every_protocol_to_its_adapter() {
    use aether_model::Models;
    use aether_model::auth::AuthMethods;
    use aether_model::auth::{CredentialStore, MemoryCredentialStore};
    use aether_model::registry::{ProviderDef, Registry};

    let mut registry = Registry::empty();
    for a in every_adapter() {
        registry.insert(ProviderDef {
            id: a.protocol.as_str().to_owned(),
            name: a.protocol.as_str().to_owned(),
            protocol: a.protocol,
            base_url: "http://127.0.0.1:1/v1".into(),
            auth: AuthMethods::api_key(["CONFORMANCE_KEY"]),
            headers: Default::default(),
            models: Vec::new(),
            default_capabilities: None,
            flags: Default::default(),
            max_concurrent: None,
            family: None,
            enabled: true,
            usage_probe: Default::default(),
        });
    }
    let store: Arc<dyn CredentialStore> = Arc::new(MemoryCredentialStore::new());
    let models = Models::new(
        registry,
        Default::default(),
        store,
        std::path::PathBuf::from("/nonexistent"),
    )
    .expect("models")
    .with_env(|k| (k == "CONFORMANCE_KEY").then(|| CREDENTIAL.to_owned()));

    for a in every_adapter() {
        let client = models
            .client(&ModelRef::new(a.protocol.as_str(), "conformance-model"))
            .await
            .unwrap_or_else(|e| panic!("{}: {e}", a.protocol.as_str()));
        assert_eq!(
            client.adapter(),
            a.adapter,
            "a provider declaring {} must get its own adapter",
            a.protocol.as_str()
        );
    }
}
