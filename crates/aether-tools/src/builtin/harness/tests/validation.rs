use super::*;

#[tokio::test]
async fn complete_send_remember_ask() {
    let dir = tempfile::tempdir().unwrap();
    let mut ctx = ctx_in(dir.path());
    ctx.scope.repository = Some("repo-a".into());
    let art = ArtifactId::new();
    let a = act(
        &CompleteTask,
        &ctx,
        json!({"summary": "done", "artifacts": [art.to_string()]}),
    )
    .await
    .unwrap();
    assert_eq!(
        a,
        HarnessAction::CompleteTask {
            summary: "done".into(),
            artifacts: vec![art],
            structured: None
        }
    );
    assert!(matches!(
        act(&CompleteTask, &ctx, json!({"summary": " "})).await,
        Err(ToolError::InvalidInput(_))
    ));

    let to = AgentId::new();
    let a = act(
        &SendMessage,
        &ctx,
        json!({"to": to.to_string(), "content": "hi"}),
    )
    .await
    .unwrap();
    assert_eq!(
        a,
        HarnessAction::SendMessage {
            to,
            content: "hi".into(),
            subtree: false
        }
    );
    let a = act(
        &SendMessage,
        &ctx,
        json!({"to": to.to_string(), "content": "hi", "subtree": true}),
    )
    .await
    .unwrap();
    assert_eq!(
        a,
        HarnessAction::SendMessage {
            to,
            content: "hi".into(),
            subtree: true
        }
    );

    let a = act(
        &Remember,
        &ctx,
        json!({"text": "uses pnpm", "kind": "fact", "tags": ["build"]}),
    )
    .await
    .unwrap();
    match a {
        HarnessAction::Remember { candidate } => {
            assert_eq!(
                candidate.scope,
                MemoryScope::Repository {
                    id: "repo-a".into()
                }
            );
            assert_eq!(candidate.kind, MemoryKind::Fact);
            assert_eq!(candidate.provenance.agent, Some(ctx.agent_id));
        }
        other => panic!("{other:?}"),
    }
    // A subtree note is always rooted at the caller; `root` was never a
    // declared field, so it must be rejected rather than silently
    // dropped - a model naming a root other than itself should
    // learn that immediately, not discover the silent no-op later.
    match act(
        &Remember,
        &ctx,
        json!({"text": "reviews run clippy first", "kind": "lesson", "scope": "subtree"}),
    )
    .await
    .unwrap()
    {
        HarnessAction::Remember { candidate } => {
            assert_eq!(candidate.scope, MemoryScope::Subtree { root: ctx.agent_id });
        }
        other => panic!("{other:?}"),
    }
    let elsewhere = AgentId::new().to_string();
    let err = act(
        &Remember,
        &ctx,
        json!({"text": "reviews run clippy first", "kind": "lesson", "scope": "subtree", "root": elsewhere}),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&err, ToolError::InvalidInput(msg) if msg.contains("root")),
        "expected an unknown-field rejection naming `root`, got {err:?}"
    );
    let a = act(&AskUser, &ctx, json!({"question": "which db?"}))
        .await
        .unwrap();
    assert_eq!(
        a,
        HarnessAction::AskUser {
            question: "which db?".into()
        }
    );
    let a = act(&FailTask, &ctx, json!({"error": "no"})).await.unwrap();
    assert_eq!(
        a,
        HarnessAction::FailTask {
            error: "no".into(),
            structured: None
        }
    );
    let a = act(
        &FailTask,
        &ctx,
        json!({"error": "no", "structured": {"attempts": 3}}),
    )
    .await
    .unwrap();
    assert_eq!(
        a,
        HarnessAction::FailTask {
            error: "no".into(),
            structured: Some(json!({"attempts": 3}))
        }
    );
    let a = act(&RequestCheckpoint, &ctx, json!({"reason": "long"}))
        .await
        .unwrap();
    assert_eq!(
        a,
        HarnessAction::RequestCheckpoint {
            reason: "long".into()
        }
    );
}

#[tokio::test]
async fn remember_kind_names_field_value_and_forms() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx_in(dir.path());
    let err = act(&Remember, &ctx, json!({"text": "note", "kind": "opinion"}))
        .await
        .unwrap_err();
    let ToolError::InvalidInput(msg) = &err else {
        panic!("expected InvalidInput, got {err:?}");
    };
    assert!(msg.contains("remember.kind"), "{msg}");
    assert!(msg.contains("opinion"), "{msg}");
    assert!(msg.contains("fact"), "{msg}");
}

