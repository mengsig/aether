use super::*;
use aether_core::agent::{AgentConfig, AgentStats, AgentStatus, Role, Scope};
use aether_core::event::Actor;
use aether_core::id::{AgentId, RunId};
use aether_core::model::{MessageRole, ModelErrorClass};
use aether_model::testing::ScriptedClient;

fn agent() -> Agent {
    let at = now();
    Agent {
        id: AgentId::new(),
        parent: None,
        role: Role::new("worker"),
        name: "w".into(),
        scope: Scope {
            repository: Some("repo".into()),
            ..Default::default()
        },
        model: ModelRef::new("mock", "m"),
        fallbacks: Vec::new(),
        status: AgentStatus::Ready,
        current_task: None,
        inbox_cursor: 0,
        checkpoint: None,
        engine_session: None,
        engine_usage: None,
        engine_sent: None,
        status_since: None,
        archived: false,
        paused: false,
        paused_via: None,
        resume_wait: None,
        restarted_from: None,
        restarted_as: None,
        config: AgentConfig::default(),
        stats: AgentStats::default(),
        run: RunId::new(),
        created_at: at,
        updated_at: at,
    }
}

fn entry(seq: u64, role: MessageRole, text: &str) -> TranscriptEntry {
    TranscriptEntry {
        seq,
        at: now(),
        message: Message {
            role,
            content: vec![ContentBlock::text(text)],
        },
        invocation: None,
    }
}

fn good_json() -> String {
    serde_json::json!({
        "checkpoint": {
            "objective": "ship the feature",
            "state": "tests written, one failing",
            "decisions": ["use sqlite"],
            "constraints": ["no force push"],
            "open_loops": ["ask about auth"],
            "artifacts": [{"reference": "src/lib.rs", "note": "main entry"}]
        },
        "memory_candidates": [
            {"scope": "repository", "kind": "fact", "text": "The build runs through scripts/ci.sh", "importance": 0.8, "confidence": 0.9, "tags": ["build"]}
        ]
    })
    .to_string()
}

#[tokio::test]
async fn an_observer_can_stop_before_another_provider_request() {
    let client = Arc::new(ScriptedClient::with_responses([
        ScriptedClient::text("invalid JSON"),
        ScriptedClient::text(&good_json()),
    ]));
    let compactor = Compactor::new(client.clone(), ModelRef::new("mock", "m"));
    let a = agent();
    let entries = vec![entry(1, MessageRole::User, "ship")];
    let report = compactor
        .compact_observed(
            CompactionInputs {
                agent: &a,
                task: None,
                child_tasks: &[],
                previous: None,
                entries: &entries,
                trigger: None,
            },
            CancellationToken::new(),
            |attempt| {
                assert!(attempt.usage.is_some());
                assert_eq!(client.requests().len(), 1, "publish before retrying");
                ControlFlow::Break(())
            },
        )
        .await;
    assert!(matches!(report.result, Err(CompactionError::Stopped)));
    assert_eq!(report.attempts.len(), 1);
    assert_eq!(client.requests().len(), 1);
    assert_eq!(client.remaining(), 1, "the unused retry is still queued");
}

