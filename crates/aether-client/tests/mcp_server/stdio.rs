use super::*;

#[tokio::test]
async fn a_client_discovers_the_surface_and_a_call_becomes_a_protocol_command() {
    let sink = Arc::new(RecordingSink::default());
    let client = client_for(sink.clone()).await;

    let discovered = client.list_tools().await.expect("tools");
    assert_eq!(discovered.capped, None, "the surface fits every cap");
    let names: Vec<&str> = discovered.tools.iter().map(|t| t.name.as_str()).collect();
    for expected in ["status", "list_agents", "create_agent", "send_message"] {
        assert!(
            names.contains(&expected),
            "{expected} missing from {names:?}"
        );
    }

    let result = client.call_tool("status", json!({})).await.expect("status");
    assert!(!result.is_error, "{result:?}");
    assert!(
        result.text.contains("\"agents_live\": 2"),
        "{}",
        result.text
    );
    assert_eq!(sink.seen(), vec![Command::Status]);

    let agent = aether_core::id::AgentId::new();
    let result = client
        .call_tool(
            "send_message",
            json!({"agent": agent.to_string(), "content": "go"}),
        )
        .await
        .expect("send_message");
    assert!(!result.is_error, "{result:?}");
    assert_eq!(
        sink.seen()[1],
        Command::SendMessage {
            to: agent,
            content: "go".into(),
            subtree: false,
            attachments: Vec::new(),
            idempotency_key: None,
        }
    );
}

#[tokio::test]
async fn a_bad_call_is_an_error_result_the_caller_can_read() {
    let sink = Arc::new(RecordingSink::default());
    let client = client_for(sink.clone()).await;

    let result = client
        .call_tool("get_agent", json!({"id": "not-an-id"}))
        .await
        .expect("a refused call is still a result");
    assert!(result.is_error);
    assert!(result.text.contains("not-an-id"), "{}", result.text);

    let result = client
        .call_tool("no_such_tool", json!({}))
        .await
        .expect("result");
    assert!(result.is_error);
    assert!(result.text.contains("unknown tool"), "{}", result.text);
    assert!(
        sink.seen().is_empty(),
        "a call that cannot be built never reaches the daemon"
    );
}

#[tokio::test]
async fn the_stdio_server_refuses_a_line_past_its_cap() {
    // Every other read from a peer in this branch is bounded; a peer that
    // never sends a newline must not be able to grow the process.
    let sink = Arc::new(RecordingSink::default());
    let (ours, theirs) = tokio::io::duplex(64 * 1024);
    let (server_reader, server_writer) = tokio::io::split(theirs);
    let serving = tokio::spawn(async move {
        aether_client::mcp::stdio::serve(
            &*sink,
            tokio::io::BufReader::new(server_reader),
            server_writer,
        )
        .await
    });

    let (_reader, mut writer) = tokio::io::split(ours);
    let flood = vec![b'x'; aether_client::mcp::stdio::MAX_LINE_BYTES + 1];
    // The peer may go away as the server gives up, so the write itself is
    // allowed to fail; the server's verdict is what is asserted.
    let _ = tokio::io::AsyncWriteExt::write_all(&mut writer, &flood).await;
    let _ = tokio::io::AsyncWriteExt::flush(&mut writer).await;

    let e = tokio::time::timeout(std::time::Duration::from_secs(10), serving)
        .await
        .expect("the server must give up rather than keep reading")
        .expect("join")
        .expect_err("an unbounded line must not be accepted");
    assert!(e.to_string().contains("cap"), "{e}");
}

