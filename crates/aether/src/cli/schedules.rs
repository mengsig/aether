use aether_client::Client;
use aether_core::agent::{AgentConfig, NetworkPolicy, Role};
use aether_core::model::{Effort, ModelChain};
use aether_core::schedule::{
    BusyPolicy, CreateScheduleSpec, MissedRunPolicy, Recurrence, Schedule, ScheduleAction,
    ScheduleMode, ScheduleOccurrence, ScheduleOutcome, ScheduleStatus, ScheduleTarget, SpawnSpec,
};
use anyhow::{anyhow, bail};
use std::path::PathBuf;

use super::execution::run_scope;
use super::{acted, connect, parse_backend, sane, short};
use crate::ScheduleCmd;

/// Newest history entries `schedule list` reads per schedule; the daemon
/// serves at most 1000.
const SKIP_SCAN: usize = 1000;

/// What a spawn schedule's runs get, from `schedule create --objective`.
struct Template {
    objective: String,
    role: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    token_budget: Option<u64>,
    cost_budget: Option<f64>,
    max_turns: Option<u32>,
    backend: Option<String>,
    network: Option<bool>,
    workspace: Option<PathBuf>,
}

pub(crate) async fn run(
    socket: impl Into<aether_client::Endpoint>,
    command: ScheduleCmd,
) -> anyhow::Result<()> {
    let socket = socket.into();
    // Before connecting: the client's own directory means nothing on the host.
    if let ScheduleCmd::Create {
        objective: Some(_),
        workspace: None,
        ..
    } = &command
    {
        anyhow::ensure!(
            !socket.is_remote(),
            "a remote spawn schedule requires --workspace /path/on/host"
        );
    }
    let client = connect(&socket).await?;
    match command {
        ScheduleCmd::Create {
            name,
            target,
            objective,
            workspace,
            role,
            model,
            effort,
            token_budget,
            cost_budget,
            max_turns,
            backend,
            network,
            every,
            cron,
            timezone,
            skip_missed,
            queue_one,
            payload,
            max_deliveries,
            json,
        } => {
            let recurrence = match (every, cron, timezone) {
                (Some(seconds), None, None) => Recurrence::Interval { seconds },
                (None, Some(expression), timezone) => Recurrence::Cron {
                    expression,
                    timezone: timezone.unwrap_or_else(|| "UTC".into()),
                },
                _ => bail!("choose exactly one of --every or --cron"),
            };
            let target = match (target, objective) {
                (Some(target), None) => {
                    ScheduleTarget::Agent(target.resolve(&client, &socket).await?)
                }
                (None, Some(objective)) => {
                    let template = Template {
                        objective,
                        role,
                        model,
                        effort,
                        token_budget,
                        cost_budget,
                        max_turns,
                        backend,
                        network,
                        workspace,
                    };
                    ScheduleTarget::Spawn(spawn_spec(&client, &socket, template).await?)
                }
                _ => bail!("choose exactly one of --target or --objective"),
            };
            let job = client
                .create_schedule(CreateScheduleSpec {
                    name,
                    target,
                    recurrence,
                    missed: if skip_missed {
                        MissedRunPolicy::Skip
                    } else {
                        MissedRunPolicy::Coalesce
                    },
                    busy: if queue_one {
                        BusyPolicy::QueueOne
                    } else {
                        BusyPolicy::Skip
                    },
                    payload: payload
                        .map(|payload| {
                            serde_json::from_str(&payload)
                                .unwrap_or(serde_json::Value::String(payload))
                        })
                        .unwrap_or_default(),
                    max_deliveries,
                })
                .await?;
            acted(json, &describe(&job), serde_json::json!(job))
        }
        ScheduleCmd::List { target, json } => {
            let target = match target {
                Some(target) => Some(target.resolve(&client, &socket).await?),
                None => None,
            };
            let jobs = client.schedules(target).await?;
            let text = if jobs.is_empty() {
                "No schedules.".into()
            } else if json {
                String::new()
            } else {
                list_lines(&client, &jobs).await?
            };
            acted(json, &text, serde_json::json!(jobs))
        }
        ScheduleCmd::Inspect { id, json } => {
            let job = client.schedule(id.resolve(&client).await?).await?;
            acted(json, &describe(&job), serde_json::json!(job))
        }
        ScheduleCmd::Pause { id, json } => {
            let job = client
                .update_schedule(id.resolve(&client).await?, ScheduleAction::Pause)
                .await?;
            acted(json, &summary(&job), serde_json::json!(job))
        }
        ScheduleCmd::Resume { id, json } => {
            let job = client
                .update_schedule(id.resolve(&client).await?, ScheduleAction::Resume)
                .await?;
            acted(json, &summary(&job), serde_json::json!(job))
        }
        ScheduleCmd::Cancel { id, json } => {
            let job = client
                .update_schedule(id.resolve(&client).await?, ScheduleAction::Cancel)
                .await?;
            acted(json, &summary(&job), serde_json::json!(job))
        }
        ScheduleCmd::History { id, limit, json } => {
            let history = client
                .schedule_history(id.resolve(&client).await?, limit)
                .await?;
            let text = history
                .iter()
                .map(|entry| {
                    format!(
                        "{}  {}",
                        entry.due.format("%Y-%m-%d %H:%M:%S UTC"),
                        outcome(&entry.outcome)
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            acted(
                json,
                if text.is_empty() {
                    "No recorded occurrences."
                } else {
                    &text
                },
                serde_json::json!(history),
            )
        }
    }
}

/// Same defaults and workspace rules as `aether run`: rooted in the directory
/// the person is in, or in `--workspace` as the host resolves it.
async fn spawn_spec(
    client: &Client,
    endpoint: &aether_client::Endpoint,
    template: Template,
) -> anyhow::Result<SpawnSpec> {
    let chain = template
        .model
        .map(|m| ModelChain::parse(&m).ok_or_else(super::model_format_error))
        .transpose()?;
    let effort = template
        .effort
        .map(|e| e.parse::<Effort>().map_err(|e| anyhow!(e)))
        .transpose()?;
    let backend = parse_backend(template.backend.as_deref().unwrap_or("host"))?;
    let network = match template.network {
        Some(true) => NetworkPolicy::Allowed,
        Some(false) => NetworkPolicy::Denied,
        None => client.status().await?.network_default,
    };
    let config = AgentConfig {
        effort,
        token_budget: template.token_budget,
        cost_budget: template.cost_budget,
        max_turns: template.max_turns,
        ..Default::default()
    };
    Ok(SpawnSpec {
        role: template.role.unwrap_or_else(|| Role::ASSISTANT.into()),
        name: None,
        objective: template.objective,
        model: chain.as_ref().map(|c| c.primary.clone()),
        fallbacks: chain.map(|c| c.fallbacks).unwrap_or_default(),
        scope: Some(run_scope(client, endpoint, template.workspace, backend, network).await?),
        config: (config != AgentConfig::default()).then_some(config),
        approval: None,
    })
}

/// One line per schedule, each ending with how many of its occurrences were
/// skipped, so a paused or busy target does not lose triggers unseen.
async fn list_lines(client: &Client, jobs: &[Schedule]) -> anyhow::Result<String> {
    let mut lines = Vec::with_capacity(jobs.len());
    for job in jobs {
        let history = client.schedule_history(job.id, SKIP_SCAN).await?;
        lines.push(format!("{}{}", summary(job), skipped_note(&history)));
    }
    Ok(lines.join("\n"))
}

/// ` · N skipped` for the occurrences among the newest `SKIP_SCAN` recorded
/// that were skipped, `N+` when the scan may have missed older ones.
fn skipped_note(history: &[ScheduleOccurrence]) -> String {
    assert!(
        history.len() <= SKIP_SCAN,
        "the daemon returned more history than asked for"
    );
    let skipped = history
        .iter()
        .filter(|entry| matches!(entry.outcome, ScheduleOutcome::Skipped { .. }))
        .count();
    match (skipped, history.len() == SKIP_SCAN) {
        (0, _) => String::new(),
        (n, true) => format!(" · {n}+ skipped"),
        (n, false) => format!(" · {n} skipped"),
    }
}

fn describe(job: &Schedule) -> String {
    let recurrence = match &job.recurrence {
        Recurrence::Interval { seconds } => format!("every {seconds} seconds"),
        Recurrence::Cron {
            expression,
            timezone,
        } => format!("{expression} ({timezone})"),
    };
    let missed = match job.missed {
        MissedRunPolicy::Coalesce => "deliver one trigger for an overdue window",
        MissedRunPolicy::Skip => "skip overdue windows",
    };
    let mut lines = vec![summary(job)];
    match &job.mode {
        ScheduleMode::Deliver { target, .. } => {
            lines.push(format!("target: {target}"));
            lines.push(format!(
                "while busy: {}",
                match job.busy {
                    BusyPolicy::Skip => "skip the occurrence",
                    BusyPolicy::QueueOne => "deliver once the previous trigger is read",
                }
            ));
        }
        ScheduleMode::Spawn {
            template,
            last_spawned,
        } => {
            let model = template
                .model
                .as_ref()
                .map_or_else(|| "daemon default".into(), ToString::to_string);
            let root = template
                .scope
                .as_ref()
                .and_then(|scope| scope.workspace_root.as_ref())
                .map_or_else(
                    || "the daemon's default root".into(),
                    |root| root.display().to_string(),
                );
            lines.push(format!(
                "spawns: {} on {} in {}",
                sane(&template.role),
                sane(&model),
                sane(&root)
            ));
            lines.push(format!("objective: {}", short(&template.objective, 160)));
            if let Some(agent) = last_spawned {
                lines.push(format!("last run: {agent}"));
            }
            lines.push("while busy: skip until the previous run ends".into());
        }
    }
    lines.push(format!("recurrence: {recurrence}"));
    lines.push(format!("missed runs: {missed}"));
    lines.push(format!("payload: {}", sane(&job.payload.to_string())));
    lines.join("\n")
}

fn summary(job: &Schedule) -> String {
    let state = match &job.status {
        ScheduleStatus::Active { next_due } => {
            format!("active · next {}", next_due.format("%Y-%m-%d %H:%M:%S UTC"))
        }
        ScheduleStatus::Paused { next_due } => {
            format!("paused · due {}", next_due.format("%Y-%m-%d %H:%M:%S UTC"))
        }
        ScheduleStatus::Cancelled { .. } => "cancelled".into(),
        ScheduleStatus::Exhausted { .. } => match job.mode {
            ScheduleMode::Deliver { .. } => "delivery limit reached".into(),
            ScheduleMode::Spawn { .. } => "run limit reached".into(),
        },
        ScheduleStatus::Failed { error, .. } => format!("failed · {}", short(error, 120)),
    };
    let limit = job
        .max_deliveries
        .map(|limit| format!(" / {limit}"))
        .unwrap_or_default();
    let done = match job.mode {
        ScheduleMode::Deliver { .. } => "delivered",
        ScheduleMode::Spawn { .. } => "started",
    };
    let last = job
        .last
        .as_ref()
        .map(|last| format!(" · last {}", outcome(&last.outcome)))
        .unwrap_or_default();
    format!(
        "{}  {} · {} · {}{} {done}{last}",
        job.id,
        short(&job.name, 80),
        state,
        job.deliveries,
        limit
    )
}

fn outcome(outcome: &ScheduleOutcome) -> String {
    match outcome {
        ScheduleOutcome::Delivered { inbox } => format!("delivered · {inbox}"),
        ScheduleOutcome::Spawned { agent, run, .. } => format!("started {agent} · run {run}"),
        ScheduleOutcome::Skipped { reason } => format!("skipped · {}", short(reason, 120)),
        ScheduleOutcome::Failed { error } => format!("failed · {}", short(error, 120)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DELIVER: &str = r#"{
        "id": "sch_01J00000000000000000000000",
        "name": "nightly",
        "owner": {"actor": "user"},
        "target": "agt_01J00000000000000000000001",
        "run": "run_01J00000000000000000000002",
        "recurrence": {"kind": "interval", "seconds": 3600},
        "missed": "coalesce",
        "busy": "queue_one",
        "payload": {"check": "ci"},
        "status": {"state": "active", "next_due": "2026-09-11T10:00:00Z"},
        "max_deliveries": 5,
        "deliveries": 2,
        "last_inbox_seq": 7,
        "last": {
            "schedule": "sch_01J00000000000000000000000",
            "due": "2026-09-11T09:00:00Z",
            "recorded_at": "2026-09-11T09:00:01Z",
            "outcome": {"outcome": "skipped", "reason": "target is paused"}
        },
        "created_at": "2026-09-11T08:00:00Z",
        "updated_at": "2026-09-11T09:00:01Z"
    }"#;

    const SPAWN: &str = r#"{
        "id": "sch_01J00000000000000000000003",
        "name": "sweep",
        "owner": {"actor": "user"},
        "spawn": {
            "role": "coordinator",
            "objective": "Sweep stale branches.\nThen report.",
            "model": {"provider": "mock", "model": "m1"},
            "scope": {"workspace_root": "/srv/repo"}
        },
        "last_spawned": "agt_01J00000000000000000000004",
        "recurrence": {"kind": "cron", "expression": "0 3 * * *", "timezone": "UTC"},
        "missed": "skip",
        "payload": null,
        "status": {"state": "paused", "next_due": "2026-09-12T03:00:00Z"},
        "max_deliveries": null,
        "deliveries": 1,
        "last_inbox_seq": null,
        "last": {
            "schedule": "sch_01J00000000000000000000003",
            "due": "2026-09-11T03:00:00Z",
            "recorded_at": "2026-09-11T03:00:00Z",
            "outcome": {
                "outcome": "spawned",
                "agent": "agt_01J00000000000000000000004",
                "task": "tsk_01J00000000000000000000005",
                "run": "run_01J00000000000000000000006"
            }
        },
        "created_at": "2026-09-10T08:00:00Z",
        "updated_at": "2026-09-11T03:00:00Z"
    }"#;

    fn job(json: &str) -> Schedule {
        serde_json::from_str(json).expect("fixture parses")
    }

    #[test]
    fn delivery_schedules_show_target_busy_policy_and_last_outcome() {
        let text = describe(&job(DELIVER));
        assert_eq!(
            text,
            "sch_01J00000000000000000000000  nightly · active · next 2026-09-11 10:00:00 UTC · \
             2 / 5 delivered · last skipped · target is paused\n\
             target: agt_01J00000000000000000000001\n\
             while busy: deliver once the previous trigger is read\n\
             recurrence: every 3600 seconds\n\
             missed runs: deliver one trigger for an overdue window\n\
             payload: {\"check\":\"ci\"}"
        );
    }

    #[test]
    fn spawn_schedules_show_template_last_run_and_history_outcome() {
        let job = job(SPAWN);
        let text = describe(&job);
        assert_eq!(
            text,
            "sch_01J00000000000000000000003  sweep · paused · due 2026-09-12 03:00:00 UTC · \
             1 started · last started agt_01J00000000000000000000004 · \
             run run_01J00000000000000000000006\n\
             spawns: coordinator on mock/m1 in /srv/repo\n\
             objective: Sweep stale branches. Then report.\n\
             last run: agt_01J00000000000000000000004\n\
             while busy: skip until the previous run ends\n\
             recurrence: 0 3 * * * (UTC)\n\
             missed runs: skip overdue windows\n\
             payload: null"
        );
    }

    fn occurrence(outcome: ScheduleOutcome) -> ScheduleOccurrence {
        let job = job(DELIVER);
        ScheduleOccurrence {
            schedule: job.id,
            due: job.created_at,
            recorded_at: job.created_at,
            outcome,
        }
    }

    #[test]
    fn list_counts_the_skipped_occurrences() {
        let skipped = || {
            occurrence(ScheduleOutcome::Skipped {
                reason: "target is paused".into(),
            })
        };
        let failed = occurrence(ScheduleOutcome::Failed { error: "x".into() });
        assert_eq!(skipped_note(&[]), "");
        assert_eq!(skipped_note(std::slice::from_ref(&failed)), "");
        assert_eq!(
            skipped_note(&[skipped(), failed, skipped()]),
            " · 2 skipped"
        );
        let full = vec![skipped(); SKIP_SCAN];
        assert_eq!(skipped_note(&full), format!(" · {SKIP_SCAN}+ skipped"));
    }

    #[test]
    fn schedule_payloads_cannot_steer_the_terminal() {
        // JSON escapes C0 controls itself; C1 CSI and bidi overrides pass through it.
        let mut job = job(DELIVER);
        job.payload = serde_json::json!({"note": "a\u{9b}2Jb\u{202e}c"});
        let text = describe(&job);
        assert!(text.ends_with("payload: {\"note\":\"a2Jbc\"}"), "{text}");
    }

    #[test]
    fn schedule_text_uses_whole_seconds_but_json_preserves_precision() {
        for fixture in [DELIVER, SPAWN] {
            let fixture = fixture.replace("00Z", "00.123456789Z");
            let job = job(&fixture);
            for text in [summary(&job), describe(&job)] {
                assert!(text.contains(":00 UTC"), "{text}");
                assert!(!text.contains("123456789"), "{text}");
            }
            assert!(serde_json::to_string(&job).unwrap().contains(".123456789Z"));
        }
    }

    #[test]
    fn an_exhausted_schedule_names_the_limit_its_mode_counts() {
        let exhausted = r#"{"state": "exhausted", "at": "2026-09-11T03:00:00Z"}"#;
        for (fixture, expected) in [
            (DELIVER, "delivery limit reached"),
            (SPAWN, "run limit reached"),
        ] {
            let mut job = job(fixture);
            job.status = serde_json::from_str(exhausted).expect("status parses");
            assert!(summary(&job).contains(expected), "{}", summary(&job));
        }
    }
}
