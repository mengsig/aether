use crate::{Result, Runtime, RuntimeError, turn_engine, withheld_by_position};
use aether_core::id::AgentId;
use aether_core::model::{ModelRef, PromptCacheTtl};
use aether_engine::{Engine, EngineKind};
use std::time::{Duration, Instant};
use std::{collections::HashMap, sync::Arc};

/// The shortest gap between two probes of one unusable engine, so a burst
/// of turns and creations shares one probe rather than each running the CLI.
const RECHECK_INTERVAL: Duration = Duration::from_secs(2);

/// What engine detection found: the usable engines, and why each other one
/// is not, when preflight ran and found a specific reason.
#[derive(Default, Clone)]
pub(crate) struct DetectedEngines {
    pub(crate) usable: HashMap<EngineKind, Arc<dyn Engine>>,
    pub(crate) unavailable: HashMap<EngineKind, String>,
}

impl DetectedEngines {
    pub(crate) fn get(&self, kind: &EngineKind) -> Option<Arc<dyn Engine>> {
        self.usable.get(kind).cloned()
    }

    /// Keep one engine's latest detection outcome, replacing the older one.
    fn record(&mut self, kind: EngineKind, found: std::result::Result<Arc<dyn Engine>, String>) {
        match found {
            Ok(engine) => {
                self.unavailable.remove(&kind);
                self.usable.insert(kind, engine);
            }
            Err(reason) => {
                self.usable.remove(&kind);
                self.unavailable.insert(kind, reason);
            }
        }
        debug_assert!(
            self.usable.contains_key(&kind) != self.unavailable.contains_key(&kind),
            "an engine is either usable or has a reason"
        );
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

/// Whether `detected` holds `kind` usable and its CLI is still installed.
fn still_usable(detected: &DetectedEngines, kind: EngineKind) -> bool {
    detected.usable.contains_key(&kind) && aether_engine::detect::missing_cli(kind).is_none()
}

/// The engines. Detection runs after the socket can answer, so a reader
/// that can wait awaits [`EngineCatalog::ready`]; one that cannot gets a
/// retryable [`RuntimeError::Starting`] from [`EngineCatalog::now`]. An
/// unusable engine is probed again by [`EngineCatalog::recheck`], so a CLI
/// fixed after startup needs no daemon restart.
#[derive(Default)]
pub(crate) struct EngineCatalog {
    finished: tokio::sync::SetOnce<()>,
    detected: parking_lot::RwLock<DetectedEngines>,
    /// `None` for engines a test supplied: there is nothing to probe.
    probing: Option<Probing>,
    /// When each engine was last probed again; held across that probe.
    rechecked: tokio::sync::Mutex<HashMap<EngineKind, Instant>>,
}

/// What probing an engine again needs.
#[derive(Clone, Copy)]
struct Probing {
    prompt_cache_ttl: Option<PromptCacheTtl>,
}

impl EngineCatalog {
    pub(crate) fn detected(engines: DetectedEngines) -> Self {
        Self {
            finished: tokio::sync::SetOnce::new_with(Some(())),
            detected: parking_lot::RwLock::new(engines),
            ..Self::default()
        }
    }

    /// A catalog that [`spawn_startup_probes`] fills by probing this machine.
    pub(crate) fn probing(prompt_cache_ttl: Option<PromptCacheTtl>) -> Self {
        Self {
            probing: Some(Probing { prompt_cache_ttl }),
            ..Self::default()
        }
    }

    /// Never waits longer than detection, whose probes are each bounded.
    pub(crate) async fn ready(&self) -> DetectedEngines {
        self.finished.wait().await;
        self.detected.read().clone()
    }

    pub(crate) fn now(&self) -> Result<DetectedEngines> {
        if self.finished.get().is_none() {
            return Err(RuntimeError::Starting(
                "engine and default model detection is still running; retry in a moment".into(),
            ));
        }
        Ok(self.detected.read().clone())
    }

    /// [`EngineCatalog::ready`], after probing `kind` again when it was
    /// unusable or its CLI has left `PATH` since. Probes of one engine are at
    /// least [`RECHECK_INTERVAL`] apart; a caller arriving meanwhile gets the
    /// latest outcome.
    pub(crate) async fn recheck(&self, kind: EngineKind) -> DetectedEngines {
        let detected = self.ready().await;
        let Some(probing) = self.probing else {
            return detected;
        };
        if still_usable(&detected, kind) {
            return detected;
        }
        let mut rechecked = self.rechecked.lock().await;
        let recent = rechecked
            .get(&kind)
            .is_some_and(|at| at.elapsed() < RECHECK_INTERVAL);
        // Another caller may have found it while this one waited for the lock.
        if still_usable(&self.detected.read(), kind) {
            return self.detected.read().clone();
        }
        if recent {
            return self.record_if_removed(kind);
        }
        let found = detect_engine(kind, probing.prompt_cache_ttl).await;
        rechecked.insert(kind, Instant::now());
        let mut detected = self.detected.write();
        detected.record(kind, found);
        detected.clone()
    }

    /// Between probes, a usable `kind` whose CLI has left `PATH` is recorded
    /// unusable at once, rather than served until the next probe may run.
    fn record_if_removed(&self, kind: EngineKind) -> DetectedEngines {
        let mut detected = self.detected.write();
        if detected.usable.contains_key(&kind)
            && let Some(missing) = aether_engine::detect::missing_cli(kind)
        {
            detected.record(kind, Err(missing.to_string()));
            debug_assert!(!detected.usable.contains_key(&kind), "recorded unusable");
        }
        detected.clone()
    }

    /// Record `kind` as unusable for `reason`, as a failed probe would.
    #[cfg(test)]
    pub(crate) fn mark_unavailable(&self, kind: EngineKind, reason: &str) {
        self.detected.write().record(kind, Err(reason.to_owned()));
    }

    fn finish(&self, engines: DetectedEngines) {
        *self.detected.write() = engines;
        let set = self.finished.set(());
        debug_assert!(set.is_ok(), "engine detection finishes once");
        if set.is_err() {
            tracing::error!("engine detection finished twice; kept the latest result");
        }
    }
}

/// Detect the engines, and the default model when none was configured,
/// without holding up startup: both run the providers' CLIs. The catalog is
/// set last and on every path, a panicking probe included, so no reader
/// waits forever or sees a default model still being resolved.
pub(super) fn spawn_startup_probes(
    runtime: &Arc<Runtime>,
    prompt_cache_ttl: Option<PromptCacheTtl>,
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
async fn detect_engines(prompt_cache_ttl: Option<PromptCacheTtl>) -> DetectedEngines {
    // Independent CLIs: probing them together bounds the wait by the slower one.
    let (claude, codex) = tokio::join!(
        detect_engine(EngineKind::CLAUDE_CODE, prompt_cache_ttl),
        detect_engine(EngineKind::CODEX, prompt_cache_ttl)
    );
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

/// Probe one engine's CLI: usable, or why not.
async fn detect_engine(
    kind: EngineKind,
    prompt_cache_ttl: Option<PromptCacheTtl>,
) -> std::result::Result<Arc<dyn Engine>, String> {
    if kind == EngineKind::CLAUDE_CODE {
        return detect_claude(prompt_cache_ttl).await;
    }
    if kind == EngineKind::CODEX {
        return detect_codex().await;
    }
    Err(format!(
        "{kind} has no installed-CLI detection in this daemon"
    ))
}

async fn detect_claude(
    prompt_cache_ttl: Option<PromptCacheTtl>,
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

/// Why `kind` cannot be used and what to do. Preflight's reason, when it
/// found one, tells too old or missing a capability from not installed.
pub(crate) fn unavailable_text(kind: EngineKind, provider: &str, reason: Option<&str>) -> String {
    assert!(!provider.is_empty(), "an engine runs for a named provider");
    match reason {
        Some(reason) => format!(
            "{reason}. Aether checks the {kind} CLI again on the next attempt, so retry once it is fixed; no daemon restart is needed"
        ),
        None => format!(
            "{kind} is not installed on this machine; install it, sign in to {provider} from your client, and retry"
        ),
    }
}

impl Runtime {
    /// Refuse to start an agent whose engine is unusable after a fresh probe,
    /// so none is created only to fail its first turn. The model is picked as
    /// creation picks it: `model`, else `parent`'s, else the default. Only a
    /// probe's reason refuses: not an engine a test supplied, nor detection
    /// still running, which the first turn waits for.
    pub async fn check_engine_for(
        &self,
        model: Option<&ModelRef>,
        parent: Option<AgentId>,
    ) -> Result<()> {
        if self.engines.now().is_err() {
            return Ok(());
        }
        // Creation reads the parent again and reports a missing one or a store error.
        let inherited = || {
            let parent = parent?;
            self.store
                .agent(parent)
                .ok()
                .flatten()
                .map(|agent| agent.model)
        };
        let Some(model) = model
            .cloned()
            .or_else(inherited)
            .or_else(|| self.default_model())
        else {
            return Ok(());
        };
        // An unknown model is refused by creation itself, with its own error.
        let Ok(resolved) = self.models.resolve(&model) else {
            return Ok(());
        };
        let Some(kind) = turn_engine::engine_kind(resolved.provider.protocol) else {
            return Ok(());
        };
        let detected = self.engines.recheck(kind).await;
        let Some(reason) = detected.unavailable.get(&kind) else {
            return Ok(());
        };
        debug_assert!(
            detected.get(&kind).is_none(),
            "an unusable engine has no adapter"
        );
        Err(RuntimeError::Invalid(format!(
            "model {model} runs on the {kind} engine, which this daemon cannot use: {}",
            unavailable_text(kind, &resolved.provider.id, Some(reason))
        )))
    }

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