/// A wrong-*typed* value (not just a wrong-valued one) must also
/// name the field - routing these fields through `Value` rather than a
/// pre-typed `String` struct field is what makes that possible, since
/// serde would otherwise reject the wrong type before either helper runs.
#[tokio::test]
async fn wrong_json_type_still_names_the_field() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx_in(dir.path());
    let err = act(
        &Delegate,
        &ctx,
        json!({"role": "w", "objective": "x", "model": "mock/m", "effort": 5}),
    )
    .await
    .unwrap_err();
    let ToolError::InvalidInput(msg) = &err else {
        panic!("expected InvalidInput, got {err:?}");
    };
    assert!(msg.contains("delegate.effort"), "{msg}");

    let err = act(&Remember, &ctx, json!({"text": "note", "kind": true}))
        .await
        .unwrap_err();
    let ToolError::InvalidInput(msg) = &err else {
        panic!("expected InvalidInput, got {err:?}");
    };
    assert!(msg.contains("remember.kind"), "{msg}");
}

/// Every advertised `additionalProperties: false` harness input
/// must actually reject an unknown field, naming it, instead of quietly
/// dropping it and doing something the caller didn't ask for. Covers
/// every harness tool with `deny_unknown_fields`, not just the eight
/// that gained it last - `InspectState`/`ListModels`/`CancelAgent`
/// already had it and must not regress.
#[tokio::test]
async fn unknown_field_is_rejected_on_every_advertised_input() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx_in(dir.path());
    let to = AgentId::new().to_string();
    let agent = AgentId::new().to_string();
    let cases: Vec<(&dyn Tool, Value)> = vec![
        (&InspectState, json!({"bogus": 1})),
        (&ListModels, json!({"bogus": 1})),
        (&ProviderUsage, json!({"bogus": 1})),
        (&RefreshProviderUsage, json!({"bogus": 1})),
        (
            &Delegate,
            json!({"role": "w", "objective": "x", "model": "mock/m", "effort": "low", "bogus": 1}),
        ),
        (&SendMessage, json!({"to": to, "content": "hi", "bogus": 1})),
        (
            &CancelAgent,
            json!({"agent": agent, "reason": "stuck", "bogus": 1}),
        ),
        (&CompleteTask, json!({"summary": "done", "bogus": 1})),
        (&FailTask, json!({"error": "no", "bogus": 1})),
        (&WaitFor, json!({"tasks": [], "bogus": 1})),
        (
            &Remember,
            json!({"text": "note", "kind": "fact", "bogus": 1}),
        ),
        (&RequestCheckpoint, json!({"reason": "long", "bogus": 1})),
        (&AskUser, json!({"question": "q?", "bogus": 1})),
        (&Notify, json!({"title": "t", "bogus": 1})),
    ];
    for (tool, input) in cases {
        let name = tool.descriptor().spec.name;
        let err = act(tool, &ctx, input).await.unwrap_err();
        let ToolError::InvalidInput(msg) = &err else {
            panic!("{name}: expected InvalidInput, got {err:?}");
        };
        assert!(msg.contains("bogus"), "{name}: {msg}");
    }
}

/// `request_checkpoint.reason` was accepted empty; it must be
/// rejected, naming the field.
#[tokio::test]
async fn request_checkpoint_rejects_empty_reason() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx_in(dir.path());
    let err = act(&RequestCheckpoint, &ctx, json!({"reason": "   "}))
        .await
        .unwrap_err();
    let ToolError::InvalidInput(msg) = &err else {
        panic!("expected InvalidInput, got {err:?}");
    };
    assert!(msg.contains("reason"), "{msg}");
}

