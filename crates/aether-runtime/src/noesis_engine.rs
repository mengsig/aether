//! What the daemon's Noesis service needs to run a teaching step on an
//! engine CLI: whether the engine can launch now, and a one-shot client
//! that sends a prompt to a fresh, toolless engine session.

use std::sync::Arc;
use std::time::Duration;

use aether_core::agent::{Agent, ExecutionBackend};
use aether_core::id::{AgentId, InvocationId};
use aether_core::model::ModelRef;
use aether_model::ModelClient;

use crate::bus::{DeltaKind, DeltaStream};
use crate::noesis_seal::StreamSeal;
use crate::{Result, Runtime, RuntimeError, turn_engine};

/// How long one Noesis step may run on an engine: a lesson is longer than
/// a checkpoint, and the CLI starts cold each time.
const NOESIS_STEP_TIMEOUT: Duration = Duration::from_secs(300);

impl Runtime {
    /// `None` for a model no engine runs; else whether a turn could launch
    /// its engine now, as detection last found it. While the startup probes
    /// run, an engine is not ready yet.
    pub fn engine_launchable(&self, model: &ModelRef) -> Option<bool> {
        let resolved = self.models.resolve(model).ok()?;
        let kind = turn_engine::engine_kind(resolved.provider.protocol)?;
        let launchable = self
            .engines
            .now()
            .is_ok_and(|detected| detected.launchable(&kind).is_some());
        Some(launchable)
    }

    /// A client that answers each request with one fresh engine session of
    /// `agent`'s model: no Aether bridge, no native tools, read-only. The
    /// request's system blocks replace the engine's instructions and its
    /// messages' text is the one prompt. Usage is priced as a turn's is.
    pub async fn noesis_engine_client(
        self: &Arc<Self>,
        agent: &Agent,
    ) -> Result<Arc<dyn ModelClient>> {
        let resolved = self.models.resolve(&agent.model)?;
        let Some(kind) = turn_engine::engine_kind(resolved.provider.protocol) else {
            return Err(RuntimeError::Invalid(format!(
                "model {} does not run on an engine",
                agent.model
            )));
        };
        if agent.scope.backend != ExecutionBackend::Host {
            return Err(RuntimeError::Invalid(
                "a Noesis step on an engine runs on the host only".into(),
            ));
        }
        let detected = self.engines.recheck(kind).await;
        let engine = detected.launchable(&kind).ok_or_else(|| {
            RuntimeError::Invalid(crate::engine_access::unavailable_text(
                kind,
                &resolved.provider.id,
                detected.reason(&kind),
            ))
        })?;
        let temp_dir = crate::turn::agent_temp_dir(self, agent.id)?;
        let cwd = temp_dir
            .clone()
            .or_else(|| self.config.workspace_root.clone())
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        debug_assert!(!resolved.model.id.is_empty(), "a resolved model has an id");
        Ok(Arc::new(crate::compaction_client::CompactionClient {
            engine,
            context: aether_engine::EngineLaunchContext {
                run_id: agent.run,
                agent_id: agent.id,
                task_id: None,
                spawns: self.engine_spawns.clone(),
            },
            pricing: turn_engine::TurnPricing::of(self, &resolved),
            purpose: "Noesis step",
            timeout: NOESIS_STEP_TIMEOUT,
            turn: aether_engine::EngineTurn {
                redactor: self.tool_runtime.redactor().clone(),
                prompt: String::new(),
                images: Vec::new(),
                system_prompt: String::new(),
                session: None,
                session_usage: aether_core::model::Usage::default(),
                session_model_usage: None,
                new_session_id: aether_engine::new_session_id(),
                session_recorder: Some(turn_engine::session_recorder(self, agent.id)),
                model: Some(resolved.model.id),
                effort: None,
                cwd,
                extra_dirs: Vec::new(),
                summary_only: true,
                compact_session: false,
                auto_compact_tokens: None,
                lean_host_context: true,
                network_access: false,
                bypass_permissions: false,
                hidden_env: self.tools.engine_hidden_env().to_vec(),
                tool_hidden_env: self.tools.hidden_env().to_vec(),
                temp_dir,
                build_env: Vec::new(),
                mcp: None,
            },
        }))
    }
}

/// The live deltas of one Noesis step, so a client streams the lesson as it
/// does an ordinary turn. Only the turn's bounds and its text go out: a
/// tool's input may hold a sealed value. Dropping it finishes the turn.
pub struct NoesisStepStream {
    runtime: Arc<Runtime>,
    agent: AgentId,
    deltas: DeltaStream,
}

impl std::fmt::Debug for NoesisStepStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NoesisStepStream")
            .field("agent", &self.agent)
            .field("deltas", &self.deltas)
            .finish_non_exhaustive()
    }
}

impl NoesisStepStream {
    /// Opens turn `turn` (1 for the agent's first step) of `agent`.
    pub fn start(runtime: Arc<Runtime>, agent: AgentId, turn: u32) -> Self {
        assert!(turn >= 1, "turns count from one");
        let mut deltas = DeltaStream::new(InvocationId::new(), StreamSeal::Noesis);
        runtime
            .bus
            .publish_delta(agent, &mut deltas, DeltaKind::TurnStarted { turn });
        Self {
            runtime,
            agent,
            deltas,
        }
    }

    pub fn text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let delta = DeltaKind::Text { text: text.into() };
        self.runtime
            .bus
            .publish_delta(self.agent, &mut self.deltas, delta);
    }
}

impl Drop for NoesisStepStream {
    fn drop(&mut self) {
        self.runtime
            .bus
            .publish_delta(self.agent, &mut self.deltas, DeltaKind::TurnFinished);
    }
}
