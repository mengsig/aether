//! Bounded reads of the caller's delegated work.
use crate::tool::{HarnessAction, Tool, ToolContext, ToolError, ToolExecution, parse_input};
use aether_core::id::{AgentId, ArtifactId, TaskId};
use aether_core::tool::{ResourceKey, SideEffectClass, ToolDescriptor};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub const MAX_LIST_ITEMS: usize = 25;
pub const MAX_ARTIFACT_BYTES: usize = 8_192;
pub const MAX_TASK_CHARS: usize = 8_192;
fn artifact_limit() -> usize {
    MAX_ARTIFACT_BYTES
}
fn list_limit() -> usize {
    10
}
fn task_limit() -> usize {
    2_000
}

#[derive(Debug)]
pub enum InspectionTool {
    Agents,
    Peers,
    Tasks,
    Task,
    Artifact,
}

pub fn all() -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(InspectionTool::Agents),
        Arc::new(InspectionTool::Peers),
        Arc::new(InspectionTool::Tasks),
        Arc::new(InspectionTool::Task),
        Arc::new(InspectionTool::Artifact),
    ]
}

#[async_trait]
impl Tool for InspectionTool {
    fn descriptor(&self) -> ToolDescriptor {
        let (name, description, schema) = match self {
            Self::Agents => (
                "list_agents",
                "List yourself and your delegated descendants, including completed agents. Returns bounded metadata including model and effort in agent-ID order. Pass next_after as after for another page; each call reads current state.",
                json!({"type":"object","properties":{"after":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":MAX_LIST_ITEMS,"default":10}},"additionalProperties":false}),
            ),
            Self::Peers => (
                "list_peers",
                "List the other agents your parent delegated. Pass next_after as after for more.",
                json!({"type":"object","properties":{"after":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":MAX_LIST_ITEMS}},"additionalProperties":false}),
            ),
            Self::Tasks => (
                "list_tasks",
                "List tasks created or owned by yourself or your delegated descendants, including settled tasks. Returns short objectives in task-ID order. Pass next_after as after for another page. Use read_task for full results or structured data.",
                json!({"type":"object","properties":{"after":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":MAX_LIST_ITEMS,"default":10}},"additionalProperties":false}),
            ),
            Self::Artifact => (
                "read_artifact",
                "Read a UTF-8 text artifact produced by yourself or your delegated descendants. offset is a zero-based byte offset; limit bounds bytes returned. Pass next_offset for more. Pages end on character boundaries. Missing or evicted content is an error.",
                json!({"type":"object","properties":{"id":{"type":"string"},"offset":{"type":"integer","minimum":0,"default":0},"limit":{"type":"integer","minimum":4,"maximum":MAX_ARTIFACT_BYTES,"default":8192}},"required":["id"],"additionalProperties":false}),
            ),
            Self::Task => (
                "read_task",
                "Read a task record in your delegated scope, including its objective, status, result, structured data and artifact IDs. Returns a character page of serialized JSON, not necessarily a complete JSON document. Pass next_offset and revision for more; concatenate text pages if needed. If the record changes, restart at offset zero.",
                json!({"type":"object","properties":{"task":{"type":"string"},"revision":{"type":"string"},"offset":{"type":"integer","minimum":0,"default":0},"max_chars":{"type":"integer","minimum":256,"maximum":MAX_TASK_CHARS,"default":2000}},"required":["task"],"additionalProperties":false}),
            ),
        };
        super::descriptor(name, description, schema, SideEffectClass::ReadOnly, false)
    }
    fn resources(&self, _: &Value, _: &ToolContext) -> Vec<ResourceKey> {
        vec![]
    }
    async fn execute(
        &self,
        input: Value,
        _: &ToolContext,
        _: CancellationToken,
    ) -> Result<ToolExecution, ToolError> {
        let action = match self {
            Self::Agents | Self::Peers | Self::Tasks => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Input {
                    after: Option<String>,
                    #[serde(default = "list_limit")]
                    limit: usize,
                }
                let input: Input = parse_input(input)?;
                if !(1..=MAX_LIST_ITEMS).contains(&input.limit) {
                    return Err(ToolError::InvalidInput(format!(
                        "limit must be between 1 and {MAX_LIST_ITEMS}"
                    )));
                }
                let agent_cursor = || {
                    input
                        .after
                        .as_deref()
                        .map(str::parse::<AgentId>)
                        .transpose()
                        .map_err(|e| ToolError::InvalidInput(format!("invalid agent cursor: {e}")))
                };
                match self {
                    Self::Agents => HarnessAction::ListAgents {
                        after: agent_cursor()?,
                        limit: input.limit,
                    },
                    Self::Peers => HarnessAction::ListPeers {
                        after: agent_cursor()?,
                        limit: input.limit,
                    },
                    _ => HarnessAction::ListTasks {
                        after: input
                            .after
                            .map(|id| id.parse::<TaskId>())
                            .transpose()
                            .map_err(|e| {
                                ToolError::InvalidInput(format!("invalid task cursor: {e}"))
                            })?,
                        limit: input.limit,
                    },
                }
            }
            Self::Artifact => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Input {
                    // A raw `Value`, not `ArtifactId`: direct id
                    // deserialization loses the field name through
                    // `serde::de::Error::custom` (aether-core/src/id.rs), and
                    // a `String` field would drop it too for a wrong-typed
                    // value. Parsed below via `parse_field` so the rejection
                    // names `read_artifact.id` either way.
                    id: Value,
                    #[serde(default)]
                    offset: u64,
                    #[serde(default = "artifact_limit")]
                    limit: usize,
                }
                let input: Input = parse_input(input)?;
                if !(4..=MAX_ARTIFACT_BYTES).contains(&input.limit) {
                    return Err(ToolError::InvalidInput(format!(
                        "limit must be between 4 and {MAX_ARTIFACT_BYTES}"
                    )));
                }
                let id: ArtifactId = super::parse_field(&input.id, "read_artifact.id")?;
                HarnessAction::ReadArtifact {
                    id,
                    offset: input.offset,
                    limit: input.limit,
                }
            }
            Self::Task => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Input {
                    // See the comment on `Self::Artifact`'s `id` field.
                    task: Value,
                    revision: Option<String>,
                    #[serde(default)]
                    offset: usize,
                    #[serde(default = "task_limit")]
                    max_chars: usize,
                }
                let input: Input = parse_input(input)?;
                if !(256..=MAX_TASK_CHARS).contains(&input.max_chars) {
                    return Err(ToolError::InvalidInput(format!(
                        "max_chars must be between 256 and {MAX_TASK_CHARS}"
                    )));
                }
                let task: TaskId = super::parse_field(&input.task, "read_task.task")?;
                HarnessAction::ReadTask {
                    task,
                    revision: input.revision,
                    offset: input.offset,
                    max_chars: input.max_chars,
                }
            }
        };
        Ok(ToolExecution::Action(action))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::ctx_in;

    async fn act(
        tool: &dyn Tool,
        ctx: &ToolContext,
        input: Value,
    ) -> Result<HarnessAction, ToolError> {
        match tool.execute(input, ctx, CancellationToken::new()).await? {
            ToolExecution::Action(a) => Ok(a),
            ToolExecution::Outcome(_) => Err(ToolError::Internal("unexpected outcome".into())),
        }
    }

    fn invalid_input(err: &ToolError) -> &str {
        match err {
            ToolError::InvalidInput(msg) => msg,
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn read_artifact_id_names_field_value_and_form() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_in(dir.path());
        let bad = TaskId::new().to_string(); // wrong prefix for an artifact id
        let err = act(&InspectionTool::Artifact, &ctx, json!({"id": bad}))
            .await
            .unwrap_err();
        let msg = invalid_input(&err);
        assert!(msg.contains("read_artifact.id"), "{msg}");
        assert!(msg.contains(&bad), "{msg}");
        assert!(msg.contains("art_"), "{msg}");
    }

    #[tokio::test]
    async fn read_task_task_names_field_value_and_form() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_in(dir.path());
        let bad = ArtifactId::new().to_string(); // wrong prefix for a task id
        let err = act(&InspectionTool::Task, &ctx, json!({"task": bad}))
            .await
            .unwrap_err();
        let msg = invalid_input(&err);
        assert!(msg.contains("read_task.task"), "{msg}");
        assert!(msg.contains(&bad), "{msg}");
        assert!(msg.contains("tsk_"), "{msg}");
    }

    /// A wrong-*typed* value (not just a wrong-valued one) must also
    /// name the field - see the identical test in `harness.rs`.
    #[tokio::test]
    async fn wrong_json_type_still_names_the_field() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_in(dir.path());
        let err = act(&InspectionTool::Artifact, &ctx, json!({"id": 5}))
            .await
            .unwrap_err();
        assert!(invalid_input(&err).contains("read_artifact.id"), "{err:?}");

        let err = act(&InspectionTool::Task, &ctx, json!({"task": false}))
            .await
            .unwrap_err();
        assert!(invalid_input(&err).contains("read_task.task"), "{err:?}");
    }

    #[tokio::test]
    async fn valid_ids_still_work() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_in(dir.path());
        let id = ArtifactId::new();
        let action = act(
            &InspectionTool::Artifact,
            &ctx,
            json!({"id": id.to_string()}),
        )
        .await
        .unwrap();
        assert!(matches!(action, HarnessAction::ReadArtifact { id: got, .. } if got == id));

        let task = TaskId::new();
        let action = act(
            &InspectionTool::Task,
            &ctx,
            json!({"task": task.to_string()}),
        )
        .await
        .unwrap();
        assert!(matches!(action, HarnessAction::ReadTask { task: got, .. } if got == task));
    }

    /// Every advertised `additionalProperties: false` input here must
    /// reject an unknown field rather than silently drop it.
    #[tokio::test]
    async fn unknown_field_is_rejected_on_every_input() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_in(dir.path());
        let cases: Vec<(&dyn Tool, Value)> = vec![
            (&InspectionTool::Agents, json!({"bogus": 1})),
            (&InspectionTool::Peers, json!({"bogus": 1})),
            (&InspectionTool::Tasks, json!({"bogus": 1})),
            (
                &InspectionTool::Artifact,
                json!({"id": ArtifactId::new().to_string(), "bogus": 1}),
            ),
            (
                &InspectionTool::Task,
                json!({"task": TaskId::new().to_string(), "bogus": 1}),
            ),
        ];
        for (tool, input) in cases {
            let name = tool.descriptor().spec.name;
            let err = act(tool, &ctx, input).await.unwrap_err();
            let msg = invalid_input(&err);
            assert!(msg.contains("bogus"), "{name}: {msg}");
        }
    }
}