#[tokio::test]
async fn report_preserves_usage_for_repair_fallback_and_failure() {
    let first = Usage {
        input_tokens: 120,
        output_tokens: 15,
        cost_usd: 0.3,
        ..Default::default()
    };
    let second = Usage {
        input_tokens: 200,
        output_tokens: 20,
        cost_usd: 0.5,
        ..Default::default()
    };
    let mut malformed = ScriptedClient::text("not a checkpoint").expect("script");
    for event in &mut malformed {
        if let StreamEvent::Usage { usage } = event {
            *usage = first;
        }
    }
    // A stream can fail after reporting usage. The report must retain it
    // even when the response accumulator rejects the subsequent event.
    let interrupted = vec![
        StreamEvent::Usage { usage: second },
        StreamEvent::TextDelta {
            index: 99,
            text: "no block".into(),
        },
    ];
    let primary = Arc::new(ScriptedClient::with_responses([
        Ok(malformed),
        Ok(interrupted),
    ]));
    let fallback = Arc::new(ScriptedClient::with_responses([ScriptedClient::error(
        ModelErrorClass::Unknown,
        "unavailable",
    )]));
    let compactor = Compactor::new(primary, ModelRef::new("mock", "cheap"))
        .with_fallback(fallback, ModelRef::new("mock", "strong"));
    let a = agent();
    let entries = vec![entry(1, MessageRole::User, "ship")];
    let report = compactor
        .compact_report(
            CompactionInputs {
                agent: &a,
                task: None,
                child_tasks: &[],
                previous: None,
                entries: &entries,
                trigger: None,
            },
            CancellationToken::new(),
        )
        .await;
    assert!(report.result.is_err());
    assert_eq!(report.attempts.len(), 3);
    assert_eq!(report.attempts[0].usage, Some(first));
    assert_eq!(report.attempts[1].usage, Some(second));
    assert_eq!(report.attempts[2].usage, None);
    assert_eq!(report.attempts[2].model, ModelRef::new("mock", "strong"));
}

#[tokio::test]
async fn folds_transcript_into_validated_checkpoint_with_candidates() {
    let client = Arc::new(ScriptedClient::with_responses([ScriptedClient::text(
        &good_json(),
    )]));
    let compactor = Compactor::new(client.clone(), ModelRef::new("mock", "m"));
    let a = agent();
    let entries = vec![
        entry(1, MessageRole::User, "do the thing"),
        entry(2, MessageRole::Assistant, "doing it"),
    ];
    let task = Task::new("ship", Actor::user(), a.run, now());
    let out = compactor
        .compact(
            CompactionInputs {
                agent: &a,
                task: Some(&task),
                child_tasks: &[],
                previous: None,
                entries: &entries,
                trigger: None,
            },
            CancellationToken::new(),
        )
        .await
        .expect("compact");
    assert_eq!(out.checkpoint.version, 1);
    assert_eq!(out.checkpoint.provenance.from_seq, 1);
    assert_eq!(out.checkpoint.provenance.to_seq, 2);
    assert_eq!(out.checkpoint.draft.objective, "ship the feature");
    assert_eq!(out.memory_candidates.len(), 1);
    assert_eq!(
        out.memory_candidates[0].scope,
        MemoryScope::Repository { id: "repo".into() }
    );
    assert_eq!(out.attempts, 1);
    let req = &client.requests()[0];
    assert_eq!(req.effort, Some(aether_core::model::Effort::Medium));
    assert!(req.system[0].text.contains("compaction service"));
    assert!(req.messages[0].text().contains("do the thing"));
    assert!(req.messages[0].text().contains("\"required\""));
}

/// A folded message that had an image: the compaction request names the
/// image as text and carries no reference a model adapter would have to read.
#[tokio::test]
async fn a_folded_message_with_a_stored_image_names_it_in_text() {
    use aether_core::attachment::{AttachmentId, AttachmentRef};

    let client = Arc::new(ScriptedClient::with_responses([ScriptedClient::text(
        &good_json(),
    )]));
    let compactor = Compactor::new(client.clone(), ModelRef::new("mock", "m"));
    let a = agent();
    let mut asked = entry(1, MessageRole::User, "what is on this screen?");
    asked
        .message
        .content
        .push(ContentBlock::ImageRef(AttachmentRef {
            id: AttachmentId::of(b"pixels"),
            media_type: "image/png".into(),
            size_bytes: 6,
            name: Some("shot.png".into()),
        }));
    let entries = vec![asked, entry(2, MessageRole::Assistant, "a login form")];
    compactor
        .compact(
            CompactionInputs {
                agent: &a,
                task: None,
                child_tasks: &[],
                previous: None,
                entries: &entries,
                trigger: None,
            },
            CancellationToken::new(),
        )
        .await
        .expect("compact");
    let request = &client.requests()[0];
    let folded = request.messages[0].text();
    assert!(folded.contains("what is on this screen?\n[image: shot.png, 6 B]\n"));
    let blocks = request.messages.iter().flat_map(|m| &m.content);
    assert!(
        blocks
            .clone()
            .all(|block| matches!(block, ContentBlock::Text { .. })),
        "the compaction request is text only"
    );
    assert!(blocks.count() > 0);
}

