use std::sync::Arc;
use std::time::Duration;

use aether_core::event::{Actor, EventDraft, EventKind};
use aether_core::id::{InvocationId, RunId, SnapshotId};
use aether_core::model::{ModelRef, Usage};
use aether_protocol::noesis::{BillingPatch, BudgetPatch, DisablePatch, NoesisSettingsPatch};
use aether_protocol::{Command, ErrorCode, Reply};
use aether_runtime::CreateAgentSpec;
use aether_runtime::noesis_guard::{
    NOESIS_SUBSYSTEM, NoesisBudget, NoesisGate, NoesisLane, NoesisRefusal,
};
use aether_store::{Commit, Store};

use super::NoesisService;
use super::budget::{self, ServiceBudget};
use crate::Daemon;

struct Harness {
    daemon: Daemon,
    config: tempfile::TempDir,
    data: tempfile::TempDir,
}

impl Harness {
    async fn new() -> Self {
        let config = tempfile::tempdir().expect("config dir");
        let data = tempfile::tempdir().expect("data dir");
        let models = Arc::new(
            aether_model::Models::load(Some(config.path().to_path_buf())).expect("models"),
        );
        let gate = NoesisGate::new();
        let runtime = aether_runtime::Runtime::start_with_engines(
            aether_runtime::RuntimeConfig {
                holder: "test".into(),
                default_model: Some(ModelRef::new("anthropic", "claude-sonnet-5-5")),
                attachment_dir: Some(config.path().join("attachments")),
                noesis: gate.clone(),
                ..Default::default()
            },
            Arc::new(aether_store::SqliteStore::open_in_memory().expect("store")),
            models.clone(),
            Arc::new(aether_tools::ToolRegistry::with_builtins()),
            None,
            Default::default(),
        )
        .await
        .expect("runtime");
        let noesis = Arc::new(NoesisService::new(
            config.path(),
            data.path(),
            models.clone(),
            gate,
        ));
        let daemon = Daemon {
            runtime,
            models,
            instance_id: "inst".into(),
            version: "test".into(),
            push: None,
            remote: Default::default(),
            status_reads: Default::default(),
            noesis,
        };
        Self {
            daemon,
            config,
            data,
        }
    }

    async fn send(&self, command: Command) -> Result<Reply, aether_protocol::ErrorReply> {
        let submission = crate::authz::AuthContext::Local.submission(&command)?;
        super::dispatch(&self.daemon, command, Actor::user(), submission).await
    }

    async fn configure(&self, patch: NoesisSettingsPatch) -> aether_protocol::noesis::NoesisStatus {
        let reply = self
            .send(Command::NoesisConfigure {
                patch,
                idempotency_key: None,
            })
            .await
            .expect("configure");
        let Reply::NoesisStatus(status) = reply else {
            panic!("a status reply, got {reply:?}");
        };
        status
    }

    fn db_exists(&self) -> bool {
        self.data.path().join("noesis.db").exists()
    }

    fn last_seq(&self) -> u64 {
        self.daemon.runtime.store().last_seq().expect("last seq")
    }

    fn settings_events(&self) -> usize {
        let events = self
            .daemon
            .runtime
            .store()
            .events(0, 10_000, &aether_store::EventFilter::default())
            .expect("events");
        events
            .iter()
            .filter(|event| matches!(event.kind, EventKind::NoesisSettingsChanged { .. }))
            .count()
    }
}

fn enable(enabled: bool) -> NoesisSettingsPatch {
    NoesisSettingsPatch {
        enabled: Some(enabled),
        ..Default::default()
    }
}

/// One of each family the service owns, beyond status and configure.
fn service_commands() -> Vec<Command> {
    [
        r#"{"cmd":"noesis_list_sessions"}"#,
        r#"{"cmd":"noesis_get_session","session":"ses_1"}"#,
        r#"{"cmd":"noesis_send","session":"ses_1","text":"hi"}"#,
        r#"{"cmd":"noesis_graph"}"#,
        r#"{"cmd":"noesis_jobs"}"#,
        r#"{"cmd":"noesis_cancel_job","job":"job_1"}"#,
        r#"{"cmd":"noesis_learner"}"#,
        r#"{"cmd":"noesis_extract"}"#,
    ]
    .iter()
    .map(|json| serde_json::from_str(json).expect(json))
    .collect()
}

