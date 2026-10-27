use crate::{Result, Runtime, RuntimeError, turn_engine, withheld_by_position};
use aether_core::id::AgentId;
use aether_engine::{Engine, EngineKind};
use std::{collections::HashMap, sync::Arc};

/// What engine detection found: the usable engines, and why each other one
/// is not, when preflight ran and found a specific reason.
#[derive(Default)]
pub(crate) struct DetectedEngines {
    pub(crate) usable: HashMap<EngineKind, Arc<dyn Engine>>,
    pub(crate) unavailable: HashMap<EngineKind, String>,
}

impl DetectedEngines {
    pub(crate) fn get(&self, kind: &EngineKind) -> Option<&Arc<dyn Engine>> {
        self.usable.get(kind)
    }

    /// Every engine unusable for one reason: detection itself failed.
    fn failed(reason: &str) -> Self {
        let unavailable = [EngineKind::CLAUDE_CODE, EngineKind::CODEX]
            .into_iter()
            .map(|kind| (kind, reason.to_owned()))
            .collect();
        Self {
            usable: HashMap::new(),
            unavailable,
        }
    }
}

/// The engines, set once. Detection runs after the socket can answer, so a
/// reader that can wait awaits [`EngineCatalog::ready`]; one that cannot gets
/// a retryable [`RuntimeError::Starting`] from [`EngineCatalog::now`].
#[derive(Default)]
pub(crate) struct EngineCatalog(tokio::sync::SetOnce<DetectedEngines>);

impl EngineCatalog {
    pub(crate) fn detected(engines: DetectedEngines) -> Self {
        Self(tokio::sync::SetOnce::new_with(Some(engines)))
    }

    /// Never waits longer than detection, whose probes are each bounded.
    pub(crate) async fn ready(&self) -> &DetectedEngines {
        self.0.wait().await
    }

    pub(crate) fn now(&self) -> Result<&DetectedEngines> {
        self.0.get().ok_or_else(|| {
            RuntimeError::Starting(
                "engine and default model detection is still running; retry in a moment".into(),
            )
        })
    }

    fn finish(&self, engines: DetectedEngines) {
        let set = self.0.set(engines);
        debug_assert!(set.is_ok(), "engine detection finishes once");
        if set.is_err() {
            tracing::error!("engine detection finished twice; kept the first result");
        }
    }
}

/// Detect the engines, and the default model when none was configured,
/// without holding up startup: both run the providers' CLIs. The catalog is
/// set last and on every path, a panicking probe included, so no reader
/// waits forever or sees a default model still being resolved.
pub(super) fn spawn_startup_probes(
    runtime: &Arc<Runtime>,
    prompt_cache_ttl: Option<aether_core::model::PromptCacheTtl>,
) {
    debug_assert!(
        runtime.engines.now().is_err(),
        "detection runs only for a runtime started without engines"
    );
    let detecting = runtime.clone();
    let resolve_default = runtime.default_model().is_none();
    tokio::spawn(async move {
        let started = std::time::Instant::now();
        let models = detecting.models.clone();
        let (found, default_model) =
            tokio::join!(tokio::spawn(detect_engines(prompt_cache_ttl)), async move {
                if resolve_default {
                    Some(models.default_model().await)
                } else {
                    None
                }
            });
        match default_model {
            Some(None) => tracing::warn!(
                "no model is configured; sign in from your client before running agents"
            ),
            Some(Some(model)) => detecting.set_default_model(Some(model)),
            None => {}
        }
        let found = match found {
            Ok(found) => found,
            Err(error) => {
                let reason = format!("engine detection failed: {error}; restart the daemon");
                detecting.report_problem("engine detection".into(), reason.clone());
                DetectedEngines::failed(&reason)
            }
        };
        let mut usable: Vec<&str> = found.usable.keys().map(|kind| kind.as_str()).collect();
        usable.sort_unstable();
        tracing::info!(
            elapsed_ms = started.elapsed().as_millis() as u64,
            engines = ?usable,
            "engine detection finished"
        );
        detecting.engines.finish(found);
    });
}

/// The official CLIs installed on this machine, keyed by engine, plus why
/// each omitted engine is unusable, keyed the same way. An engine this
/// machine cannot use is omitted, same as before this preflight; the reason
/// is logged at `warn` and kept on [`Runtime`] so a turn that reaches it
/// reports why rather than a generic "not installed".
async fn detect_engines(
    prompt_cache_ttl: Option<aether_core::model::PromptCacheTtl>,
) -> DetectedEngines {
    // Independent CLIs: probing them together bounds the wait by the slower one.
    let (claude, codex) = tokio::join!(detect_claude(prompt_cache_ttl), detect_codex());
    let mut detected = DetectedEngines::default();
    for (kind, found) in [
        (EngineKind::CLAUDE_CODE, claude),
        (EngineKind::CODEX, codex),
    ] {
        match found {
            Ok(engine) => {
                detected.usable.insert(kind, engine);
            }
            Err(reason) => {
                detected.unavailable.insert(kind, reason);
            }
        }
    }
    debug_assert_eq!(
        detected.usable.len() + detected.unavailable.len(),
        2,
        "each engine is either usable or has a reason"
    );
    detected
}

async fn detect_claude(
    prompt_cache_ttl: Option<aether_core::model::PromptCacheTtl>,
) -> std::result::Result<Arc<dyn Engine>, String> {
    match aether_engine::ClaudeCodeEngine::detect_verbose().await {
        Ok(e) => {
            // Usable-with-a-limitation is not unavailable: it
            // still registers, distinct in the log from a refusal below.
            if let Some(limitation) = e.limitation() {
                tracing::warn!(engine = "claude-code", %limitation, "engine usable, with limits");
            }
            let e = match prompt_cache_ttl {
                Some(ttl) => e.with_prompt_cache_ttl(ttl),
                None => e,
            };
            Ok(Arc::new(e))
        }
        Err(reason) => {
            tracing::warn!(engine = "claude-code", %reason, "engine CLI unusable");
            Err(reason.to_string())
        }
    }
}

