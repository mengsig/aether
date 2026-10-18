use crate::{Result, Runtime, RuntimeError, turn_engine, withheld_by_position};
use aether_core::id::AgentId;
use std::{collections::HashMap, sync::Arc};

/// The official CLIs installed on this machine, keyed by engine, plus why
/// each omitted engine is unusable, keyed the same way. An engine this
/// machine cannot use is omitted, same as before this preflight; the reason
/// is logged at `warn` and kept on [`Runtime`] so a turn that reaches it
/// reports why rather than a generic "not installed".
pub(super) async fn detect_engines(
    prompt_cache_ttl: Option<aether_core::model::PromptCacheTtl>,
) -> (
    HashMap<aether_engine::EngineKind, Arc<dyn aether_engine::Engine>>,
    HashMap<aether_engine::EngineKind, String>,
) {
    let mut engines: HashMap<aether_engine::EngineKind, Arc<dyn aether_engine::Engine>> =
        HashMap::new();
    let mut unavailable: HashMap<aether_engine::EngineKind, String> = HashMap::new();
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
            engines.insert(aether_engine::EngineKind::CLAUDE_CODE, Arc::new(e));
        }
        Err(reason) => {
            tracing::warn!(engine = "claude-code", %reason, "engine CLI unusable");
            unavailable.insert(aether_engine::EngineKind::CLAUDE_CODE, reason.to_string());
        }
    }
    // The app-server protocol gates, steers and interrupts; exec mode is
    // the fallback for older Codex builds.
    match aether_engine::CodexAppEngine::detect_verbose().await {
        Ok(e) => {
            engines.insert(aether_engine::EngineKind::CODEX, Arc::new(e));
        }
        Err(app_reason) => match aether_engine::CodexEngine::detect_verbose().await {
            Ok(e) => {
                engines.insert(aether_engine::EngineKind::CODEX, Arc::new(e));
            }
            Err(exec_reason) => {
                tracing::warn!(
                    engine = "codex",
                    app_server_reason = %app_reason,
                    exec_reason = %exec_reason,
                    "engine CLI unusable"
                );
                // The common case - codex simply absent - fails both probes
                // with the identical reason; do not repeat it.
                let reason = if app_reason == exec_reason {
                    app_reason.to_string()
                } else {
                    format!("{app_reason} (legacy exec fallback: {exec_reason})")
                };
                unavailable.insert(aether_engine::EngineKind::CODEX, reason);
            }
        },
    }
    (engines, unavailable)
}

impl Runtime {
    /// Engines detected on this machine, by provider protocol name.
    pub fn engines(&self) -> Vec<String> {
        self.engines.keys().map(|k| k.as_str().to_owned()).collect()
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