#[tokio::test]
async fn initialization_only_advertises_supported_versions() {
    let sink = RecordingSink::default();
    for (requested, expected) in [
        (json!("2024-11-05"), "2024-11-05"),
        (json!("2025-03-26"), "2025-03-26"),
        (json!("2025-06-18"), "2025-06-18"),
        (json!("2025-11-25"), "2025-11-25"),
        (json!("2099-01-01"), "2025-06-18"),
        (json!(42), "2025-06-18"),
        (Value::Null, "2025-06-18"),
    ] {
        let answer = aether_client::mcp::handle(&sink, &aether_client::mcp::Surface::Fleet,
            &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": requested}})).await;
        let aether_client::mcp::Answer::Reply(reply) = answer else {
            panic!("expected reply");
        };
        assert_eq!(reply["result"]["protocolVersion"], expected, "{requested}");
    }
    assert!(sink.seen().is_empty());
}

#[tokio::test]
async fn create_agent_forwards_an_explicit_child_effort() {
    let sink = Arc::new(RecordingSink::default());
    let client = client_for(sink.clone()).await;
    let parent = aether_core::id::AgentId::new();
    client
        .call_tool(
            "create_agent",
            json!({
                "role": "worker", "parent": parent.to_string(), "effort": "low", "start": false
            }),
        )
        .await
        .expect("MCP result");
    let commands = sink.seen();
    let Command::CreateAgent(request) = &commands[0] else {
        panic!("expected creation");
    };
    assert_eq!(request.parent, Some(parent));
    assert_eq!(
        request.config.as_ref().unwrap().effort,
        Some(aether_core::model::Effort::Low)
    );
    let result = client
        .call_tool(
            "create_agent",
            json!({
                "role": "worker", "parent": parent.to_string(), "effort": "invalid"
            }),
        )
        .await
        .expect("MCP result");
    assert!(result.is_error);
    assert_eq!(sink.seen().len(), 1);
}

/// Send `lines` to a stdio server and return every reply it wrote.
async fn stdio_replies(lines: &[&str]) -> Vec<Value> {
    let sink = Arc::new(RecordingSink::default());
    let input = format!("{}\n", lines.join("\n"));
    let mut output = Vec::new();
    aether_client::mcp::stdio::serve(
        &*sink,
        tokio::io::BufReader::new(input.as_bytes()),
        &mut output,
    )
    .await
    .expect("serve");
    assert!(
        sink.seen().is_empty(),
        "protocol errors never reach the daemon"
    );
    String::from_utf8(output)
        .expect("utf8")
        .lines()
        .map(|line| serde_json::from_str(line).expect("each reply is one JSON line"))
        .collect()
}

const INITIALIZE: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;

#[tokio::test]
async fn a_malformed_line_is_answered_with_a_parse_error() {
    let replies = stdio_replies(&["{not json", INITIALIZE]).await;
    assert_eq!(replies[0]["error"]["code"], -32700);
    assert_eq!(replies[0]["id"], Value::Null);
    assert_eq!(replies[1]["id"], 1, "the server keeps serving afterwards");
}

#[tokio::test]
async fn a_batch_is_refused_as_an_invalid_request() {
    let batch = r#"[{"jsonrpc":"2.0","id":1,"method":"ping"}]"#;
    let replies = stdio_replies(&[batch]).await;
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0]["error"]["code"], -32600);
    assert_eq!(replies[0]["id"], Value::Null);
}

#[tokio::test]
async fn requests_before_initialize_are_refused_but_ping_and_notifications_are_not() {
    let early = r#"{"jsonrpc":"2.0","id":7,"method":"tools/list"}"#;
    let ping = r#"{"jsonrpc":"2.0","id":8,"method":"ping"}"#;
    let note = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
    let listed = r#"{"jsonrpc":"2.0","id":9,"method":"tools/list"}"#;
    let replies = stdio_replies(&[early, ping, note, INITIALIZE, listed]).await;
    assert_eq!(replies.len(), 4, "the notification takes no reply");
    assert_eq!(replies[0]["error"]["code"], -32002);
    assert_eq!(replies[0]["id"], 7);
    assert_eq!(replies[1]["result"], json!({}));
    assert!(replies[3]["result"]["tools"].is_array(), "{}", replies[3]);
}
