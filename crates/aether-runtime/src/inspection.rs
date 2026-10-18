//! Scoped, bounded projections of authoritative agent and task records.
use crate::{Result, Runtime, RuntimeError};
use aether_core::agent::{Agent, subtree_of};
use aether_core::id::{AgentId, ArtifactId, TaskId};
use aether_core::task::Task;
use aether_core::tool::ToolOutcome;
use aether_store::{AgentFilter, TaskFilter};
use aether_tools::builtin::inspection::{MAX_LIST_ITEMS, MAX_TASK_CHARS};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

fn scope(runtime: &Runtime, caller: AgentId) -> Result<(Vec<Agent>, BTreeSet<AgentId>)> {
    let agents = runtime.store.agents(&AgentFilter::default())?;
    let ids = subtree_of(&agents, caller).into_iter().collect();
    Ok((agents, ids))
}

fn visible(task: &Task, ids: &BTreeSet<AgentId>) -> bool {
    task.owner.is_some_and(|id| ids.contains(&id))
        || task.creator.agent_id().is_some_and(|id| ids.contains(&id))
}

pub(crate) fn outcome(value: Value) -> ToolOutcome {
    ToolOutcome {
        content: value.to_string(),
        structured: Some(value),
        ..Default::default()
    }
}

pub(crate) fn brief(text: &str, limit: usize) -> Value {
    json!({"text":text.chars().take(limit).collect::<String>(), "truncated":text.chars().count() > limit})
}

pub(crate) fn agents(
    runtime: &Runtime,
    caller: AgentId,
    after: Option<AgentId>,
    limit: usize,
) -> Result<ToolOutcome> {
    if !(1..=MAX_LIST_ITEMS).contains(&limit) {
        return Ok(ToolOutcome::error(format!(
            "limit must be between 1 and {MAX_LIST_ITEMS}"
        )));
    }
    let (agents, ids) = scope(runtime, caller)?;
    let mut agents: Vec<_> = agents
        .into_iter()
        .filter(|a| ids.contains(&a.id) && after.is_none_or(|after| a.id > after))
        .collect();
    agents.sort_by_key(|a| a.id);
    let more = agents.len() > limit;
    agents.truncate(limit);
    let next = more.then(|| agents.last().map(|a| a.id)).flatten();
    let rows: Vec<_> = agents
        .into_iter()
        .map(|a| {
            json!({
                "id":a.id, "parent":a.parent, "name":brief(&a.name,64),
                "status":a.status.kind(), "task":a.current_task,
                "model":a.model, "effort":a.config.effort,
            })
        })
        .collect();
    Ok(outcome(json!({"agents":rows,"next_after":next})))
}

pub(crate) fn tasks(
    runtime: &Runtime,
    caller: AgentId,
    after: Option<TaskId>,
    limit: usize,
) -> Result<ToolOutcome> {
    if !(1..=MAX_LIST_ITEMS).contains(&limit) {
        return Ok(ToolOutcome::error(format!(
            "limit must be between 1 and {MAX_LIST_ITEMS}"
        )));
    }
    let (_, ids) = scope(runtime, caller)?;
    let mut tasks: Vec<_> = runtime
        .store
        .tasks(&TaskFilter::default())?
        .into_iter()
        .filter(|t| visible(t, &ids) && after.is_none_or(|after| t.id > after))
        .collect();
    tasks.sort_by_key(|t| t.id);
    let more = tasks.len() > limit;
    tasks.truncate(limit);
    let next = more.then(|| tasks.last().map(|t| t.id)).flatten();
    let rows: Vec<_> = tasks.into_iter().map(|t| json!({
        "id":t.id, "owner":t.owner, "status":t.status.kind(), "objective":brief(&t.objective,160),
    })).collect();
    Ok(outcome(json!({"tasks":rows,"next_after":next})))
}

