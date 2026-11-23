use super::*;
use aether_core::agent::AgentConfig;
use aether_core::delegation::{DelegationPolicy, ModelTier, WorkerChoice};
use aether_core::model::Effort;

pub(super) fn adaptive_policy() -> DelegationPolicy {
    DelegationPolicy {
        workers: vec![WorkerChoice {
            model: None,
            tier: Some(ModelTier::Balanced),
            effort: Some(Effort::Medium),
        }],
        default_tier: Some(ModelTier::Balanced),
        max_effort: Some(Effort::High),
        max_parallel: Some(1),
        exempt_roles: vec!["reviewer".into()],
        exempt_models: Vec::new(),
        require_independent_vendor: false,
    }
}

/// A lead on the frontier model carrying `policy`, as Archon creates one.
pub(super) async fn lead_harness(policy: DelegationPolicy) -> (ComposeHarness, tempfile::TempDir) {
    // A scripted client stands in for provider auth, as in a real daemon
    // with credentials; the children's turns are never inspected.
    let dir = tempfile::tempdir().expect("config dir");
    let runtime = Runtime::start_with_engines(
        crate::RuntimeConfig {
            holder: "unit-test".into(),
            default_model: Some(ModelRef::new("anthropic", "claude-sonnet-5-5")),
            ..Default::default()
        },
        Arc::new(aether_store::SqliteStore::open_in_memory().expect("store")),
        Arc::new(aether_model::Models::load(Some(dir.path().into())).expect("models")),
        Arc::new(aether_tools::ToolRegistry::with_builtins()),
        Some(Arc::new(aether_model::testing::ScriptedClient::new())),
        Default::default(),
    )
    .await
    .expect("runtime");
    let (agent, task) = runtime
        .create_agent(
            CreateAgentSpec {
                role: "lead".into(),
                objective: Some("lead the change".into()),
                model: Some(ModelRef::new("anthropic", "claude-fable-5-2")),
                config: Some(AgentConfig {
                    delegation_policy: Some(policy),
                    ..Default::default()
                }),
                start: false,
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("create lead");
    assert!(
        agent.config.delegation_policy.is_some(),
        "the policy is stored"
    );
    let ctx = TurnCtx {
        agent,
        task,
        turn: 0,
        turn_usage: aether_core::model::Usage::default(),
        base_actor: Actor::user(),
        hard_cancel: CancellationToken::new(),
        recovery_checked_for: None,
        user_reply_pending: false,
        reply_given_unrecorded: false,
        start_handoff: None,
        unanswered_tool_error: None,
    };
    (ComposeHarness { runtime, ctx }, dir)
}

pub(super) fn delegate(
    role: &str,
    model: &str,
    effort: Effort,
    reason: Option<&str>,
) -> HarnessAction {
    HarnessAction::Delegate {
        role: role.into(),
        name: None,
        objective: "part".into(),
        instructions: None,
        model: ModelRef::parse(model).expect("provider/model"),
        fallbacks: Vec::new(),
        effort,
        scope: None,
        workspace_mode: aether_tools::WorkspaceMode::Shared,
        wait: false,
        leaf: true,
        host_context: None,
        idempotency: None,
        reason: reason.map(str::to_owned),
        references: Vec::new(),
        inherit_references: false,
    }
}

pub(super) async fn run(
    h: &mut ComposeHarness,
    action: HarnessAction,
) -> aether_core::tool::ToolOutcome {
    apply_action(
        &h.runtime,
        &mut h.ctx,
        action,
        &mut ToolEffects::default(),
        false,
    )
    .await
    .expect("a refusal is a tool outcome")
}

/// An unstarted child stays unfinished, so it holds a `max_parallel` slot.
fn seed_unfinished_child(h: &ComposeHarness, role: &str) {
    h.runtime
        .create_agent(
            CreateAgentSpec {
                role: role.into(),
                parent: Some(h.ctx.agent.id),
                model: Some(ModelRef::new("anthropic", "claude-sonnet-5-5")),
                config: Some(AgentConfig {
                    effort: Some(Effort::Low),
                    ..Default::default()
                }),
                start: false,
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("seed child");
}

#[tokio::test]
async fn a_policy_refuses_a_child_outside_it_naming_the_allowed_choices() {
    let (mut h, _dir) = lead_harness(adaptive_policy()).await;
    let clone = delegate(
        "worker",
        "anthropic/claude-fable-5-2",
        Effort::Low,
        Some("a reason does not widen a policy"),
    );
    let refused = run(&mut h, clone).await;
    assert!(refused.is_error, "{}", refused.content);
    assert!(
        refused
            .content
            .contains("the model is outside your delegation policy"),
        "{}",
        refused.content
    );
    assert!(
        refused
            .content
            .contains("any balanced-tier model at effort medium or lower"),
        "{}",
        refused.content
    );
    let too_hard = run(
        &mut h,
        delegate("worker", "anthropic/claude-sonnet-5-5", Effort::High, None),
    )
    .await;
    assert!(
        too_hard.is_error && too_hard.content.contains("effort high is above"),
        "{}",
        too_hard.content
    );
    let children = h.runtime.store.agents(&aether_store::AgentFilter {
        parent: Some(h.ctx.agent.id),
        ..Default::default()
    });
    assert!(
        children.expect("children").is_empty(),
        "a refusal created a child"
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn a_fallback_obeys_its_own_tier_and_effort_limits() {
    let primary = "anthropic/claude-sonnet-5-5";
    let haiku = "anthropic/claude-haiku-5-5";
    let tiered = DelegationPolicy {
        default_tier: Some(ModelTier::Balanced),
        ..Default::default()
    };
    let with_efforts = DelegationPolicy {
        workers: [(primary, Effort::Medium), (haiku, Effort::Low)]
            .into_iter()
            .map(|(model, effort)| WorkerChoice {
                model: ModelRef::parse(model),
                tier: None,
                effort: Some(effort),
            })
            .collect(),
        ..Default::default()
    };
    for (policy, fallback, why) in [
        (tiered, "anthropic/claude-opus-5-5", "tier strong is above"),
        (with_efforts, haiku, "effort medium is above"),
    ] {
        let (mut h, _dir) = lead_harness(policy).await;
        let mut action = delegate("worker", primary, Effort::Medium, None);
        let HarnessAction::Delegate { fallbacks, .. } = &mut action else {
            panic!("expected delegate");
        };
        fallbacks.push(ModelRef::parse(fallback).unwrap());
        let refusal = run(&mut h, action).await;
        assert!(refusal.is_error, "{}", refusal.content);
        assert!(
            refusal
                .content
                .contains(&format!("fallback model `{fallback}`"))
                && refusal.content.contains(why),
            "{}",
            refusal.content
        );
        h.runtime.shutdown().await;
    }
}

#[tokio::test]
async fn an_independent_reviewer_cannot_fall_back_to_the_leads_vendor() {
    let policy = DelegationPolicy {
        require_independent_vendor: true,
        exempt_roles: vec!["reviewer".into()],
        ..Default::default()
    };
    let (mut h, _dir) = lead_harness(policy).await;
    let mut action = delegate("reviewer", "openai/gpt-6.2-sol", Effort::Medium, None);
    let HarnessAction::Delegate { fallbacks, .. } = &mut action else {
        panic!("expected delegate");
    };
    fallbacks.push(ModelRef::new("claude-code", "haiku"));
    let refusal = run(&mut h, action).await;
    assert!(refusal.is_error, "{}", refusal.content);
    assert!(
        refusal
            .content
            .contains("fallback model `claude-code/haiku`")
            && refusal.content.contains("shares your vendor `anthropic`"),
        "{}",
        refusal.content
    );
    let independent = delegate("reviewer", "openai/gpt-6.2-sol", Effort::Medium, None);
    let allowed = run(&mut h, independent).await;
    assert!(!allowed.is_error, "{}", allowed.content);
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn a_chain_warns_about_a_fresh_zero_headroom_report() {
    use aether_core::provider::{ProviderUsage, QuotaWindow, UsageWindow};

    let (mut h, _dir) = lead_harness(DelegationPolicy::default()).await;
    let at = aether_core::time::now();
    crate::scheduler::record_quota(
        &h.runtime,
        "anthropic",
        ProviderUsage {
            observed_at: at,
            windows: vec![UsageWindow {
                name: "five_hour".into(),
                quota: QuotaWindow {
                    used_percent: 100,
                    window_minutes: Some(300),
                    resets_at: Some(at + chrono::Duration::hours(1)),
                },
            }],
        },
    )
    .unwrap();
    let mut action = delegate(
        "worker",
        "anthropic/claude-sonnet-5-5",
        Effort::Medium,
        None,
    );
    let legacy = run(&mut h, action.clone()).await;
    assert!(!legacy.is_error && !legacy.content.contains("zero quota headroom"));
    let HarnessAction::Delegate { fallbacks, .. } = &mut action else {
        panic!("expected delegate");
    };
    fallbacks.push(ModelRef::new("openai", "gpt-6.2-sol"));
    let admitted = run(&mut h, action).await;
    assert!(!admitted.is_error, "{}", admitted.content);
    assert!(
        admitted.content.contains("anthropic/claude-sonnet-5-5")
            && admitted.content.contains("zero quota headroom"),
        "{}",
        admitted.content
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn max_parallel_counts_unfinished_workers_but_not_exempt_roles() {
    let (mut h, _dir) = lead_harness(adaptive_policy()).await;
    seed_unfinished_child(&h, "reviewer");
    let first = run(
        &mut h,
        delegate(
            "worker",
            "anthropic/claude-sonnet-5-5",
            Effort::Medium,
            None,
        ),
    )
    .await;
    assert!(
        !first.is_error,
        "an exempt child holds no slot: {}",
        first.content
    );
    seed_unfinished_child(&h, "worker");
    let second = run(
        &mut h,
        delegate("worker", "anthropic/claude-haiku-5-5", Effort::Low, None),
    )
    .await;
    assert!(second.is_error, "{}", second.content);
    assert!(
        second.content.contains("at most 1 unfinished"),
        "{}",
        second.content
    );
    let reviewer = run(
        &mut h,
        delegate("reviewer", "anthropic/claude-opus-5-5", Effort::XHigh, None),
    )
    .await;
    assert!(
        !reviewer.is_error,
        "an exempt reviewer is not bounded by the list: {}",
        reviewer.content
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn an_exempt_role_is_still_bounded_by_the_parents_tier() {
    let mut policy = adaptive_policy();
    policy.exempt_models = vec![ModelRef::new("anthropic", "claude-fable-5-1")];
    let (mut h, _dir) = lead_harness(policy).await;
    h.ctx.agent.model = ModelRef::new("anthropic", "claude-sonnet-5-5");
    let stored = h
        .runtime
        .store
        .agent(h.ctx.agent.id)
        .expect("read")
        .expect("lead");
    let mut demoted = stored;
    demoted.model = ModelRef::new("anthropic", "claude-sonnet-5-5");
    h.runtime
        .store
        .commit(aether_store::Commit::new().mutate(aether_store::Mutation::UpsertAgent(demoted)))
        .expect("move the lead to the balanced tier");
    let above = run(
        &mut h,
        delegate("reviewer", "anthropic/claude-opus-5-5", Effort::High, None),
    )
    .await;
    assert!(above.is_error, "{}", above.content);
    assert!(
        above.content.contains("above your own balanced tier"),
        "{}",
        above.content
    );
    let planned = run(
        &mut h,
        delegate("reviewer", "anthropic/claude-fable-5-1", Effort::High, None),
    )
    .await;
    assert!(
        !planned.is_error,
        "a planned exempt model passes: {}",
        planned.content
    );
    h.runtime.shutdown().await;
}
