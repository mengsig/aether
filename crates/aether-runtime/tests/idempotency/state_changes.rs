use super::*;
use aether_core::agent::{ApprovalPolicy, ArchiveTarget};
use aether_core::approval::{RuleMatch, RuleScope};
use aether_core::memory::{MemoryKind, OperatorMemoryScope, OperatorMemorySpec};
use aether_core::schedule::ScheduleAction;
use aether_core::tool::{ApprovalDecision, ApprovalStatus, SideEffectClass};
use aether_runtime::AgentUpdate;

fn key(command: &str, changed: bool) -> Submission {
    Submission::new(command, &json!({"cmd": command, "changed": changed})).unwrap()
}

fn memory_spec(agent: AgentId) -> OperatorMemorySpec {
    OperatorMemorySpec {
        text: "Run cargo test before submitting changes in this repository.".into(),
        kind: MemoryKind::Fact,
        scope: OperatorMemoryScope::Agent { id: agent },
        importance: None,
        confidence: None,
        tags: vec![],
        supersedes: None,
        proposed: false,
    }
}

fn pending(fx: &Fixture, agent: AgentId) -> ApprovalId {
    let approval = ApprovalRequest {
        id: ApprovalId::new(),
        agent_id: agent,
        task_id: None,
        call_id: ToolCallId::new(),
        tool: "bash".into(),
        description: "run tests".into(),
        argument: Some("cargo test".into()),
        resources: vec![],
        side_effect: SideEffectClass::External,
        requested_at: now(),
        status: ApprovalStatus::Pending,
    };
    fx.runtime
        .apply(Commit::new().mutate(Mutation::PutApproval(approval.clone())))
        .unwrap();
    assert!(fx.store.approval(approval.id).unwrap().is_some());
    assert_eq!(approval.agent_id, agent);
    approval.id
}

struct Targets {
    agent: AgentId,
    approval: ApprovalId,
    schedule: ScheduleId,
}

fn run(
    fx: &Fixture,
    command: &str,
    targets: &Targets,
    submission: Submission,
) -> aether_runtime::Result<serde_json::Value> {
    let rt = &fx.runtime;
    let id = targets.agent;
    let by = Actor::user();
    let key = Some(submission);
    let value = match command {
        "cancel" => json!(rt.cancel_agent_keyed(id, None, by, key)?),
        "interrupt" => json!(rt.interrupt_agent_keyed(id, by, key)?),
        "pause" => json!(rt.pause_agent_keyed(id, true, by, key)?),
        "resume" => json!(rt.resume_agent_keyed(id, true, by, key)?),
        "archive" => json!(rt.archive_agents_keyed(ArchiveTarget::Agent(id), true, by, key)?),
        "update_agent" => json!(rt.update_agent_keyed(
            id,
            AgentUpdate {
                name: Some("renamed".into()),
                ..Default::default()
            },
            by,
            key
        )?),
        "set_approvals" => {
            json!(rt.set_approvals_keyed(id, ApprovalPolicy::default(), true, by, key)?)
        }
        "restart" => json!(rt.restart_agent_keyed(id, by, key)?),
        "resolve_approval" => json!(rt.resolve_approval_keyed(
            targets.approval,
            ApprovalDecision::Approved,
            by,
            None,
            Some(aether_runtime::Remember::narrow(None)),
            key
        )?),
        "create_memory" => json!(rt.create_memory_keyed(memory_spec(id), by, key)?),
        "add_approval_rule" => json!(rt.add_approval_rule_keyed(
            ApprovalDecision::Approved,
            RuleMatch::tool("bash"),
            RuleScope::Agent { id },
            by,
            None,
            key
        )?),
        "update_schedule" => {
            json!(rt.update_schedule_keyed(targets.schedule, ScheduleAction::Pause, by, key)?)
        }
        other => panic!("missing test case {other}"),
    };
    Ok(value)
}

const STATE_CHANGING: [&str; 12] = [
    "cancel",
    "interrupt",
    "pause",
    "resume",
    "archive",
    "update_agent",
    "set_approvals",
    "restart",
    "resolve_approval",
    "create_memory",
    "add_approval_rule",
    "update_schedule",
];