#[tokio::test]
async fn disabled_by_default_every_service_command_is_refused_with_no_store_change() {
    let harness = Harness::new().await;
    let Reply::NoesisStatus(status) = harness.send(Command::NoesisStatus).await.expect("status")
    else {
        panic!("status reply");
    };
    assert!(!status.enabled && !status.effective.enabled);
    let before = harness.last_seq();
    for command in service_commands() {
        let name = command.name();
        let error = harness.send(command).await.expect_err(&name);
        assert_eq!(error.code, ErrorCode::Unavailable, "{name}: {error:?}");
        assert_eq!(
            error.reason.as_deref(),
            Some(aether_protocol::NOESIS_DISABLED),
            "{name}"
        );
    }
    assert_eq!(harness.last_seq(), before, "a refusal records nothing");
    assert!(
        !harness.db_exists(),
        "nothing opens noesis.db while disabled"
    );
    assert!(
        !harness.config.path().join("config.toml").exists()
            || harness
                .daemon
                .noesis
                .config
                .read_noesis()
                .expect("read")
                .is_empty()
    );
}

#[tokio::test]
async fn configure_while_disabled_writes_the_config_only() {
    let harness = Harness::new().await;
    let before = harness.last_seq();
    let status = harness
        .configure(NoesisSettingsPatch {
            budget: Some(BudgetPatch {
                interactive_usd_daily: Some(3.5),
                ..Default::default()
            }),
            ..Default::default()
        })
        .await;
    assert!(!status.enabled && !status.effective.enabled);
    assert_eq!(status.settings.budget.interactive_usd_daily, 3.5);
    let stored = harness.daemon.noesis.config.read_noesis().expect("read");
    assert!(stored.contains_key("budget"), "{stored:?}");
    assert!(
        !harness.db_exists(),
        "a disabled configure opens no database"
    );
    assert_eq!(
        harness.last_seq(),
        before,
        "nothing to announce while Noesis stays off"
    );
    assert!(!harness.daemon.noesis.gate().is_enabled());
}

#[tokio::test]
async fn enabling_writes_first_then_opens_the_database_and_announces() {
    let harness = Harness::new().await;
    let status = harness.configure(enable(true)).await;
    assert!(status.enabled && status.effective.enabled, "{status:?}");
    assert!(harness.db_exists(), "enabled, noesis.db is open");
    assert!(harness.daemon.noesis.gate().is_enabled());
    let stored = harness.daemon.noesis.config.read_noesis().expect("read");
    assert_eq!(
        stored.get("enabled").and_then(toml::Value::as_bool),
        Some(true)
    );
    assert_eq!(harness.settings_events(), 1);
    let error = harness
        .send(Command::NoesisJobs { state: None })
        .await
        .expect_err("not built yet");
    assert_ne!(
        error.reason.as_deref(),
        Some(aether_protocol::NOESIS_DISABLED)
    );
}

