use super::*;
use aether_core::id::{AgentId, ApprovalId};
use aether_tui::app::transcript::{ApprovalScope, MemoryNote, QuestionState};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Deserialize)]
struct Fixture {
    version: u32,
    modes: BTreeMap<String, serde_json::Value>,
    rows: Vec<Row>,
    expect: BTreeMap<String, BTreeMap<String, String>>,
    scope: Vec<ScopeCase>,
}

#[derive(Deserialize)]
struct Row {
    id: String,
    kind: String,
}

#[derive(Deserialize)]
struct ScopeCase {
    mode: String,
    role: Option<String>,
    name: Option<String>,
    applies: bool,
}

#[test]
fn shared_fixture_matches_tui_presentation_and_scope() {
    let fixture: Fixture =
        serde_json::from_str(include_str!("../fixtures/view_mode_parity.json")).unwrap();
    assert_eq!(fixture.version, 1);
    assert_eq!(fixture.modes.len(), 5);
    let catalog = Catalog::default();
    let definitions: BTreeMap<_, Option<Definition>> = fixture
        .modes
        .iter()
        .map(|(name, value)| {
            let definition = match name.as_str() {
                "standard" => {
                    assert!(value.is_null());
                    None
                }
                "focus" => {
                    // The desktop reads focus from this copy; it must stay the built-in itself.
                    let copy: Definition = serde_json::from_value(value.clone()).unwrap();
                    assert_eq!(copy, catalog.modes["focus"].definition);
                    Some(copy)
                }
                _ => Some(serde_json::from_value(value.clone()).unwrap()),
            };
            (name.as_str(), definition)
        })
        .collect();
    assert_eq!(fixture.expect.len(), definitions.len());
    let row_ids: BTreeSet<_> = fixture.rows.iter().map(|row| &row.id).collect();
    assert_eq!(row_ids.len(), fixture.rows.len(), "unique fixture row ids");
    for (name, definition) in &definitions {
        let policy = Policy {
            definition: definition.as_ref(),
        };
        let expected = &fixture.expect[*name];
        assert_eq!(expected.keys().collect::<BTreeSet<_>>(), row_ids);
        for row in &fixture.rows {
            let model = representative(&row.kind);
            let actual = outcome(policy, &model);
            assert_eq!(
                actual, expected[&row.id],
                "{name}: {} ({})",
                row.id, row.kind
            );
        }
    }
    for case in &fixture.scope {
        let applies = definitions[case.mode.as_str()]
            .as_ref()
            .is_none_or(|definition| {
                definition.matches(case.name.as_deref(), case.role.as_deref())
            });
        assert_eq!(applies, case.applies, "scope for {}", case.mode);
    }
}

fn outcome(policy: Policy<'_>, model: &CellModel) -> &'static str {
    if !policy.shows(model) {
        return "hidden";
    }
    if policy.failed_tool_line(model) {
        return "line";
    }
    if policy.compact(model) {
        "compact"
    } else {
        "full"
    }
}

fn representative(kind: &str) -> CellModel {
    match kind {
        "header" => CellModel::Header {
            version: "test".into(),
            cwd: "/workspace".into(),
            needs_sign_in: false,
            sign_in_details: vec![],
            welcome: None,
        },
        "user_operator" | "user_task" | "user_message" => {
            let from = match kind {
                "user_operator" => UserFrom::Operator,
                "user_task" => UserFrom::Task {
                    from: "parent".into(),
                },
                _ => UserFrom::Message {
                    from: "worker".into(),
                },
            };
            CellModel::User {
                from,
                text: "hello".into(),
                images: vec![],
            }
        }
        "assistant_text" => CellModel::Assistant {
            text: "answer".into(),
        },
        "post" => CellModel::Post {
            text: "**Item shipped**".into(),
        },
        "reasoning" => CellModel::Reasoning {
            title: None,
            text: "thinking".into(),
            duration_ms: None,
        },
        "tool_succeeded" => exec(CallStatus::Succeeded),
        "tool_running" => exec(CallStatus::Running),
        "tool_awaiting" => exec(CallStatus::Awaiting),
        "tool_failed" => exec(CallStatus::Failed),
        "tool_denied" => exec(CallStatus::Denied),
        "tool_cancelled" => exec(CallStatus::Cancelled),
        "edit_failed" => CellModel::Edited {
            files: vec![],
            status: CallStatus::Failed,
        },
        "memory" => CellModel::Memory(MemoryNote::Recalled {
            names: vec!["fact".into()],
        }),
        "spawned" | "waiting" | "child_finished" | "child_failed" | "sent" => coordination(kind),
        _ => decision_or_notice(kind),
    }
}

fn coordination(kind: &str) -> CellModel {
    let agent = AgentId::new();
    match kind {
        "spawned" => CellModel::Spawned {
            agent,
            name: "worker".into(),
            role: "review".into(),
            model: "mock".into(),
            effort: None,
            objective: "verify".into(),
        },
        "waiting" => CellModel::Waiting {
            on: vec![(agent, "worker".into())],
            since: chrono::Utc::now(),
            ended: None,
        },
        "child_finished" | "child_failed" => CellModel::ChildResult {
            agent,
            name: "worker".into(),
            role: "review".into(),
            outcome: if kind == "child_finished" {
                ChildOutcome::Finished
            } else {
                ChildOutcome::Failed {
                    error: "failed".into(),
                    quota: None,
                }
            },
            elapsed_ms: 1000,
            cost_with_children: None,
            result: "result".into(),
            descendants: vec![],
        },
        "sent" => CellModel::Sent {
            to: "worker".into(),
            text: "hello".into(),
        },
        _ => panic!("unknown coordination kind: {kind}"),
    }
}

fn decision_or_notice(kind: &str) -> CellModel {
    match kind {
        "approval" => CellModel::ApprovalRecord {
            approval: ApprovalId::new(),
            approved: false,
            what: "run command".into(),
            scope: ApprovalScope::Once,
        },
        "question" => CellModel::Question {
            asker: AgentId::new(),
            asker_name: "worker".into(),
            text: "ready?".into(),
            state: QuestionState::Pending,
            seq: 1,
        },
        "error" => CellModel::Error {
            message: "failed".into(),
            cause: None,
            hint: None,
        },
        "notice_error" => CellModel::Notice {
            kind: NoticeKind::Backoff {
                incident: "provider".into(),
            },
            text: "retrying".into(),
        },
        "notice_status" => CellModel::Notice {
            kind: NoticeKind::Reconnected,
            text: "connected".into(),
        },
        "compaction" => CellModel::Compaction(CompactionState::Done {
            before: Some(1000),
            after: Some(100),
            by: None,
            duration_ms: None,
        }),
        "compaction_failed" => CellModel::Compaction(CompactionState::Failed {
            error: "failed".into(),
        }),
        "usage" => CellModel::Separator {
            elapsed_ms: 1000,
            cost: None,
            agents: 1,
            finished_at: chrono::Utc::now(),
        },
        _ => panic!("unknown fixture row kind: {kind}"),
    }
}

#[test]
fn shared_reserved_names_match_every_tui_command_and_alias() {
    let expected: BTreeSet<_> = aether_tui::commands::COMMANDS
        .iter()
        .flat_map(|command| {
            std::iter::once(command.name).chain(command.hidden_aliases.iter().copied())
        })
        .chain(["standard", "focus", "reload"])
        .collect();
    let names = aether_core::view_modes::RESERVED_NAMES;
    let actual: BTreeSet<_> = names.iter().copied().collect();
    assert_eq!(actual.len(), names.len(), "no duplicate reserved names");
    assert_eq!(actual, expected);
}