pub(crate) fn task(
    runtime: &Runtime,
    caller: AgentId,
    id: TaskId,
    revision: Option<&str>,
    offset: usize,
    max_chars: usize,
) -> Result<ToolOutcome> {
    if !(256..=MAX_TASK_CHARS).contains(&max_chars) {
        return Ok(ToolOutcome::error(format!(
            "max_chars must be between 256 and {MAX_TASK_CHARS}"
        )));
    }
    let (_, ids) = scope(runtime, caller)?;
    let Some(task) = runtime.store.task(id)?.filter(|task| visible(task, &ids)) else {
        return Ok(ToolOutcome::error(
            "Task not found in your delegated scope. Copy its task ID from delegate or list_tasks.",
        ));
    };
    let serialized = serde_json::to_string(&task).map_err(|error| {
        RuntimeError::Internal(format!("could not serialize task {id}: {error}"))
    })?;
    let current_revision = hex::encode(Sha256::digest(serialized.as_bytes()));
    if offset > 0 && revision.is_none() {
        return Ok(ToolOutcome::error(
            "Pass the revision returned by the first page when offset is nonzero.",
        ));
    }
    if revision.is_some_and(|revision| revision != current_revision) {
        return Ok(ToolOutcome::error(
            "Task record changed. Restart at offset 0 without a revision.",
        ));
    }
    let total_chars = serialized.chars().count();
    if offset > total_chars {
        return Ok(ToolOutcome::error(format!(
            "offset {offset} exceeds task record length {total_chars}"
        )));
    }
    let text: String = serialized.chars().skip(offset).take(max_chars).collect();
    let end = offset + text.chars().count();
    Ok(outcome(
        json!({"task":id,"revision":current_revision,"format":"json","offset":offset,"text":text,"total_chars":total_chars,"next_offset":(end < total_chars).then_some(end)}),
    ))
}

