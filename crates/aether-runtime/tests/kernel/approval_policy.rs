//! approval policy scenarios and fixtures.
use super::*;

#[tokio::test]
async fn yolo_and_codex_agents_carry_no_approval_policy() {
    let h = harness_configured(vec![], |_| {}, |c| c.yolo = true).await;
    let (agent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                start: false,
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("create");
    assert!(
        agent.config.approval.require_for.is_empty(),
        "yolo strips approvals"
    );
    let err = h
        .runtime
        .update_agent(
            agent.id,
            aether_runtime::AgentUpdate {
                approval: Some(ApprovalPolicy::default()),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect_err("approvals cannot be turned on under yolo");
    assert!(err.to_string().contains("yolo"), "{err}");

    let h = harness(vec![]).await;
    let codex_model = ModelRef::new("codex", "gpt-6-luna");
    let (codex, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                model: Some(codex_model.clone()),
                approval: Some(ApprovalPolicy::default()),
                start: false,
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("create codex agent");
    // Codex gates only through its app-server; an exec-only build cannot.
    assert_eq!(
        codex.config.approval.require_for.is_empty(),
        !h.runtime
            .engine_can_gate(&codex_model)
            .expect("engines detected"),
        "policy follows the engine's ability to ask: {:?}",
        codex.config.approval
    );
    let (api, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                start: false,
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("create api agent");
    assert!(api.config.approval.is_yolo(), "the shipped default is yolo");
}

/// Yolo is the shipped default, a child inherits its parent's
/// policy, and neither a create nor a live patch may hand a child fewer gates
/// than its parent keeps.
#[tokio::test]
async fn approvals_default_to_yolo_and_a_child_never_widens_past_its_parent() {
    let gated = ApprovalPolicy::default();

    // `[approval] default = "ask"` turns the shipped default around.
    let h = harness_configured(
        vec![],
        |_| {},
        |c| c.approval_default = aether_core::agent::ApprovalDefault::Ask,
    )
    .await;
    let (root, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "root".into(),
                start: false,
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("root");
    assert_eq!(
        root.config.approval, gated,
        "the configured default applies"
    );

    // A child inherits it without asking for anything.
    let (child, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                parent: Some(root.id),
                config: Some(aether_core::agent::AgentConfig {
                    effort: Some(aether_core::model::Effort::Low),
                    ..Default::default()
                }),
                start: false,
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("child");
    assert_eq!(
        child.config.approval, gated,
        "a child inherits the parent's"
    );

    // ...and cannot be created with fewer gates than the parent keeps.
    let err = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                parent: Some(root.id),
                config: Some(aether_core::agent::AgentConfig {
                    effort: Some(aether_core::model::Effort::Low),
                    ..Default::default()
                }),
                approval: Some(ApprovalPolicy::none()),
                start: false,
                ..Default::default()
            },
            Actor::user(),
        )
        .expect_err("a yolo child under a gated parent");
    assert!(err.to_string().contains("cannot drop a gate"), "{err}");

    // Nor patched into one afterwards.
    let err = h
        .runtime
        .set_approvals(child.id, ApprovalPolicy::none(), false, Actor::user())
        .expect_err("a live child cannot widen either");
    assert!(err.to_string().contains("cannot drop a gate"), "{err}");

    // The parent going yolo takes the subtree with it, in one call.
    let moved = h
        .runtime
        .set_approvals(root.id, ApprovalPolicy::none(), true, Actor::user())
        .expect("subtree yolo");
    assert_eq!(moved.len(), 2, "root and child");
    assert!(moved.iter().all(|a| a.config.approval.is_yolo()));

    // A yolo parent may still hand out a gated child: gating more is allowed.
    let (strict, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                parent: Some(root.id),
                config: Some(aether_core::agent::AgentConfig {
                    effort: Some(aether_core::model::Effort::Low),
                    ..Default::default()
                }),
                approval: Some(gated.clone()),
                start: false,
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("a gated child under a yolo parent");
    assert_eq!(strict.config.approval, gated);
}

/// Tightening a parent must not strand a live child that already
/// exists and stays less gated underneath it - the inheritance rule holds
/// against an existing subtree, not only at creation.
#[tokio::test]
async fn tightening_one_agent_refuses_to_strand_a_less_gated_live_child() {
    let gated = ApprovalPolicy::default();
    let h = harness(vec![]).await;
    let (root, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "root".into(),
                start: false,
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("root");
    // Both start yolo (the shipped default); the child inherits it.
    assert!(root.config.approval.is_yolo());
    let (child, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                parent: Some(root.id),
                config: Some(aether_core::agent::AgentConfig {
                    effort: Some(aether_core::model::Effort::Low),
                    ..Default::default()
                }),
                start: false,
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("child");
    assert!(child.config.approval.is_yolo());

    // `UpdateAgent` on the root alone must not leave the child less gated.
    let err = h
        .runtime
        .update_agent(
            root.id,
            aether_runtime::AgentUpdate {
                approval: Some(gated.clone()),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect_err("child would end up less gated than its now-strict parent");
    assert!(
        err.to_string().contains("live descendant gated less"),
        "{err}"
    );

    // `SetApprovals { subtree: false }` on the root has the same shape.
    let err = h
        .runtime
        .set_approvals(root.id, gated.clone(), false, Actor::user())
        .expect_err("same invariant, different entry point");
    assert!(
        err.to_string().contains("live descendant gated less"),
        "{err}"
    );

    // Tightening the child first, then the root, is fine either way.
    h.runtime
        .set_approvals(child.id, gated.clone(), false, Actor::user())
        .expect("child can gate itself first");
    let moved = h
        .runtime
        .set_approvals(root.id, gated.clone(), false, Actor::user())
        .expect("root can now tighten alone");
    assert_eq!(moved.len(), 1);
    assert_eq!(moved[0].config.approval, gated);
}

/// A subtree `SetApprovals` must not silently
/// loosen a descendant deliberately created more gated than its parent - the
/// one case the inheritance rule explicitly permits.
#[tokio::test]
async fn subtree_yolo_leaves_a_deliberately_stricter_child_alone() {
    let root_policy = ApprovalPolicy::default();
    let child_policy = ApprovalPolicy {
        require_for: [SideEffectClass::External, SideEffectClass::Mutation].into(),
    };
    assert!(child_policy.is_narrowing_of(&root_policy), "test setup");

    let h = harness(vec![]).await;
    let (root, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "root".into(),
                approval: Some(root_policy.clone()),
                start: false,
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("root");
    let (child, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                parent: Some(root.id),
                config: Some(aether_core::agent::AgentConfig {
                    effort: Some(aether_core::model::Effort::Low),
                    ..Default::default()
                }),
                approval: Some(child_policy.clone()),
                start: false,
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("child gates more than its parent");
    assert_eq!(child.config.approval, child_policy);
    // Grandchild inherits the stricter child's policy (no override), same
    // shape as the report's B: it must stay gated with its parent, not just
    // fail to move on its own account.
    let (grandchild, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                parent: Some(child.id),
                config: Some(aether_core::agent::AgentConfig {
                    effort: Some(aether_core::model::Effort::Low),
                    ..Default::default()
                }),
                start: false,
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("grandchild inherits the stricter child's policy");
    assert_eq!(grandchild.config.approval, child_policy);

    let moved = h
        .runtime
        .set_approvals(root.id, ApprovalPolicy::none(), true, Actor::user())
        .expect("subtree yolo");
    assert_eq!(
        moved.iter().map(|a| a.id).collect::<Vec<_>>(),
        vec![root.id],
        "only the root moves; the child is deliberately stricter and the \
         grandchild must stay behind with it"
    );
    assert!(moved[0].config.approval.is_yolo());

    let reloaded_child = h.store.agent(child.id).expect("load").expect("exists");
    assert_eq!(
        reloaded_child.config.approval, child_policy,
        "the deliberately stricter child must not be silently loosened"
    );
    let reloaded_grandchild = h.store.agent(grandchild.id).expect("load").expect("exists");
    assert_eq!(
        reloaded_grandchild.config.approval, child_policy,
        "a subtree /yolo must not un-gate a grandchild beneath a \
         deliberately stricter agent, even though the grandchild itself is \
         not stricter than its own parent"
    );
}