#[tokio::test]
async fn retries_when_previous_open_loop_is_dropped() {
    let dropped = serde_json::json!({"checkpoint": {"objective": "o", "state": "s"}}).to_string();
    let fixed = serde_json::json!({"checkpoint": {"objective": "o", "state": "s", "resolved_open_loops": ["ask about auth"], "constraints": ["no force push"]}}).to_string();
    let client = Arc::new(ScriptedClient::with_responses([
        ScriptedClient::text(&dropped),
        ScriptedClient::text(&fixed),
    ]));
    let compactor = Compactor::new(client.clone(), ModelRef::new("mock", "m"));
    let a = agent();
    let previous = Checkpoint {
        id: CheckpointId::new(),
        agent_id: a.id,
        version: 1,
        draft: CheckpointDraft {
            objective: "o".into(),
            state: "s".into(),
            open_loops: vec!["ask about auth".into()],
            constraints: vec!["no force push".into()],
            ..Default::default()
        },
        provenance: CheckpointProvenance {
            from_seq: 1,
            to_seq: 5,
            source_snapshot: None,
            compactor_model: None,
            previous: None,
            trigger: None,
        },
        created_at: now(),
    };
    let entries = vec![
        entry(6, MessageRole::User, "x"),
        entry(7, MessageRole::Assistant, "y"),
    ];
    let out = compactor
        .compact(
            CompactionInputs {
                agent: &a,
                task: None,
                child_tasks: &[],
                previous: Some(&previous),
                entries: &entries,
                trigger: None,
            },
            CancellationToken::new(),
        )
        .await
        .expect("compact");
    assert_eq!(out.attempts, 2);
    assert_eq!(out.checkpoint.version, 2);
    assert_eq!(out.checkpoint.provenance.previous, Some(previous.id));
    let second = &client.requests()[1];
    assert!(
        second
            .messages
            .last()
            .map(|m| m.text())
            .unwrap_or_default()
            .contains("rejected")
    );
}

