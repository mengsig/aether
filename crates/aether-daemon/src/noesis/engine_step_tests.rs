use std::sync::Arc;

use aether_core::model::{
    ContentBlock, Message, MessageRole, ModelErrorClass, ModelRef, ModelRequest, SystemBlock,
    ToolResultBlock, ToolSpec,
};
use aether_model::testing::ScriptedClient;
use aether_model::{ModelClient, collect_partial};
use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::*;

fn request() -> ModelRequest {
    let history = vec![
        Message::user_text("Teach me limits"),
        Message {
            role: MessageRole::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "call_1".into(),
                name: "noesis_ask".into(),
                input: json!({"prompt": "2+2?", "answer_key": "<sealed>"}),
            }],
        },
        Message {
            role: MessageRole::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "call_1".into(),
                content: vec![ToolResultBlock::Text {
                    text: "{\"correct\":true}".into(),
                }],
                is_error: false,
            }],
        },
    ];
    let mut request = ModelRequest::new(ModelRef::new("claude-code", "haiku"), history);
    request.system = vec![SystemBlock {
        text: "You are the Prober.".into(),
        cache: true,
    }];
    request.tools = vec![ToolSpec {
        name: "noesis_ask".into(),
        description: "Pose one question.".into(),
        input_schema: json!({"type": "object", "properties": {"prompt": {"type": "string"}}}),
        strict: false,
    }];
    request
}

async fn step(replies: &[&str]) -> (Arc<ScriptedClient>, Result<ModelResponse, String>) {
    let inner = Arc::new(ScriptedClient::new());
    for reply in replies {
        inner.push(ScriptedClient::text(reply));
    }
    let client = EngineStepClient::new(inner.clone());
    let stream = client
        .stream(request(), CancellationToken::new())
        .await
        .expect("a stream");
    let reply = collect_partial(stream, |_| {})
        .await
        .map_err(|failure| failure.error.message.clone());
    (inner, reply)
}

use aether_core::model::ModelResponse;

#[tokio::test]
async fn the_engine_reads_tools_and_transcript_as_text_and_gets_no_native_tool() {
    let (inner, reply) = step(&[r#"{"text": "Next one.", "tool_calls": []}"#]).await;
    assert_eq!(reply.expect("parsed").text(), "Next one.");
    let sent = inner.requests();
    assert_eq!(sent.len(), 1);
    assert!(
        sent[0].tools.is_empty(),
        "no native tool reaches the engine"
    );
    let system = &sent[0].system[0].text;
    assert!(system.starts_with("You are the Prober."), "{system}");
    assert!(system.contains("### noesis_ask") && system.contains("Pose one question."));
    let prompt = sent[0].messages[0].text();
    assert!(prompt.contains("Teach me limits"), "{prompt}");
    assert!(prompt.contains("\"answer_key\":\"<sealed>\""), "{prompt}");
    assert!(prompt.contains("- call_1: {\"correct\":true}"), "{prompt}");
}

#[tokio::test]
async fn a_json_reply_becomes_text_and_tool_calls_in_order() {
    let raw = "```json\n{\"text\": \"First:\", \"tool_calls\": [\
               {\"name\": \"noesis_ask\", \"input\": {\"prompt\": \"a\"}},\
               {\"name\": \"noesis_probe_done\", \"input\": {\"strands\": []}}]}\n```";
    let (_, reply) = step(&[raw]).await;
    let reply = reply.expect("parsed");
    assert_eq!(reply.text(), "First:");
    let calls: Vec<(&str, &str)> = reply
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolUse { id, name, .. } => Some((id.as_str(), name.as_str())),
            _ => None,
        })
        .collect();
    let names: Vec<&str> = calls.iter().map(|(_, name)| *name).collect();
    assert_eq!(names, vec!["noesis_ask", "noesis_probe_done"]);
    assert_ne!(calls[0].0, calls[1].0, "ids are unique");
    let (_, again) = step(&[raw]).await;
    let again = again.expect("parsed");
    assert!(
        again.content.iter().all(|block| match block {
            ContentBlock::ToolUse { id, .. } => id != calls[0].0 && id != calls[1].0,
            _ => true,
        }),
        "a later reply's ids differ from an earlier one's"
    );
}

