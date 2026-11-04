use std::sync::Arc;
use std::time::Duration;

use aether_core::event::{Actor, EventDraft, EventKind};
use aether_core::id::RunId;
use aether_core::model::{ModelRef, Usage};
use aether_protocol::noesis::{BillingPatch, BudgetPatch, DisablePatch, NoesisSettingsPatch};
use aether_protocol::{Command, ErrorCode, Reply};
use aether_runtime::CreateAgentSpec;
use aether_runtime::noesis_guard::{
    NOESIS_SUBSYSTEM, NoesisBudget, NoesisGate, NoesisLane, NoesisRefusal, usage_spent,
};
use aether_store::{Commit, Store};

use super::NoesisService;
use super::budget::{self, ServiceBudget};
use crate::Daemon;

pub(super) struct Harness {
    pub(super) daemon: Daemon,
    config: tempfile::TempDir,
    pub(super) data: tempfile::TempDir,
}

impl Harness {
    pub(super) async fn new() -> Self {
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

    pub(super) async fn send(
        &self,
        command: Command,
    ) -> Result<Reply, aether_protocol::ErrorReply> {
        let submission = crate::authz::AuthContext::Local.submission(&command)?;
        super::dispatch(&self.daemon, command, Actor::user(), submission).await
    }

    pub(super) async fn configure(
        &self,
        patch: NoesisSettingsPatch,
    ) -> aether_protocol::noesis::NoesisStatus {
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
    let sessions = serde_json::from_str(r#"{"cmd":"noesis_list_sessions"}"#).expect("command");
    let reply = harness
        .send(sessions)
        .await
        .expect("the enabled service answers");
    assert!(
        matches!(reply, Reply::NoesisSessions { ref sessions, .. } if sessions.is_empty()),
        "{reply:?}"
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

/// The record a Noesis turn writes with each usage it adds: the agent's
/// absolute total after it.
fn complete(store: &dyn Store, agent: aether_core::id::AgentId, total: Usage) {
    let kind = EventKind::AgentUsageUpdated {
        total,
        turn_usage: total,
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
    let desk = live.desk.lock().await;
    let noesis = desk.noesis.as_ref().expect("open");
    let accountant = noesis.accountant().clone();
    let mut settings = noesis.settings().clone();
    settings.billing_allow_api = false;
    let budget = ServiceBudget::new(accountant.clone(), &settings, harness.daemon.models.clone());
    // Admission reads a snapshot of the row; the rule binds the chain the turn runs.
    let on_snapshot = budget
        .admit(&api, NoesisLane::Interactive)
        .expect("admission does not judge the snapshot's chain");
    assert!(
        on_snapshot
            .refuses_model(&ModelRef::new("anthropic", "claude-sonnet-5-5"))
            .is_some_and(|why| why.contains("allow_api"))
    );
    on_snapshot.settle(&Usage::default());
    let charge = budget
        .admit(&subscription, NoesisLane::Interactive)
        .expect("admitted");
    assert!(
        charge
            .refuses_model(&ModelRef::new("anthropic", "claude-sonnet-5-5"))
            .is_some_and(|why| why.contains("allow_api")),
        "a model changed after admission is refused at its call"
    );
    assert_eq!(
        charge.refuses_model(&ModelRef::new("claude-code", "opus")),
        None
    );
    charge.settle(&Usage::default());
    settings.billing_allow_api = true;
    let allowed = ServiceBudget::new(accountant, &settings, harness.daemon.models.clone());
    assert!(allowed.admit(&api, NoesisLane::Interactive).is_ok());
    drop(desk);
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

/// A budget of $1 a day on each lane, nothing spent yet.
fn one_dollar_budget(harness: &Harness) -> ServiceBudget {
    let mut settings = aether_noesis::settings::NoesisSettings {
        enabled: true,
        ..Default::default()
    };
    settings.budget.interactive_usd_daily = 1.0;
    let accountant = aether_noesis::budget::Accountant::new(
        &settings,
        budget::today(),
        aether_noesis::budget::LaneSpend::default(),
    );
    ServiceBudget::new(accountant, &settings, harness.daemon.models.clone())
}

fn costing(usd: f64) -> Usage {
    Usage {
        cost_usd: usd,
        ..Usage::default()
    }
}

#[tokio::test]
async fn a_running_turn_s_spend_counts_against_the_budget_before_it_settles() {
    let harness = Harness::new().await;
    harness.configure(enable(true)).await;
    let tutor = noesis_agent(
        &harness,
        "noesis.tutor",
        ModelRef::new("claude-code", "opus"),
    );
    let budget = one_dollar_budget(&harness);
    let first = budget
        .admit(&tutor, NoesisLane::Interactive)
        .expect("admitted");
    assert_eq!(
        first.before_invocation(&costing(0.4)),
        None,
        "under the limit"
    );
    let second = budget
        .admit(&tutor, NoesisLane::Interactive)
        .expect("room left");
    let spent = first
        .before_invocation(&costing(1.0))
        .expect("the limit is reached mid-turn");
    assert!(spent.contains("budget"), "{spent}");
    let refused = budget
        .admit(&tutor, NoesisLane::Interactive)
        .err()
        .expect("spent out");
    assert!(
        matches!(refused, NoesisRefusal::Exhausted(_)),
        "{refused:?}"
    );
    assert!(
        second.before_invocation(&Usage::default()).is_some(),
        "the other turn stops too"
    );
    first.settle(&costing(1.0));
    second.settle(&Usage::default());
    assert_eq!(budget.outstanding(), 0);
    assert!(
        budget.admit(&tutor, NoesisLane::Interactive).is_err(),
        "the settled spend still counts"
    );
}

#[tokio::test]
async fn a_retired_budget_s_admissions_start_nothing() {
    let harness = Harness::new().await;
    harness.configure(enable(true)).await;
    let tutor = noesis_agent(
        &harness,
        "noesis.tutor",
        ModelRef::new("claude-code", "opus"),
    );
    let budget = one_dollar_budget(&harness);
    let charge = budget
        .admit(&tutor, NoesisLane::Interactive)
        .expect("admitted");
    assert!(charge.is_current());
    budget.retire();
    assert!(
        !charge.is_current(),
        "a retired service's claim starts no turn"
    );
    let refused = budget
        .admit(&tutor, NoesisLane::Interactive)
        .err()
        .expect("closed");
    assert!(matches!(refused, NoesisRefusal::Busy(_)), "{refused:?}");
    charge.settle(&Usage::default());
    assert_eq!(budget.outstanding(), 0);
}

#[tokio::test]
async fn settling_one_turn_never_shows_a_reader_less_than_the_spend_of_both() {
    let harness = Harness::new().await;
    harness.configure(enable(true)).await;
    let tutor = noesis_agent(
        &harness,
        "noesis.tutor",
        ModelRef::new("claude-code", "opus"),
    );
    let budget = Arc::new(one_dollar_budget(&harness));
    let a = budget
        .admit(&tutor, NoesisLane::Interactive)
        .expect("admitted");
    let b = budget
        .admit(&tutor, NoesisLane::Interactive)
        .expect("admitted");
    assert_eq!(
        a.before_invocation(&costing(0.6)),
        None,
        "A is under the limit"
    );
    assert!(
        b.before_invocation(&costing(0.4)).is_some(),
        "A + B reach $1"
    );

    let reader_budget = Arc::clone(&budget);
    let reader_agent = tutor.clone();
    let reader = std::thread::spawn(move || {
        for _ in 0..2_000 {
            let admitted = reader_budget.admit(&reader_agent, NoesisLane::Interactive);
            assert!(
                matches!(admitted, Err(NoesisRefusal::Exhausted(_))),
                "a reader saw less than $1 spent while A settled"
            );
        }
    });
    a.settle(&costing(0.6));
    b.settle(&costing(0.4));
    reader
        .join()
        .expect("the reader never saw a half-moved state");
    assert_eq!(budget.outstanding(), 0);
}

#[tokio::test]
async fn a_rebuild_counts_a_failed_invocation_and_a_compaction_as_settlement_did() {
    let harness = Harness::new().await;
    harness.configure(enable(true)).await;
    let tutor = noesis_agent(
        &harness,
        "noesis.tutor",
        ModelRef::new("claude-code", "opus"),
    );
    let store = harness.daemon.runtime.store().clone();
    let (failed, compaction) = (costing(0.3), costing(0.2));
    let after_failure = failed;
    let mut after_compaction = failed;
    after_compaction += compaction;
    complete(store.as_ref(), tutor.id, after_failure);
    complete(store.as_ref(), tutor.id, after_compaction);

    let settled = budget::spend_of(&usage_spent(&Usage::default(), &after_compaction));
    let rebuilt = budget::spent_today(store.as_ref(), budget::today()).expect("spend");
    assert_eq!(settled.usd, 0.5, "the turn charged both");
    assert_eq!(rebuilt.interactive.usd, settled.usd, "so does the rebuild");
    assert_eq!(rebuilt.background.usd, 0.0);
}

/// A topic concept with one exposure, written straight into `noesis.db`.
async fn seed_concept(harness: &Harness, title: &str) -> String {
    use aether_noesis::learner::{self, NewEvidence};
    use aether_protocol::noesis::{ConceptKind, EvidenceKind, EvidenceOutcome};
    let state = harness.daemon.noesis.state.lock().await;
    let super::State::Running(live) = &*state else {
        panic!("the service runs");
    };
    let desk = live.desk.lock().await;
    let conn = desk.noesis.as_ref().expect("open").store().connection();
    let now = chrono::Utc::now();
    let concept = learner::ensure_concept(conn, ConceptKind::Topic, title, title, now).unwrap();
    let exposure = NewEvidence {
        concept: concept.id.clone(),
        session: None,
        kind: EvidenceKind::Exposure,
        outcome: EvidenceOutcome::Exposed,
        confidence: None,
        mode: None,
        hint_rung: None,
        miss_class: None,
        note: None,
    };
    learner::append(conn, &[exposure], now).unwrap();
    concept.id
}

fn learner_events(harness: &Harness) -> Vec<Vec<String>> {
    let events = harness
        .daemon
        .runtime
        .store()
        .events(0, 10_000, &aether_store::EventFilter::default())
        .expect("events");
    let updated = events.into_iter().filter_map(|event| match event.kind {
        EventKind::NoesisLearnerUpdated { concepts } => Some(concepts),
        _ => None,
    });
    updated.collect()
}

#[tokio::test]
async fn the_learner_commands_read_and_correct_the_learner_model() {
    use aether_protocol::noesis::{ConceptStateKind, EvidenceKind};
    let harness = Harness::new().await;
    harness.configure(enable(true)).await;
    let concept = seed_concept(&harness, "Limits").await;
    let read = Command::NoesisLearner {
        concepts: None,
        query: Some("lim".into()),
        limit: None,
    };
    let Reply::NoesisLearner { concepts } = harness.send(read).await.expect("read") else {
        panic!("a learner reply");
    };
    assert_eq!(concepts.len(), 1);
    assert_eq!(concepts[0].state, ConceptStateKind::Exposed);
    let correct = |concept: &str| {
        let json = format!(
            r#"{{"cmd":"noesis_correct_learner","concept":"{concept}","state":"fragile","reason":"shaky","idempotency_key":"c1"}}"#
        );
        serde_json::from_str::<Command>(&json).expect("command")
    };
    let missing = harness
        .send(correct("k_missing"))
        .await
        .expect_err("unknown");
    assert_eq!(missing.code, ErrorCode::NotFound, "{missing:?}");
    let Reply::ConceptState(first) = harness.send(correct(&concept)).await.expect("correct") else {
        panic!("a concept state");
    };
    assert_eq!(first.state, ConceptStateKind::Fragile);
    let Reply::ConceptState(again) = harness.send(correct(&concept)).await.expect("retry") else {
        panic!("a concept state");
    };
    assert_eq!(again, first, "the retry replays the first correction");
    assert_eq!(learner_events(&harness), [vec![concept.clone()]]);
    let page = Command::NoesisLearnerEvidence {
        concept,
        before: None,
        limit: None,
    };
    let Reply::NoesisEvidence {
        evidence,
        next_before,
    } = harness.send(page).await.expect("page")
    else {
        panic!("an evidence page");
    };
    let kinds: Vec<_> = evidence.iter().map(|e| e.kind).collect();
    assert_eq!(
        kinds,
        [EvidenceKind::OperatorCorrection, EvidenceKind::Exposure]
    );
    assert_eq!(evidence[0].note.as_deref(), Some("fragile: shaky"));
    assert_eq!(next_before, None);
}
