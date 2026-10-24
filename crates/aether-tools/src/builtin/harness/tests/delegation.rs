use super::*;
use crate::tool::WorkspaceMode;

#[tokio::test]
async fn delegation_requires_model_and_effort() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx_in(dir.path());
    for (value, expected) in [
        ("low", Effort::Low),
        ("medium", Effort::Medium),
        ("high", Effort::High),
        ("xhigh", Effort::XHigh),
        ("max", Effort::Max),
    ] {
        let action = act(
            &Delegate,
            &ctx,
            json!({"role":"worker", "objective":"check", "model":"mock/m", "effort":value}),
        )
        .await
        .unwrap();
        assert!(matches!(action, HarnessAction::Delegate { effort: e, .. } if e == expected));
    }
    let err = act(
        &Delegate,
        &ctx,
        json!({"role":"worker", "objective":"check", "model":"mock/m", "effort":"huge"}),
    )
    .await
    .unwrap_err();
    // Names the field (`delegate.effort`), the value received (`huge`)
    // and the accepted forms - the serde-layer enum rejection this fixes
    // would otherwise drop the field name entirely.
    let ToolError::InvalidInput(msg) = &err else {
        panic!("expected InvalidInput, got {err:?}");
    };
    assert!(msg.contains("delegate.effort"), "{msg}");
    assert!(msg.contains("huge"), "{msg}");
    assert!(msg.contains("low"), "{msg}");
    for input in [
        json!({"role":"worker", "objective":"check"}),
        json!({"role":"worker", "objective":"check", "model":"mock/m"}),
        json!({"role":"worker", "objective":"check", "effort":"low"}),
        json!({"role":"worker", "objective":"check", "model":null, "effort":"low"}),
        json!({"role":"worker", "objective":"check", "model":"mock/m", "effort":null}),
    ] {
        assert!(matches!(
            act(&Delegate, &ctx, input).await,
            Err(ToolError::InvalidInput(_))
        ));
    }
    let schema = Delegate.descriptor().spec.input_schema;
    for field in ["model", "effort"] {
        assert!(
            schema["required"]
                .as_array()
                .unwrap()
                .contains(&json!(field))
        );
    }
}

#[tokio::test]
async fn delegate_leaf_is_explicit_and_defaults_off() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx_in(dir.path());
    for setting in [None, Some(false), Some(true)] {
        let mut input =
            json!({"role":"worker", "objective":"check", "model":"mock/m", "effort":"low"});
        if let Some(leaf) = setting {
            input["leaf"] = json!(leaf);
        }
        let action = act(&Delegate, &ctx, input).await.unwrap();
        assert!(
            matches!(action, HarnessAction::Delegate { leaf, .. } if leaf == setting.unwrap_or(false))
        );
    }
    let invalid = json!({"role":"worker", "objective":"check", "model":"mock/m", "effort":"low", "leaf":"true"});
    assert!(matches!(
        act(&Delegate, &ctx, invalid).await,
        Err(ToolError::InvalidInput(_))
    ));
    let schema = Delegate.descriptor().spec.input_schema;
    assert_eq!(schema["properties"]["leaf"]["default"], false);
}

#[tokio::test]
async fn delegate_host_context_is_optional_and_named_on_error() {
    use aether_core::agent::HostContext;
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx_in(dir.path());
    for (setting, expected) in [
        (None, None),
        (Some("full"), Some(HostContext::Full)),
        (Some("lean"), Some(HostContext::Lean)),
    ] {
        let mut input =
            json!({"role":"worker", "objective":"check", "model":"mock/m", "effort":"low"});
        if let Some(host_context) = setting {
            input["host_context"] = json!(host_context);
        }
        let action = act(&Delegate, &ctx, input).await.unwrap();
        assert!(
            matches!(action, HarnessAction::Delegate { host_context, .. } if host_context == expected)
        );
    }
    let invalid = json!({"role":"worker", "objective":"check", "model":"mock/m", "effort":"low", "host_context":"minimal"});
    let err = act(&Delegate, &ctx, invalid).await.unwrap_err();
    let ToolError::InvalidInput(msg) = &err else {
        panic!("expected InvalidInput, got {err:?}");
    };
    assert!(msg.contains("delegate.host_context"), "{msg}");
    assert!(msg.contains("minimal"), "{msg}");
    let schema = Delegate.descriptor().spec.input_schema;
    assert_eq!(
        schema["properties"]["host_context"]["enum"],
        json!(["full", "lean"])
    );
}

#[tokio::test]
async fn delegate_workspace_mode_is_explicit_and_shared_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx_in(dir.path());
    let input = json!({"role":"worker", "objective":"edit", "model":"mock/m", "effort":"low"});
    let default = act(&Delegate, &ctx, input.clone()).await.unwrap();
    assert!(matches!(
        default,
        HarnessAction::Delegate {
            workspace_mode: WorkspaceMode::Shared,
            ..
        }
    ));

    let mut isolated = input.clone();
    isolated["workspace_mode"] = json!("isolated");
    isolated["workspace_root"] = json!("nested");
    let action = act(&Delegate, &ctx, isolated).await.unwrap();
    assert!(matches!(
        action,
        HarnessAction::Delegate {
            workspace_mode: WorkspaceMode::Isolated,
            scope: Some(_),
            ..
        }
    ));

    let mut invalid = input;
    invalid["workspace_mode"] = json!("automatic");
    assert!(matches!(
        act(&Delegate, &ctx, invalid).await,
        Err(ToolError::InvalidInput(_))
    ));
    let schema = Delegate.descriptor().spec.input_schema;
    assert_eq!(schema["properties"]["workspace_mode"]["default"], "shared");
    assert_eq!(
        schema["properties"]["workspace_mode"]["enum"],
        json!(["shared", "isolated"])
    );
}