#[tokio::test]
async fn a_malformed_reply_fails_closed_with_its_usage_and_never_quotes_itself() {
    let secret = "SEALED-KEY-77";
    let raw = format!(
        "Here you go: {{\"text\": \"Q\", \"tool_calls\": [{{\"name\": \"noesis_ask\", \
         \"input\": {{\"answer_key\": \"{secret}\"}},}}]}}"
    );
    let inner = Arc::new(ScriptedClient::new());
    inner.push(ScriptedClient::text(&raw));
    let client = EngineStepClient::new(inner.clone());
    let stream = client
        .stream(request(), CancellationToken::new())
        .await
        .expect("a stream");
    let failure = collect_partial(stream, |_| {}).await.expect_err("refused");
    assert!(is_malformed(&failure.error), "{:?}", failure.error);
    assert!(
        !failure.error.message.contains(secret),
        "{}",
        failure.error.message
    );
    assert!(
        failure.partial.text().is_empty(),
        "no text of a refused reply streams"
    );
    assert!(
        failure.partial.usage.total_tokens() > 0,
        "the attempt's usage is reported"
    );
    assert_eq!(
        inner.requests().len(),
        1,
        "the step, not the client, retries"
    );
    let note = retry_note().text();
    assert!(!note.contains(secret) && note.contains("refused"), "{note}");
}

#[tokio::test]
async fn tool_like_prose_fails_closed_and_streams_nothing() {
    let secret = "SEALED-KEY-88";
    let raw = format!("noesis_ask(answer_key=\"{secret}\")");
    let inner = Arc::new(ScriptedClient::new());
    inner.push(ScriptedClient::text(&raw));
    let client = EngineStepClient::new(inner.clone());
    let stream = client
        .stream(request(), CancellationToken::new())
        .await
        .expect("a stream");
    let mut deltas = String::new();
    let failure = collect_partial(stream, |event| {
        if let StreamEvent::TextDelta { text, .. } = event {
            deltas.push_str(text);
        }
    })
    .await
    .expect_err("refused");
    assert!(is_malformed(&failure.error), "{:?}", failure.error);
    assert!(!failure.error.message.contains(secret));
    assert!(deltas.is_empty() && failure.partial.text().is_empty());
    assert!(failure.partial.usage.total_tokens() > 0, "usage is kept");
    assert!(!retry_note().text().contains(secret));
}

#[tokio::test]
async fn a_refused_first_call_is_not_sent_and_says_why() {
    let inner = Arc::new(ScriptedClient::new());
    inner.push(ScriptedClient::error(
        ModelErrorClass::Auth,
        "not signed in",
    ));
    let client = EngineStepClient::new(inner);
    let stream = client.stream(request(), CancellationToken::new()).await;
    match stream {
        Err(error) => assert!(error.message.contains("not signed in")),
        Ok(stream) => {
            let failure = collect_partial(stream, |_| {}).await.expect_err("failed");
            assert!(failure.error.message.contains("not signed in"));
        }
    }
}

#[test]
fn plain_prose_and_empty_objects_are_refused() {
    assert!(
        parse_reply("Well done, that is right.").is_err(),
        "bare prose"
    );
    assert!(parse_reply("   ").is_err());
    assert!(parse_reply("{}").is_err(), "neither text nor a call");
    assert!(parse_reply(r#"{"tool_calls": [{"name": " "}]}"#).is_err());
    assert!(parse_reply(r#"{"text": "x", "extra": 1}"#).is_err());
    assert!(
        parse_reply("{bad").is_err(),
        "an object that does not close"
    );
    assert!(
        parse_reply("The set {1, 2} has two members.").is_err(),
        "a brace that is no reply object fails closed"
    );
    assert!(
        parse_reply("Note: {\"text\": \"Hi\",}").is_err(),
        "a trailing comma"
    );
    let embedded = parse_reply("Here: {\"text\": \"Hi\"}").expect("embedded");
    assert_eq!(embedded.text.as_deref(), Some("Hi"));
    assert!(parse_reply(r#"{"tool_calls": [{"name": "x", "input": 3}]}"#).is_err());
}

#[tokio::test]
async fn malformed_streamed_text_keeps_final_usage_and_never_emits_tool_json() {
    let inner = Arc::new(ScriptedClient::new());
    inner.push(ScriptedClient::text(
        r#"{"text":"Safe\xPRIVATE","tool_calls":[]}"#,
    ));
    let client = EngineStepClient::new(inner);
    let stream = client
        .stream(request(), CancellationToken::new())
        .await
        .unwrap();
    let failure = collect_partial(stream, |_| {})
        .await
        .expect_err("invalid escape");
    assert!(is_malformed(&failure.error));
    assert!(
        failure.partial.usage.total_tokens() > 0,
        "read the final receipt even after malformed JSON"
    );
    assert!(!failure.error.message.contains("PRIVATE"));
    assert!(!failure.partial.text().contains("PRIVATE"));
}

#[tokio::test]
async fn a_noncanonical_field_order_keeps_private_text_out_of_deltas() {
    let raw = r#"{"tool_calls":[{"name":"noesis_problem","input":{"solution":"PRIVATE","text":"NESTED"}}],"text":"Public"}"#;
    let (_, reply) = step(&[raw]).await;
    let reply = reply.expect("validated fallback");
    assert_eq!(reply.text(), "Public");
    let (_, name, input) = reply.tool_uses().next().expect("tool");
    assert_eq!(name, "noesis_problem");
    assert_eq!(input["solution"], "PRIVATE");
}