#[tokio::test]
async fn transport_failure_uses_fallback_with_configured_effort() {
    let primary = Arc::new(ScriptedClient::with_responses([Err(ModelError::new(
        ModelErrorClass::Unknown,
        "connection lost",
    ))]));
    let fallback = Arc::new(ScriptedClient::with_responses([ScriptedClient::text(
        &good_json(),
    )]));
    let compactor = Compactor::new(primary, ModelRef::new("mock", "cheap"))
        .with_fallback(fallback.clone(), ModelRef::new("mock", "strong"))
        .with_effort(aether_core::model::Effort::Low);
    let a = agent();
    let entries = vec![entry(1, MessageRole::User, "ship")];
    let result = compactor
        .compact(
            CompactionInputs {
                agent: &a,
                task: None,
                child_tasks: &[],
                previous: None,
                entries: &entries,
                trigger: None,
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(result.model.model, "strong");
    assert_eq!(result.attempts, 2);
    assert_eq!(
        fallback.requests()[0].effort,
        Some(aether_core::model::Effort::Low)
    );
}

#[tokio::test]
async fn falls_back_to_stronger_model_after_two_failures() {
    let bad = "not json at all";
    let primary = Arc::new(ScriptedClient::with_responses([
        ScriptedClient::text(bad),
        ScriptedClient::text(bad),
    ]));
    let fallback = Arc::new(ScriptedClient::with_responses([ScriptedClient::text(
        &good_json(),
    )]));
    let compactor = Compactor::new(primary, ModelRef::new("mock", "weak"))
        .with_fallback(fallback.clone(), ModelRef::new("mock", "strong"));
    let a = agent();
    let entries = vec![
        entry(1, MessageRole::User, "x"),
        entry(2, MessageRole::Assistant, "y"),
    ];
    let out = compactor
        .compact(
            CompactionInputs {
                agent: &a,
                task: None,
                child_tasks: &[],
                previous: None,
                entries: &entries,
                trigger: None,
            },
            CancellationToken::new(),
        )
        .await
        .expect("compact");
    assert_eq!(out.attempts, 3);
    assert_eq!(out.model, ModelRef::new("mock", "strong"));
    assert_eq!(fallback.requests().len(), 1);
}

#[tokio::test]
async fn model_errors_surface() {
    let client = Arc::new(ScriptedClient::with_responses([ScriptedClient::error(
        ModelErrorClass::Server,
        "boom",
    )]));
    let compactor = Compactor::new(client, ModelRef::new("mock", "m"));
    let a = agent();
    let entries = vec![entry(1, MessageRole::User, "x")];
    let err = compactor
        .compact(
            CompactionInputs {
                agent: &a,
                task: None,
                child_tasks: &[],
                previous: None,
                entries: &entries,
                trigger: None,
            },
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, CompactionError::Model(_)));
}

#[test]
fn fold_range_keeps_a_tail_starting_at_a_user_turn() {
    let entries: Vec<TranscriptEntry> = (1..=8)
        .map(|i| {
            entry(
                i,
                if i % 2 == 1 {
                    MessageRole::User
                } else {
                    MessageRole::Assistant
                },
                "m",
            )
        })
        .collect();
    let (fold, tail) = fold_range(&entries, 3).expect("range");
    assert_eq!(tail[0].message.role, MessageRole::User);
    assert_eq!(fold.len() + tail.len(), 8);
    assert!(fold.len() >= 4);
    assert!(fold_range(&entries[..2], 3).is_none());
    // A handoff keeps no tail: everything folds, however short.
    let (all, none) = fold_range(&entries, 0).expect("range");
    assert_eq!(all.len(), 8);
    assert!(none.is_empty());
    let (one, _) = fold_range(&entries[..1], 0).expect("range");
    assert_eq!(one.len(), 1);
    assert!(fold_range(&[], 0).is_none());
}

#[test]
fn fold_range_keeps_tool_results_with_their_calls() {
    let mut messages = vec![Message::user_text("do the task")];
    for i in 0..6 {
        messages.push(tool_calls(&[&format!("c{i}")]));
        messages.push(tool_result(&format!("c{i}")));
    }
    let entries = transcript_entries(messages);
    let (fold, tail) = fold_range(&entries, 6).expect("range");
    assert_eq!(fold.last().unwrap().seq, 5);
    assert_eq!(tail[0].message, tool_calls(&["c2"]));
    assert_tool_results_preserved(&entries);
}

#[test]
fn fold_range_keeps_multiple_results_from_one_call_message() {
    let entries = transcript_entries(vec![
        Message::user_text("do the task"),
        tool_calls(&["c1", "c2", "c3"]),
        tool_result("c1"),
        Message::user_text("also check the tests"),
        tool_result("c2"),
        tool_result("c3"),
        Message::assistant_text("done"),
    ]);
    assert_tool_results_preserved(&entries);
}

#[test]
fn fold_range_keeps_a_tail_made_only_of_tool_results() {
    let entries = transcript_entries(vec![
        Message::user_text("do the task"),
        tool_calls(&["c1", "c2", "c3"]),
        tool_result("c1"),
        tool_result("c2"),
        tool_result("c3"),
    ]);
    let (fold, tail) = fold_range(&entries, 3).expect("range");
    assert_eq!(fold.len(), 1);
    assert_eq!(tail[0].message, tool_calls(&["c1", "c2", "c3"]));
    assert_tool_results_preserved(&entries);
    assert!(fold_range(&entries[1..], 3).is_none());
}

fn tool_calls(ids: &[&str]) -> Message {
    Message::assistant_blocks(
        ids.iter()
            .map(|id| ContentBlock::ToolUse {
                id: (*id).into(),
                name: "shell".into(),
                input: serde_json::json!({}),
            })
            .collect(),
    )
}

fn tool_result(id: &str) -> Message {
    Message::user_blocks(vec![ContentBlock::tool_result_text(
        id,
        format!("OUTPUT-{id}"),
        false,
    )])
}

fn transcript_entries(messages: Vec<Message>) -> Vec<TranscriptEntry> {
    messages
        .into_iter()
        .enumerate()
        .map(|(i, message)| TranscriptEntry {
            seq: i as u64 + 1,
            at: now(),
            message,
            invocation: None,
        })
        .collect()
}

fn assert_tool_results_preserved(entries: &[TranscriptEntry]) {
    let compactor = Compactor::new(
        Arc::new(ScriptedClient::default()),
        ModelRef::new("mock", "m"),
    );
    let a = agent();
    for keep_tail in 0..entries.len() {
        let Some((fold, tail)) = fold_range(entries, keep_tail) else {
            continue;
        };
        let prompt = compactor.render_request(&CompactionInputs {
            agent: &a,
            task: None,
            child_tasks: &[],
            previous: None,
            entries: fold,
            trigger: None,
        });
        let sanitized =
            crate::transcript::sanitize(tail.iter().map(|entry| entry.message.clone()).collect());
        for (i, entry) in entries.iter().enumerate() {
            for block in &entry.message.content {
                let ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } = block
                else {
                    continue;
                };
                let call = entries
                    .iter()
                    .position(|entry| {
                        entry.message.content.iter().any(|block| {
                            matches!(block, ContentBlock::ToolUse { id, .. } if id == tool_use_id)
                        })
                    })
                    .expect("matching call");
                assert_eq!(
                    call < fold.len(),
                    i < fold.len(),
                    "split pair: {tool_use_id}, keep_tail={keep_tail}"
                );
                if i < fold.len() {
                    for result in content {
                        if let ToolResultBlock::Text { text } = result {
                            assert!(prompt.contains(text), "missing folded result: {text}");
                        }
                    }
                } else {
                    assert!(
                        sanitized
                            .iter()
                            .any(|message| message.content.contains(block)),
                        "missing raw result: {tool_use_id}"
                    );
                }
            }
        }
    }
}