/// The app-server protocol gates, steers and interrupts; exec mode is the
/// fallback for older Codex builds.
async fn detect_codex() -> std::result::Result<Arc<dyn Engine>, String> {
    let app_reason = match aether_engine::CodexAppEngine::detect_verbose().await {
        Ok(e) => return Ok(Arc::new(e)),
        Err(reason) => reason,
    };
    match aether_engine::CodexEngine::detect_verbose().await {
        Ok(e) => Ok(Arc::new(e)),
        Err(exec_reason) => {
            tracing::warn!(
                engine = "codex",
                app_server_reason = %app_reason,
                exec_reason = %exec_reason,
                "engine CLI unusable"
            );
            // The common case - codex simply absent - fails both probes
            // with the identical reason; do not repeat it.
            if app_reason == exec_reason {
                Err(app_reason.to_string())
            } else {
                Err(format!(
                    "{app_reason} (legacy exec fallback: {exec_reason})"
                ))
            }
        }
    }
}

impl Runtime {
    /// Whether the startup probes, engine detection and the default model,
    /// have finished.
    pub fn startup_probes_finished(&self) -> bool {
        self.engines.now().is_ok()
    }

    /// Wait for the startup probes to finish.
    pub async fn await_startup_probes(&self) {
        self.engines.ready().await;
    }

    /// Engines detected on this machine, by provider protocol name, once
    /// detection has finished.
    pub async fn engines(&self) -> Vec<String> {
        let detected = self.engines.ready().await;
        detected
            .usable
            .keys()
            .map(|k| k.as_str().to_owned())
            .collect()
    }

    /// The channel of the engine turn that issued `token`, or a refusal. The
    /// token is per-turn, so a stale bridge cannot reach a later turn.
    fn engine_channel(
        &self,
        agent: AgentId,
        token: &str,
    ) -> Result<tokio::sync::mpsc::Sender<turn_engine::EngineRequest>> {
        let turns = self.engine_turns.lock();
        match turns.get(&agent) {
            Some(h) if aether_protocol::auth::constant_time_eq(&h.token, token) => Ok(h.tx.clone()),
            Some(_) => Err(RuntimeError::Invalid("engine token mismatch".into())),
            None => Err(RuntimeError::Invalid(format!(
                "no engine turn is running for {agent}"
            ))),
        }
    }

    /// Tools this agent may receive through Aether's API or native model path.
    pub(crate) fn agent_tool_specs(
        &self,
        agent: &aether_core::agent::Agent,
    ) -> Vec<aether_core::model::ToolSpec> {
        self.tools
            .specs_for(&agent.config.tools, &agent.scope)
            .into_iter()
            .filter(|spec| withheld_by_position(agent, &spec.name).is_none())
            .collect()
    }

    /// What an engine reaches through Aether: the harness tools, plus the MCP
    /// servers' tools this agent may use. The engine's own shell and file
    /// tools are its business, not Aether's.
    pub fn engine_tool_specs(
        &self,
        agent: &aether_core::agent::Agent,
    ) -> Vec<aether_core::model::ToolSpec> {
        let mut offered: std::collections::BTreeSet<String> =
            aether_tools::builtin::harness_tools()
                .iter()
                .map(|t| t.descriptor().spec.name)
                .collect();
        if let Some(mcp) = self.tools.mcp() {
            offered.extend(mcp.tools().iter().map(|t| t.descriptor().spec.name));
        }
        self.agent_tool_specs(agent)
            .into_iter()
            .filter(|s| offered.contains(&s.name))
            .collect()
    }

    /// The same list, for the bridge that presents it over MCP. Authenticated
    /// by the per-turn token so only the running turn's bridge can ask.
    pub fn engine_tools(
        &self,
        agent: AgentId,
        token: &str,
    ) -> Result<Vec<aether_core::model::ToolSpec>> {
        self.engine_channel(agent, token)?;
        let record = self
            .store
            .agent(agent)?
            .ok_or_else(|| RuntimeError::NotFound(agent.to_string()))?;
        Ok(self.engine_tool_specs(&record))
    }

    /// Route a bridge tool call or permission prompt to the engine turn that
    /// issued `token`. Returns the model-facing text and an error flag.
    pub async fn engine_tool(
        &self,
        agent: AgentId,
        token: &str,
        name: String,
        input: serde_json::Value,
    ) -> Result<(String, bool)> {
        let tx = self.engine_channel(agent, token)?;
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        tx.send(turn_engine::EngineRequest {
            name,
            input,
            reply: reply_tx,
        })
        .await
        .map_err(|_| RuntimeError::Internal("engine turn ended before answering".into()))?;
        reply_rx
            .await
            .map_err(|_| RuntimeError::Internal("engine turn dropped the request".into()))
    }
}

/// A fresh 256-bit capability for one engine turn.
pub(crate) fn mint_engine_token() -> String {
    hex::encode(rand::random::<[u8; 32]>())
}

#[cfg(test)]
mod tests {
    #[test]
    fn engine_tokens_are_full_length_hex_and_unique() {
        let tokens: std::collections::HashSet<_> =
            (0..128).map(|_| super::mint_engine_token()).collect();
        assert_eq!(tokens.len(), 128);
        assert!(
            tokens.iter().all(
                |token| token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit())
            )
        );
    }
}
