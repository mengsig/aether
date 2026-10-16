use super::*;

#[test]
fn the_mcp_timeout_stays_under_the_tool_runtimes_own() {
    // The six lines above DEFAULT_TIMEOUT explain why this ordering
    // matters; the two live in different modules, so it is pinned here.
    assert!(
        DEFAULT_TIMEOUT < crate::runtime::ExecutionPolicy::default().default_timeout,
        "the manager must fail a call before the runtime drops its future"
    );
}

#[test]
fn one_servers_colliding_tool_names_are_refused_at_discovery() {
    // `read.file` and `read_file` both sanitize to `mcp__s__read_file`.
    // Advertised, one shadows the other and `get()` resolves to whichever
    // came first, leaving the second unreachable with nothing said.
    let spec = McpServerSpec {
        name: "s".into(),
        transport: McpTransportSpec::Http {
            url: "http://127.0.0.1".into(),
            token: None,
        },
        classes: BTreeMap::new(),
        send_caller_meta: true,
        auth: None,
    };
    let of = |name: &str| {
        discovered(
            &spec,
            RemoteTool {
                name: name.to_owned(),
                description: String::new(),
                input_schema: serde_json::json!({}),
                annotations: serde_json::Value::Null,
            },
        )
    };

    assert!(first_collision(&[of("read_file"), of("write_file")]).is_none());
    let e = first_collision(&[of("read.file"), of("read_file")])
        .expect("a name Aether cannot tell apart is refused");
    assert!(e.to_string().contains("mcp__s__read_file"), "{e}");
}

#[test]
fn an_undeclared_tool_is_classed_conservatively() {
    let mut spec = McpServerSpec {
        name: "s".into(),
        transport: McpTransportSpec::Http {
            url: "http://x".into(),
            token: None,
        },
        classes: BTreeMap::new(),
        send_caller_meta: true,
        auth: None,
    };
    assert_eq!(
        classify(&spec, "delete_everything"),
        (SideEffectClass::External, false)
    );
    assert!(
        aether_core::agent::ApprovalPolicy::default().requires(SideEffectClass::External),
        "the conservative class must be one the default policy gates"
    );
    spec.classes
        .insert("delete_everything".to_owned(), SideEffectClass::ReadOnly);
    assert_eq!(
        classify(&spec, "delete_everything"),
        (SideEffectClass::ReadOnly, true)
    );
}

#[test]
fn a_token_is_never_in_the_description() {
    let transport = McpTransportSpec::Http {
        url: "https://example.com/mcp".into(),
        token: Some("secret-value".into()),
    };
    let text = transport.describe();
    assert!(!text.contains("secret-value"), "{text}");
    assert!(text.contains("(token)"));
}

#[test]
fn only_aether_keys_where_a_server_might_read_a_caller_are_refused() {
    for spoof in [
        serde_json::json!({"aether/agent_id": "agt_x"}),
        serde_json::json!({"Aether/Agent_Id": "agt_x"}),
        serde_json::json!({"_meta": {"aether/task_id": "tsk_x"}}),
    ] {
        let e = refuse_spoofed_caller(&spoof).expect_err("a caller claim");
        assert!(matches!(e, McpError::InvalidArguments(_)), "{spoof}: {e}");
        assert!(
            !e.is_fatal(),
            "a refused call says nothing about the server"
        );
    }
    for fine in [
        serde_json::json!(null),
        serde_json::json!(["aether/agent_id"]),
        serde_json::json!({"aether": 1, "aeth": 2, "é/aether/x": 3, "ééééé": 4}),
        serde_json::json!({"_meta": "aether/agent_id"}),
        serde_json::json!({"_meta": {"progressToken": 1}}),
        serde_json::json!({"nested": {"aether/agent_id": "agt_x"}}),
    ] {
        assert!(refuse_spoofed_caller(&fine).is_ok(), "{fine}");
    }
}

#[test]
fn a_callers_meta_names_its_agent_and_any_task() {
    let agent_id = aether_core::id::AgentId::new();
    let task_id = aether_core::id::TaskId::new();
    let meta = McpCaller {
        agent_id,
        task_id: Some(task_id),
    }
    .meta();
    assert_eq!(
        serde_json::Value::Object(meta),
        serde_json::json!({META_AGENT_ID: agent_id.to_string(), META_TASK_ID: task_id.to_string()})
    );
    let meta = McpCaller {
        agent_id,
        task_id: None,
    }
    .meta();
    assert_eq!(meta.len(), 1);
    assert!(meta.keys().all(|key| key.starts_with(META_PREFIX)));
}