#[test]
fn parses_fenced_and_wrapped_json() {
    let fenced = format!("```json\n{}\n```", good_json());
    assert!(parse_output(&fenced).is_ok());
    let wrapped = format!("Here you go:\n{}\nDone.", good_json());
    assert!(parse_output(&wrapped).is_ok());
    assert!(parse_output("nope").is_err());
}

#[test]
fn short_slices_starting_with_calls_fold_complete_pairs_forward() {
    for length in [7, 8] {
        let mut messages = vec![tool_calls(&["c1"]), tool_result("c1")];
        messages.extend((2..length).map(|_| Message::assistant_text("continuation")));
        let entries = transcript_entries(messages);
        let (fold, tail) = fold_range(&entries, 6).expect("short range makes progress");
        assert_eq!(fold.len(), 2);
        assert_eq!(tail.len(), length - 2);
        assert_tool_results_preserved(&entries);
    }
}

#[tokio::test]
async fn a_stalled_compactor_request_ends_with_a_timeout_error() {
    let client = Arc::new(ScriptedClient::with_responses([ScriptedClient::text(
        &good_json(),
    )]));
    client.set_delay(std::time::Duration::from_secs(30));
    let compactor = Compactor::new(client, ModelRef::new("mock", "slow"))
        .with_attempt_timeout(std::time::Duration::from_millis(50));
    let a = agent();
    let entries = vec![entry(1, MessageRole::User, "ship")];
    let started = std::time::Instant::now();
    let report = compactor
        .compact_report(
            CompactionInputs {
                agent: &a,
                task: None,
                child_tasks: &[],
                previous: None,
                entries: &entries,
                trigger: None,
            },
            CancellationToken::new(),
        )
        .await;
    assert!(report.result.is_err());
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    let error = report.attempts[0].error.as_ref().expect("timed out");
    assert_eq!(error.class, ModelErrorClass::Timeout);
}
