use super::*;
use crate::testing::ctx_in;

async fn act(tool: &dyn Tool, ctx: &ToolContext, input: Value) -> Result<HarnessAction, ToolError> {
    match tool.execute(input, ctx, CancellationToken::new()).await? {
        ToolExecution::Action(a) => Ok(a),
        ToolExecution::Outcome(_) => Err(ToolError::Internal("unexpected outcome".into())),
    }
}

use aether_core::{
    agent::WaitCondition,
    id::{AgentId, ArtifactId, TaskId},
    memory::{MemoryKind, MemoryScope},
    model::{Effort, ModelRef},
};
use std::collections::BTreeSet;

mod delegation;

mod waiting;

mod inspection;
mod providers;

mod validation;
