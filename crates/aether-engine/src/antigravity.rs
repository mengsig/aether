//! Antigravity as an engine: `agy acp`, the CLI's Agent Client Protocol server.
//!
//! Print mode (`agy -p --output-format stream-json`) streams the same turn,
//! but it cannot attach a per-session MCP server (`--settings` refuses
//! `mcpServers`) and auto-denies every permission request, so Aether's
//! harness tools and approvals could not reach the agent. ACP takes the
//! bridge in `session/new` and asks through `session/request_permission`.
//! See `docs/engines.md`.

use aether_core::model::Effort;
use tokio_util::sync::CancellationToken;

use crate::detect::{self, DetectError, EngineVersion};
use crate::{Engine, EngineError, EngineKind, EngineLaunchContext, EngineRun, EngineTurn};

mod acp;
mod launch;

/// The models Aether offers when `agy models` cannot be read: the Gemini
/// families, without their effort suffix.
pub const FALLBACK_MODELS: &[&str] = &[
    "gemini-4-hydrogen",
    "gemini-4-neon",
    "gemini-4-carbon",
    "gemini-4-argon",
    "gemini-3.8-flash",
];

/// The effort suffixes `agy models` appends to a family id, lowest first.
const EFFORT_SUFFIXES: &[(&str, Effort)] = &[
    ("low", Effort::Low),
    ("medium", Effort::Medium),
    ("high", Effort::High),
    ("xhigh", Effort::XHigh),
];

#[derive(Debug, Clone)]
pub struct AntigravityEngine {
    program: String,
    version: String,
    /// Every model id `agy models` listed, effort variants included.
    variants: Vec<String>,
}

impl AntigravityEngine {
    pub fn new(program: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            version: version.into(),
            variants: Vec::new(),
        }
    }

    /// Use `variants` (ids as `agy models` prints them) to pick effort levels.
    pub fn with_variants(mut self, variants: Vec<String>) -> Self {
        self.variants = variants;
        self
    }

    /// Find `agy`, read its version, and confirm it is signed in by listing
    /// its models. Credentials are never read: a listing that fails is the
    /// signed-out state.
    pub async fn detect_verbose() -> Result<Self, DetectError> {
        Self::detect_verbose_on(None).await
    }

    async fn detect_verbose_on(path: Option<&std::path::Path>) -> Result<Self, DetectError> {
        let version = detect::probe_version("agy", path, None).await?;
        if version < MIN_VERSION {
            return Err(DetectError::TooOld {
                program: "agy".into(),
                detected: version,
                minimum: MIN_VERSION,
            });
        }
        let listing =
            detect::probe_output("agy", &["models"], "a signed-in Google account", path).await?;
        let variants = parse_model_list(&listing);
        if variants.is_empty() {
            return Err(DetectError::MissingCapability {
                program: "agy".into(),
                capability: "a signed-in Google account".into(),
                detail: "agy models listed no models".into(),
            });
        }
        Ok(Self::new("agy", version.to_string()).with_variants(variants))
    }

    /// The model families this CLI offers, without effort suffixes and
    /// without its orchestrator entries; the fallback list when unread.
    pub fn models(&self) -> Vec<String> {
        let mut families: Vec<String> = Vec::new();
        for id in &self.variants {
            let family = family_of(id).to_owned();
            if !families.contains(&family) {
                families.push(family);
            }
        }
        if families.is_empty() {
            return FALLBACK_MODELS.iter().map(|m| (*m).to_owned()).collect();
        }
        families
    }

    /// The `agy` model id for `model` at `effort`: the highest listed variant
    /// at or below it, else the lowest listed. Unlisted families keep the
    /// requested suffix, so `agy` itself names what it lacks.
    pub fn variant_for(&self, model: &str, effort: Option<Effort>) -> String {
        assert!(!model.trim().is_empty(), "a model id names a family");
        let family = family_of(model);
        let listed: Vec<(&str, Effort)> = EFFORT_SUFFIXES
            .iter()
            .copied()
            .filter(|(suffix, _)| {
                self.variants
                    .iter()
                    .any(|v| *v == format!("{family}-{suffix}"))
            })
            .collect();
        let wanted = effort.unwrap_or(Effort::Medium);
        if listed.is_empty() {
            let suffix = EFFORT_SUFFIXES
                .iter()
                .rev()
                .find(|(_, level)| *level <= wanted)
                .map_or("low", |(suffix, _)| *suffix);
            return format!("{family}-{suffix}");
        }
        let chosen = listed
            .iter()
            .rev()
            .find(|(_, level)| *level <= wanted)
            .unwrap_or(&listed[0]);
        format!("{family}-{}", chosen.0)
    }
}

/// The model ids in `agy models` output: one `id<TAB>name` per line, after
/// a progress line. Orchestrator entries route to other models and are left out.
pub fn parse_model_list(listing: &str) -> Vec<String> {
    listing
        .lines()
        .filter_map(|line| line.split_once('\t').map(|(id, _)| id.trim()))
        .filter(|id| !id.is_empty() && !id.contains(char::is_whitespace))
        .filter(|id| !id.contains("orchestrator"))
        .map(str::to_owned)
        .collect()
}

/// `gemini-4-neon-high` -> `gemini-4-neon`; an id with no effort suffix is its own family.
fn family_of(id: &str) -> &str {
    EFFORT_SUFFIXES
        .iter()
        .find_map(|(suffix, _)| id.strip_suffix(suffix)?.strip_suffix('-'))
        .unwrap_or(id)
}

#[async_trait::async_trait]
impl Engine for AntigravityEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::ANTIGRAVITY
    }

    fn adapter(&self) -> String {
        format!("antigravity/{}", self.version)
    }

    fn validate_network_policy(&self, network_access: bool) -> Result<(), EngineError> {
        if network_access {
            return Ok(());
        }
        Err(EngineError::Unsupported(
            "Antigravity turns cannot deny network access; agy has no network sandbox".into(),
        ))
    }

    async fn run(
        &self,
        turn: EngineTurn,
        cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        launch::launch(self, turn, None, cancel).await
    }

    async fn run_with_context(
        &self,
        turn: EngineTurn,
        context: EngineLaunchContext,
        cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        launch::launch(self, turn, Some(context), cancel).await
    }
}

/// The oldest `agy` whose ACP server this adapter was verified against.
pub const MIN_VERSION: EngineVersion = EngineVersion::new(1, 5, 18);

#[cfg(test)]
#[path = "antigravity/tests.rs"]
mod tests;