/// The targets `command` acts on, with the agent put in the state it needs.
async fn targets_for(fx: &Fixture, command: &str) -> Targets {
    let agent = an_agent(fx).await;
    let approval = pending(fx, agent);
    let schedule = fx
        .runtime
        .create_schedule(schedule_spec(agent), Actor::user())
        .unwrap()
        .id;
    if command == "resume" {
        fx.runtime.pause_agent(agent, true, Actor::user()).unwrap();
    }
    if matches!(command, "archive" | "restart") {
        fx.runtime.cancel_agent(agent, None, Actor::user()).unwrap();
    }
    Targets {
        agent,
        approval,
        schedule,
    }
}

async fn replays_and_conflicts(command: &str) {
    let (fx, log) = logged().await;
    let targets = targets_for(&fx, command).await;
    log.take();
    let first = run(&fx, command, &targets, key(command, false)).unwrap();
    let commits = log.take();
    assert_eq!(
        commits.iter().filter(|c| records_submission(c)).count(),
        1,
        "{command}"
    );
    if command != "interrupt" {
        assert!(
            commits.iter().any(|c| records_submission(c) && c.len() > 1),
            "{command}: atomic work and receipt"
        );
    }
    let seq = fx.store.last_seq().unwrap();
    assert_eq!(
        run(&fx, command, &targets, key(command, false)).unwrap(),
        first,
        "{command}"
    );
    assert!(
        matches!(
            run(&fx, command, &targets, key(command, true)),
            Err(aether_runtime::RuntimeError::Invalid(_))
        ),
        "{command}"
    );
    assert_eq!(
        fx.store.last_seq().unwrap(),
        seq,
        "{command}: retries cannot mutate"
    );
    assert!(log.take().is_empty(), "{command}: replay cannot write");
    fx.runtime.shutdown().await;
}

#[tokio::test]
async fn state_changing_commands_replay_original_receipts_and_conflict() {
    for command in STATE_CHANGING {
        replays_and_conflicts(command).await;
    }
}

#[tokio::test]
async fn a_pause_receipt_stays_unchanged_after_resume_and_daemon_restart() {
    let fx = fixture().await;
    let agent = an_agent(&fx).await;
    let first = fx
        .runtime
        .pause_agent_keyed(agent, true, Actor::user(), Some(key("pause", false)))
        .unwrap();
    fx.runtime.resume_agent(agent, true, Actor::user()).unwrap();
    fx.runtime.shutdown().await;
    let restarted = over(fx.store.clone()).await;
    let replayed = restarted
        .runtime
        .pause_agent_keyed(agent, true, Actor::user(), Some(key("pause", false)))
        .unwrap();
    assert_eq!(json!(first), json!(replayed));
    assert!(!restarted.store.agent(agent).unwrap().unwrap().paused);
    restarted.runtime.shutdown().await;
}

async fn rolls_back_when_the_receipt_is_refused(command: &str) {
    let log = CommitLog::open(Refusal::Records);
    let fx = over(log.clone() as Arc<dyn Store>).await;
    let targets = targets_for(&fx, command).await;
    log.take();
    let agents = fx.store.agents(&AgentFilter::default()).unwrap();
    let seq = fx.store.last_seq().unwrap();
    assert!(
        run(&fx, command, &targets, key(command, false)).is_err(),
        "{command}"
    );
    assert_eq!(fx.store.last_seq().unwrap(), seq, "{command}");
    assert_eq!(
        fx.store.agents(&AgentFilter::default()).unwrap(),
        agents,
        "{command}"
    );
    assert!(
        fx.store
            .submission(&key(command, false).key)
            .unwrap()
            .is_none()
    );
    assert!(
        log.take().is_empty(),
        "{command}: failed receipt must not commit work"
    );
    fx.runtime.shutdown().await;
}

#[tokio::test]
async fn refusing_a_receipt_rolls_back_each_state_change() {
    for command in STATE_CHANGING {
        rolls_back_when_the_receipt_is_refused(command).await;
    }
}