#[tokio::test]
async fn integrate_workspace_requires_a_child_agent_id() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx_in(dir.path());
    let child = AgentId::new();
    let action = act(
        &IntegrateWorkspace,
        &ctx,
        json!({"child_agent_id": child.to_string()}),
    )
    .await
    .unwrap();
    assert!(matches!(action, HarnessAction::IntegrateWorkspace { child: id } if id == child));
    for input in [json!({}), json!({"child_agent_id": "bad"})] {
        assert!(matches!(
            act(&IntegrateWorkspace, &ctx, input).await,
            Err(ToolError::InvalidInput(_))
        ));
    }
    let schema = IntegrateWorkspace.descriptor().spec.input_schema;
    assert_eq!(schema["required"], json!(["child_agent_id"]));
}

#[tokio::test]
async fn delegate_parses_and_narrows_scope() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx_in(dir.path());
    let a = act(&Delegate, &ctx, json!({"role": "worker", "objective": "do x", "model": "mock/m", "effort":"low", "workspace_root": "sub"}))
        .await
        .unwrap();
    match a {
        HarnessAction::Delegate {
            role,
            model,
            scope,
            wait,
            ..
        } => {
            assert_eq!(role, "worker");
            assert_eq!(model, ModelRef::new("mock", "m"));
            assert_eq!(scope.unwrap().workspace_root, Some(dir.path().join("sub")));
            assert!(wait);
        }
        other => panic!("{other:?}"),
    }
    let escape = act(
        &Delegate,
        &ctx,
        json!({"role": "w", "objective": "x", "model":"mock/m", "effort":"low", "network": true}),
    )
    .await;
    assert!(
        matches!(escape, Err(ToolError::Denied(_))),
        "network cannot exceed parent"
    );
    let bad_model = act(
        &Delegate,
        &ctx,
        json!({"role": "w", "objective": "x", "model": "nope", "effort":"low"}),
    )
    .await;
    assert!(matches!(bad_model, Err(ToolError::InvalidInput(_))));
}

/// The scope fields decide what a child may touch, so each must
/// describe itself - `allowed_paths` shipped without a description.
#[test]
fn delegate_documents_scope_properties() {
    let schema = Delegate.descriptor().spec.input_schema;
    for field in ["workspace_root", "allowed_paths", "network"] {
        let description = schema["properties"][field]["description"]
            .as_str()
            .unwrap_or_default();
        assert!(
            !description.trim().is_empty(),
            "delegate.{field} has no description"
        );
    }
    let paths = schema["properties"]["allowed_paths"]["description"]
        .as_str()
        .unwrap();
    assert!(
        paths.contains("enforced") && paths.contains("Aether's built-in shell"),
        "delegate.allowed_paths must say the boundary is enforced: {paths}"
    );
}

/// `delegate.role` was accepted unvalidated; an empty or
/// whitespace-only role must be rejected, naming the field.
#[tokio::test]
async fn delegate_rejects_empty_role() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx_in(dir.path());
    let err = act(
        &Delegate,
        &ctx,
        json!({"role": "  ", "objective": "x", "model": "mock/m", "effort": "low"}),
    )
    .await
    .unwrap_err();
    let ToolError::InvalidInput(msg) = &err else {
        panic!("expected InvalidInput, got {err:?}");
    };
    assert!(msg.contains("role"), "{msg}");
}

/// A keyed repeat is compared on the arguments the caller sent, not on a
/// scope inherited from a parent whose own scope may have narrowed since.
#[tokio::test]
async fn a_keyed_delegate_carries_the_callers_arguments_not_the_inherited_scope() {
    use aether_core::agent::NetworkPolicy;

    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("nested")).unwrap();
    let mut before = ctx_in(dir.path());
    before.scope.network = NetworkPolicy::Allowed;
    let mut narrowed = ctx_in(dir.path());
    narrowed.scope.network = NetworkPolicy::Denied;
    assert_ne!(before.scope, narrowed.scope);
    let input = json!({"role":"worker", "objective":"check", "model":"mock/m", "effort":"low",
                       "workspace_root":"nested", "wait": true, "idempotency_key":"k-1"});
    let keyed = |action: HarnessAction| match action {
        HarnessAction::Delegate {
            idempotency, scope, ..
        } => (idempotency.expect("keyed"), scope.expect("scoped")),
        other => panic!("not a delegation: {other:?}"),
    };
    let (first, first_scope) = keyed(act(&Delegate, &before, input.clone()).await.unwrap());
    let (repeat, repeat_scope) = keyed(act(&Delegate, &narrowed, input).await.unwrap());
    assert_ne!(first_scope.network, repeat_scope.network);
    assert_eq!(first, repeat);
    assert_eq!(first.key, "k-1");
    assert!(first.request.get("wait").is_none());
    assert!(first.request.get("idempotency_key").is_none());

    let unkeyed = json!({"role":"worker", "objective":"check", "model":"mock/m", "effort":"low"});
    assert!(matches!(
        act(&Delegate, &before, unkeyed).await.unwrap(),
        HarnessAction::Delegate {
            idempotency: None,
            ..
        }
    ));
}