pub(crate) fn artifact(
    runtime: &Runtime,
    caller: AgentId,
    id: ArtifactId,
    offset: u64,
    limit: usize,
) -> Result<ToolOutcome> {
    use aether_tools::builtin::inspection::MAX_ARTIFACT_BYTES;
    if !(4..=MAX_ARTIFACT_BYTES).contains(&limit) {
        return Ok(ToolOutcome::error(format!(
            "limit must be between 4 and {MAX_ARTIFACT_BYTES}"
        )));
    }
    let (_, ids) = scope(runtime, caller)?;
    let Some(record) = runtime
        .store
        .artifact(id)?
        .filter(|a| a.agent_id.is_some_and(|id| ids.contains(&id)))
    else {
        return Ok(ToolOutcome::error(
            "Artifact not found in your delegated scope.",
        ));
    };
    if record.evicted_at.is_some() {
        return Ok(ToolOutcome::error(
            "Artifact content was evicted by retention.",
        ));
    }
    if offset > record.size_bytes {
        return Ok(ToolOutcome::error(format!(
            "offset {offset} exceeds artifact length {}",
            record.size_bytes
        )));
    }
    let Some(bytes) = runtime.store.read_artifact_range(id, offset, limit)? else {
        return Ok(ToolOutcome::error(
            "Artifact content is missing or was evicted.",
        ));
    };
    let text = match std::str::from_utf8(&bytes) {
        Ok(text) => text,
        Err(e) if e.error_len().is_none() && offset + (bytes.len() as u64) < record.size_bytes => {
            // Only a trailing, incomplete character may be deferred to the next page.
            std::str::from_utf8(&bytes[..e.valid_up_to()])
                .map_err(|e| RuntimeError::Internal(e.to_string()))?
        }
        Err(_) => {
            return Ok(ToolOutcome::error(
                "Artifact page is not UTF-8 text, or offset is inside a character. Use a next_offset returned by read_artifact.",
            ));
        }
    };
    if text.is_empty() && offset < record.size_bytes {
        return Ok(ToolOutcome::error(
            "Artifact content is incomplete; the requested page could not be read.",
        ));
    }
    let end = offset + text.len() as u64;
    Ok(outcome(
        json!({"id":id,"offset":offset,"text":text,"total_bytes":record.size_bytes,"next_offset":(end < record.size_bytes).then_some(end)}),
    ))
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::{RuntimeConfig, commands::CreateAgentSpec};
    use aether_core::agent::{AgentConfig, AgentStatus, ApprovalPolicy};
    use aether_core::event::Actor;
    use aether_core::model::ModelRef;
    use aether_core::task::{TaskResult, TaskStatus};
    use aether_model::{Models, testing::ScriptedClient};
    use aether_store::{Commit, Mutation, SqliteStore};
    use aether_tools::ToolRegistry;
    use std::sync::Arc;

    async fn runtime() -> (Arc<Runtime>, tempfile::TempDir) {
        let config = tempfile::tempdir().unwrap();
        let runtime = Runtime::start_with_engines(
            RuntimeConfig {
                default_model: Some(ModelRef::new("anthropic", "claude-sonnet-5-5")),
                ..Default::default()
            },
            Arc::new(SqliteStore::open_in_memory().unwrap()),
            Arc::new(Models::load(Some(config.path().into())).unwrap()),
            Arc::new(ToolRegistry::with_builtins()),
            Some(Arc::new(ScriptedClient::new())),
            Default::default(),
        )
        .await
        .unwrap();
        (runtime, config)
    }

    fn create(runtime: &Runtime, parent: Option<AgentId>) -> (Agent, Task) {
        let (agent, task) = runtime
            .create_agent(
                CreateAgentSpec {
                    role: "worker".into(),
                    objective: Some("inspect fixture".into()),
                    parent,
                    config: Some(AgentConfig {
                        effort: Some(aether_core::model::Effort::Low),
                        approval: ApprovalPolicy::none(),
                        ..Default::default()
                    }),
                    start: false,
                    ..Default::default()
                },
                parent.map(Actor::agent).unwrap_or_else(Actor::user),
            )
            .unwrap();
        (agent, task.unwrap())
    }

    #[tokio::test]
    async fn pages_include_terminal_ancestry_and_exclude_other_trees() {
        let (runtime, _config) = runtime().await;
        let (root, _) = create(&runtime, None);
        let (mut child, _) = create(&runtime, Some(root.id));
        let (grandchild, _) = create(&runtime, Some(child.id));
        let (foreign, foreign_task) = create(&runtime, None);
        child.config.effort = Some(aether_core::model::Effort::Low);
        child.status = AgentStatus::Completed {
            summary: Some("done".into()),
        };
        runtime
            .store
            .commit(Commit::new().mutate(Mutation::UpsertAgent(child.clone())))
            .unwrap();
        let first = agents(&runtime, root.id, None, 2)
            .unwrap()
            .structured
            .unwrap();
        assert_eq!(first["agents"].as_array().unwrap().len(), 2);
        let cursor = serde_json::from_value(first["next_after"].clone()).unwrap();
        let second = agents(&runtime, root.id, Some(cursor), 2)
            .unwrap()
            .structured
            .unwrap();
        assert!(second["next_after"].is_null());
        let found: BTreeSet<AgentId> = first["agents"]
            .as_array()
            .unwrap()
            .iter()
            .chain(second["agents"].as_array().unwrap())
            .map(|row| serde_json::from_value(row["id"].clone()).unwrap())
            .collect();
        assert_eq!(found, BTreeSet::from([root.id, child.id, grandchild.id]));
        assert!(!found.contains(&foreign.id));
        let child_row = first["agents"]
            .as_array()
            .unwrap()
            .iter()
            .chain(second["agents"].as_array().unwrap())
            .find(|row| row["id"] == json!(child.id))
            .unwrap();
        assert_eq!(child_row["model"], json!(child.model));
        assert_eq!(child_row["effort"], json!("low"));

        let task_page = tasks(&runtime, root.id, None, 25)
            .unwrap()
            .structured
            .unwrap();
        assert_eq!(task_page["tasks"].as_array().unwrap().len(), 3);
        assert!(!task_page.to_string().contains(&foreign_task.id.to_string()));
        assert!(
            task(&runtime, root.id, foreign_task.id, None, 0, 256)
                .unwrap()
                .is_error
        );
        assert!(agents(&runtime, root.id, None, 0).unwrap().is_error);
        assert!(
            tasks(&runtime, root.id, None, MAX_LIST_ITEMS + 1)
                .unwrap()
                .is_error
        );
        runtime.shutdown().await;
    }

    #[tokio::test]
    async fn task_pages_reconstruct_unicode_and_structured_results() {
        let (runtime, _config) = runtime().await;
        let (root, mut record) = create(&runtime, None);
        record.status = TaskStatus::Completed {
            result: TaskResult {
                summary: "🦀 result ".repeat(1_000),
                artifacts: vec![],
                structured: Some(json!({"answer":42})),
            },
        };
        runtime
            .store
            .commit(Commit::new().mutate(Mutation::UpsertTask(record.clone())))
            .unwrap();
        let mut offset = 0;
        let mut full = String::new();
        let mut revision: Option<String> = None;
        loop {
            let page = task(
                &runtime,
                root.id,
                record.id,
                revision.as_deref(),
                offset,
                256,
            )
            .unwrap()
            .structured
            .unwrap();
            revision = Some(page["revision"].as_str().unwrap().to_owned());
            let text = page["text"].as_str().unwrap();
            assert!(text.chars().count() <= 256);
            full.push_str(text);
            let Some(next) = page["next_offset"].as_u64() else {
                break;
            };
            assert!(next as usize > offset);
            offset = next as usize;
        }
        assert_eq!(serde_json::from_str::<Task>(&full).unwrap(), record);
        assert!(
            task(&runtime, root.id, record.id, None, 256, 256)
                .unwrap()
                .is_error
        );
        let beyond = task(
            &runtime,
            root.id,
            record.id,
            revision.as_deref(),
            full.chars().count() + 1,
            256,
        )
        .unwrap();
        assert!(beyond.is_error);
        assert!(beyond.content.contains("exceeds task record length"));
        record.objective.push_str(" changed");
        runtime
            .store
            .commit(Commit::new().mutate(Mutation::UpsertTask(record.clone())))
            .unwrap();
        let stale = task(&runtime, root.id, record.id, revision.as_deref(), 256, 256).unwrap();
        assert!(stale.is_error);
        assert!(stale.content.contains("changed"));
        assert!(
            !task(&runtime, root.id, record.id, None, 0, 256)
                .unwrap()
                .is_error
        );

        assert!(
            task(&runtime, root.id, record.id, None, 0, MAX_TASK_CHARS + 1)
                .unwrap()
                .is_error
        );
        runtime.shutdown().await;
    }
    #[tokio::test]
    async fn artifact_pages_are_scoped_lossless_and_report_missing_content() {
        use aether_core::artifact::ArtifactKind;
        use aether_tools::ArtifactSink;
        let (runtime, _config) = runtime().await;
        let (root, _) = create(&runtime, None);
        let (child, child_task) = create(&runtime, Some(root.id));
        let (foreign, _) = create(&runtime, None);
        let sink = crate::artifacts::StoreArtifactSink {
            runtime: runtime.clone(),
        };
        let text = "a🦀é\n".repeat(2000);
        let record = sink
            .store(
                ArtifactKind::CommandOutput,
                "output",
                "test",
                Some("text/plain"),
                text.as_bytes().to_vec(),
                child.id,
                Some(child_task.id),
            )
            .await
            .unwrap();
        let mut offset = 0;
        let mut reconstructed = String::new();
        loop {
            let page = artifact(&runtime, root.id, record.id, offset, 7).unwrap();
            assert!(!page.is_error, "{}", page.content);
            let page = page.structured.unwrap();
            let part = page["text"].as_str().unwrap();
            assert!(part.len() <= 7);
            reconstructed.push_str(part);
            let Some(next) = page["next_offset"].as_u64() else {
                break;
            };
            assert!(next > offset);
            offset = next;
        }
        assert_eq!(reconstructed, text);
        assert!(
            artifact(&runtime, foreign.id, record.id, 0, 8)
                .unwrap()
                .is_error
        );
        assert!(
            artifact(&runtime, root.id, record.id, 2, 8)
                .unwrap()
                .is_error
        );
        assert!(
            artifact(&runtime, root.id, record.id, u64::MAX, 8)
                .unwrap()
                .is_error
        );
        assert!(
            artifact(&runtime, root.id, record.id, 0, 0)
                .unwrap()
                .is_error
        );
        assert!(
            artifact(&runtime, root.id, ArtifactId::new(), 0, 8)
                .unwrap()
                .is_error
        );
        let eof = artifact(&runtime, root.id, record.id, text.len() as u64, 8).unwrap();
        assert!(!eof.is_error);
        assert!(eof.structured.unwrap()["next_offset"].is_null());
        runtime.store.evict_artifact_bytes(record.id).unwrap();
        assert!(
            artifact(&runtime, root.id, record.id, 0, 8)
                .unwrap()
                .is_error
        );
        let source = _config.path().join("shell.log");
        std::fs::write(&source, b"original output").unwrap();
        let copied = sink
            .store_file(
                ArtifactKind::CommandOutput,
                "shell log",
                "test",
                Some("text/plain"),
                &source,
                root.id,
                None,
            )
            .await
            .unwrap();
        std::fs::write(&source, b"changed").unwrap();
        let page = artifact(&runtime, root.id, copied.id, 0, 8192).unwrap();
        assert_eq!(page.structured.unwrap()["text"], "original output");
        let binary = sink
            .store(
                ArtifactKind::CommandOutput,
                "binary",
                "test",
                None,
                vec![255; 16],
                root.id,
                None,
            )
            .await
            .unwrap();
        assert!(
            artifact(&runtime, root.id, binary.id, 0, 8)
                .unwrap()
                .is_error
        );
        let names: Vec<_> = runtime
            .engine_tool_specs(&root)
            .into_iter()
            .map(|s| s.name)
            .collect();
        assert!(names.iter().any(|n| n == "read_artifact"));
        for native in ["read_file", "bash", "write_stdin", "shell_output"] {
            assert!(
                !names.iter().any(|n| n == native),
                "duplicate native tool: {native}"
            );
        }
        runtime.shutdown().await;
    }
}