#[tokio::test]
async fn disabling_closes_the_gate_at_once_and_stops_after_the_drain() {
    let harness = Harness::new().await;
    harness.configure(enable(true)).await;
    let status = harness
        .configure(NoesisSettingsPatch {
            enabled: Some(false),
            disable: Some(DisablePatch {
                grace_seconds: Some(1),
            }),
            ..Default::default()
        })
        .await;
    assert!(!status.enabled && !status.effective.enabled);
    assert!(
        !harness.daemon.noesis.gate().is_enabled(),
        "no new work after a disable"
    );
    let error = harness
        .send(Command::NoesisJobs { state: None })
        .await
        .expect_err("disabled");
    assert_eq!(
        error.reason.as_deref(),
        Some(aether_protocol::NOESIS_DISABLED)
    );
    for _ in 0..50 {
        if matches!(*harness.daemon.noesis.state.lock().await, super::State::Off) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(matches!(
        *harness.daemon.noesis.state.lock().await,
        super::State::Off
    ));
    assert_eq!(
        harness.settings_events(),
        2,
        "the enable and the disable are both announced"
    );
}

#[tokio::test]
async fn a_re_enable_during_the_grace_keeps_the_service() {
    let harness = Harness::new().await;
    harness.configure(enable(true)).await;
    harness.configure(enable(false)).await;
    let status = harness.configure(enable(true)).await;
    assert!(status.effective.enabled);
    tokio::time::sleep(super::DRAIN_POLL * 2).await;
    assert!(
        harness.daemon.noesis.gate().is_enabled(),
        "the stale drain must not stop it"
    );
    assert!(matches!(
        *harness.daemon.noesis.state.lock().await,
        super::State::Running(_)
    ));
}

fn complete(store: &dyn Store, agent: aether_core::id::AgentId, usage: Usage) {
    let kind = EventKind::InvocationCompleted {
        invocation: InvocationId::new(),
        snapshot: SnapshotId::new(),
        model: ModelRef::new("claude-code", "opus"),
        usage,
        stop_reason: None,
        duration_ms: 1,
    };
    let draft = EventDraft::new(Actor::System, kind)
        .agent(agent)
        .run(RunId::new());
    store.commit(Commit::new().event(draft)).expect("commit");
}

fn noesis_agent(harness: &Harness, role: &str, model: ModelRef) -> aether_core::agent::Agent {
    let spec = CreateAgentSpec {
        role: role.into(),
        model: Some(model),
        subsystem: Some(NOESIS_SUBSYSTEM.into()),
        ..Default::default()
    };
    harness
        .daemon
        .runtime
        .create_agent(spec, Actor::System)
        .expect("create")
        .0
}

#[tokio::test]
async fn today_s_spend_is_read_back_per_lane_from_the_usage_records() {
    let harness = Harness::new().await;
    harness.configure(enable(true)).await;
    let tutor = noesis_agent(
        &harness,
        "noesis.tutor",
        ModelRef::new("claude-code", "opus"),
    );
    let extractor = noesis_agent(
        &harness,
        "noesis.extractor",
        ModelRef::new("claude-code", "opus"),
    );
    let other_spec = CreateAgentSpec {
        role: "worker".into(),
        model: Some(ModelRef::new("claude-code", "opus")),
        ..Default::default()
    };
    let other = harness
        .daemon
        .runtime
        .create_agent(other_spec, Actor::System)
        .expect("create")
        .0;
    let store = harness.daemon.runtime.store().clone();
    let spent = |usd: f64, estimate: f64| Usage {
        input_tokens: 10,
        cost_usd: usd,
        estimated_cost_usd: Some(estimate),
        ..Usage::default()
    };
    complete(store.as_ref(), tutor.id, spent(0.25, 0.5));
    complete(store.as_ref(), extractor.id, spent(0.0, 0.125));
    complete(store.as_ref(), other.id, spent(8.0, 0.0));
    let seeded = budget::spent_today(store.as_ref(), budget::today()).expect("spend");
    assert_eq!(
        seeded.interactive.usd, 0.75,
        "billed plus the subscription estimate"
    );
    assert_eq!(seeded.background.usd, 0.125);
    assert_eq!(seeded.interactive.tokens, 10);
    let yesterday = budget::today().pred_opt().expect("yesterday");
    let tomorrow = budget::today().succ_opt().expect("tomorrow");
    assert!(yesterday < tomorrow);
    let none = budget::spent_today(store.as_ref(), tomorrow).expect("spend");
    assert_eq!(
        none.interactive.usd + none.background.usd,
        0.0,
        "a new day starts at zero"
    );
}

#[tokio::test]
async fn an_api_billed_link_is_refused_unless_api_billing_is_allowed() {
    let harness = Harness::new().await;
    harness.configure(enable(true)).await;
    let api = noesis_agent(
        &harness,
        "noesis.tutor",
        ModelRef::new("anthropic", "claude-sonnet-5-5"),
    );
    let subscription = noesis_agent(
        &harness,
        "noesis.tutor",
        ModelRef::new("claude-code", "opus"),
    );
    let state = harness.daemon.noesis.state.lock().await;
    let super::State::Running(live) = &*state else {
        panic!("running");
    };
    let accountant = live.noesis.accountant().clone();
    let budget = ServiceBudget::new(accountant.clone(), false, harness.daemon.models.clone());
    let refused = budget
        .admit(&api, NoesisLane::Interactive)
        .err()
        .expect("refused");
    assert!(
        matches!(&refused, NoesisRefusal::Exhausted(why) if why.contains("allow_api")),
        "{refused:?}"
    );
    assert!(budget.admit(&subscription, NoesisLane::Interactive).is_ok());
    let allowed = ServiceBudget::new(accountant, true, harness.daemon.models.clone());
    assert!(allowed.admit(&api, NoesisLane::Interactive).is_ok());
    drop(state);
    let status = harness
        .configure(NoesisSettingsPatch {
            billing: Some(BillingPatch {
                allow_api: Some(true),
            }),
            ..Default::default()
        })
        .await;
    assert!(status.settings.billing.allow_api);
}

#[tokio::test]
async fn a_closed_budget_admits_nothing_and_counts_what_it_admitted() {
    let harness = Harness::new().await;
    harness.configure(enable(true)).await;
    let tutor = noesis_agent(
        &harness,
        "noesis.tutor",
        ModelRef::new("claude-code", "opus"),
    );
    let state = harness.daemon.noesis.state.lock().await;
    let super::State::Running(live) = &*state else {
        panic!("running");
    };
    let charge = live
        .budget
        .admit(&tutor, NoesisLane::Interactive)
        .expect("admitted");
    assert_eq!(live.budget.close(), 1, "the unsettled admission is counted");
    let refused = live
        .budget
        .admit(&tutor, NoesisLane::Interactive)
        .err()
        .expect("closed");
    assert!(matches!(refused, NoesisRefusal::Busy(_)), "{refused:?}");
    charge.settle(&Usage::default());
    assert_eq!(live.budget.outstanding(), 0);
    live.budget.reopen();
    assert!(live.budget.admit(&tutor, NoesisLane::Interactive).is_ok());
}

#[tokio::test]
async fn a_settings_change_waits_for_turns_in_flight_and_writes_nothing() {
    let harness = Harness::new().await;
    harness.configure(enable(true)).await;
    let tutor = noesis_agent(
        &harness,
        "noesis.tutor",
        ModelRef::new("claude-code", "opus"),
    );
    let charge = {
        let state = harness.daemon.noesis.state.lock().await;
        let super::State::Running(live) = &*state else {
            panic!("running");
        };
        live.budget
            .admit(&tutor, NoesisLane::Interactive)
            .expect("admitted")
    };
    let before = harness.daemon.noesis.config.read_noesis().expect("read");
    let raise = NoesisSettingsPatch {
        budget: Some(BudgetPatch {
            interactive_usd_daily: Some(9.0),
            ..Default::default()
        }),
        ..Default::default()
    };
    let error = harness
        .send(Command::NoesisConfigure {
            patch: raise.clone(),
            idempotency_key: None,
        })
        .await
        .expect_err("a turn is in flight");
    assert_eq!(error.reason.as_deref(), Some("conflict"), "{error:?}");
    assert_eq!(
        harness.daemon.noesis.config.read_noesis().expect("read"),
        before
    );
    assert!(
        harness.daemon.noesis.gate().is_enabled(),
        "the running service still admits"
    );
    charge.settle(&Usage::default());
    let status = harness.configure(raise).await;
    assert_eq!(status.settings.budget.interactive_usd_daily, 9.0);
}

#[tokio::test]
async fn a_keyed_configure_replays_its_first_outcome() {
    let harness = Harness::new().await;
    let keyed = |enabled: bool, key: &str| Command::NoesisConfigure {
        patch: enable(enabled),
        idempotency_key: Some(key.into()),
    };
    let Reply::NoesisStatus(first) = harness.send(keyed(true, "k1")).await.expect("first") else {
        panic!("status");
    };
    assert!(first.enabled);
    harness.configure(enable(false)).await;
    let Reply::NoesisStatus(again) = harness.send(keyed(true, "k1")).await.expect("replay") else {
        panic!("status");
    };
    assert_eq!(again, first, "the retry answers with the recorded outcome");
    assert!(
        !harness.daemon.noesis.gate().is_enabled(),
        "and does not enable again"
    );
    let other = harness
        .send(keyed(false, "k1"))
        .await
        .expect_err("same key, other request");
    assert_ne!(other.code, ErrorCode::Unavailable, "{other:?}");
}