/// `remember.importance`/`confidence` were silently clamped into
/// [0, 1] instead of rejecting a value outside it; they must now be
/// rejected, naming the field and the value received. Boundary values
/// (0 and 1) remain valid input and must still be accepted unchanged.
#[tokio::test]
async fn remember_rejects_out_of_range_importance_and_confidence() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx_in(dir.path());
    let err = act(
        &Remember,
        &ctx,
        json!({"text": "note", "kind": "fact", "importance": 1.5}),
    )
    .await
    .unwrap_err();
    let ToolError::InvalidInput(msg) = &err else {
        panic!("expected InvalidInput, got {err:?}");
    };
    assert!(msg.contains("importance"), "{msg}");
    assert!(msg.contains("1.5"), "{msg}");

    let err = act(
        &Remember,
        &ctx,
        json!({"text": "note", "kind": "fact", "confidence": -0.1}),
    )
    .await
    .unwrap_err();
    let ToolError::InvalidInput(msg) = &err else {
        panic!("expected InvalidInput, got {err:?}");
    };
    assert!(msg.contains("confidence"), "{msg}");

    let a = act(
        &Remember,
        &ctx,
        json!({"text": "note", "kind": "fact", "importance": 0.0, "confidence": 1.0}),
    )
    .await
    .unwrap();
    match a {
        HarnessAction::Remember { candidate } => {
            assert_eq!(candidate.importance, 0.0);
            assert_eq!(candidate.confidence, 1.0);
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn result_tools_require_an_object_for_structured() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx_in(dir.path());
    for bad in [json!([1, 2]), json!("s"), json!(3)] {
        let complete = act(
            &CompleteTask,
            &ctx,
            json!({"summary": "d", "structured": bad}),
        )
        .await;
        assert!(
            matches!(&complete, Err(ToolError::InvalidInput(m)) if m.contains("structured")),
            "{bad}: {complete:?}"
        );
        let fail = act(&FailTask, &ctx, json!({"error": "e", "structured": bad})).await;
        assert!(
            matches!(&fail, Err(ToolError::InvalidInput(m)) if m.contains("structured")),
            "{bad}: {fail:?}"
        );
    }
    let ok = act(
        &CompleteTask,
        &ctx,
        json!({"summary": "d", "structured": {"k": 1}}),
    )
    .await;
    assert!(ok.is_ok(), "{ok:?}");
}

#[tokio::test]
async fn result_tools_treat_null_structured_as_absent() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx_in(dir.path());
    let complete = act(
        &CompleteTask,
        &ctx,
        json!({"summary": "d", "structured": null}),
    )
    .await
    .unwrap();
    assert_eq!(
        complete,
        HarnessAction::CompleteTask {
            summary: "d".into(),
            artifacts: vec![],
            structured: None
        }
    );
    let fail = act(&FailTask, &ctx, json!({"error": "e", "structured": null}))
        .await
        .unwrap();
    assert_eq!(
        fail,
        HarnessAction::FailTask {
            error: "e".into(),
            structured: None
        }
    );
}

#[tokio::test]
async fn result_tools_bound_text_and_structured_size() {
    use super::super::tasks::{MAX_RESULT_TEXT_BYTES, MAX_STRUCTURED_BYTES};
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx_in(dir.path());
    let big = "x".repeat(MAX_RESULT_TEXT_BYTES + 1);
    let r = act(&CompleteTask, &ctx, json!({"summary": big})).await;
    assert!(
        matches!(&r, Err(ToolError::InvalidInput(m)) if m.contains("summary")),
        "{r:?}"
    );
    let r = act(&FailTask, &ctx, json!({"error": big})).await;
    assert!(
        matches!(&r, Err(ToolError::InvalidInput(m)) if m.contains("error")),
        "{r:?}"
    );
    let huge = json!({"blob": "x".repeat(MAX_STRUCTURED_BYTES)});
    let r = act(
        &CompleteTask,
        &ctx,
        json!({"summary": "d", "structured": huge}),
    )
    .await;
    assert!(
        matches!(&r, Err(ToolError::InvalidInput(m)) if m.contains("structured")),
        "{r:?}"
    );
    let at_limit = "x".repeat(MAX_RESULT_TEXT_BYTES);
    assert!(
        act(&CompleteTask, &ctx, json!({"summary": at_limit}))
            .await
            .is_ok()
    );
}

/// A post is checked against the notification limits before it reaches the
/// runtime, and `resolve` takes the key and nothing else.
#[tokio::test]
async fn notify_posts_a_checked_notification_or_resolves_by_key() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx_in(dir.path());
    let post = json!({"title": "Ship 1.3?", "kind": "question", "key": "rel",
        "actions": [{"id": "ship", "label": "Ship it"}], "free_text": true});
    let HarnessAction::Notify { post } = act(&Notify, &ctx, post).await.unwrap() else {
        panic!("a post");
    };
    assert_eq!(
        post.kind,
        aether_core::notification::NotificationKind::Question
    );
    assert_eq!(post.actions[0].label, "Ship it");
    let plain = act(&Notify, &ctx, json!({"title": "Done"})).await.unwrap();
    assert!(
        matches!(&plain, HarnessAction::Notify { post } if post.kind.as_str() == "info"),
        "{plain:?}"
    );
    for (input, named) in [
        (json!({"title": " "}), "title"),
        (json!({"title": "t", "kind": "shout"}), "notify.kind"),
        (json!({"resolve": true}), "key"),
        (
            json!({"resolve": true, "key": "rel", "title": "t"}),
            "only the key",
        ),
    ] {
        let err = act(&Notify, &ctx, input.clone()).await.unwrap_err();
        let ToolError::InvalidInput(msg) = &err else {
            panic!("{input}: expected InvalidInput, got {err:?}");
        };
        assert!(msg.contains(named), "{input}: {msg}");
    }
    let resolved = act(&Notify, &ctx, json!({"resolve": true, "key": "rel"}))
        .await
        .unwrap();
    assert_eq!(
        resolved,
        HarnessAction::ResolveNotification { key: "rel".into() }
    );
}
