use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::ModelCost;

/// Which thinking controls a model accepts.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingSupport {
    #[default]
    None,
    /// Fixed token budget (`budget_tokens`, `reasoning_effort` mapped to budgets).
    Budget,
    /// Model decides depth; effort steers it.
    Adaptive,
}

/// What a model can do. Declared per model in the registry so callers can ask
/// before they depend on a feature.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CapabilityProfile {
    pub context_window: u32,
    pub max_output_tokens: u32,
    /// Context size at which Aether compacts automatically; `None` compacts at
    /// three quarters of the window. Resolved per model, never above the window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction_tokens: Option<u32>,
    #[serde(default = "default_true")]
    pub tools: bool,
    #[serde(default = "default_true")]
    pub parallel_tools: bool,
    #[serde(default)]
    pub vision: bool,
    #[serde(default)]
    pub thinking: ThinkingSupport,
    #[serde(default)]
    pub effort: bool,
    #[serde(default)]
    pub prompt_caching: bool,
    #[serde(default)]
    pub structured_output: bool,
    #[serde(default)]
    pub strict_tools: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<ModelCost>,
    /// Provider-declared extras, keyed like request extensions.
    #[serde(default)]
    pub extensions: BTreeMap<String, Value>,
}

fn default_true() -> bool {
    true
}

/// Extension key marking a provider that compacts the context on its server.
pub const SERVER_COMPACTION_KEY: &str = "aether.server_compaction";

impl CapabilityProfile {
    /// Whether the provider folds old context itself, so Aether need not.
    pub fn compacts_on_server(&self) -> bool {
        self.extensions
            .get(SERVER_COMPACTION_KEY)
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }
}

impl Default for CapabilityProfile {
    fn default() -> Self {
        Self {
            context_window: 128_000,
            max_output_tokens: 8_192,
            compaction_tokens: None,
            tools: true,
            parallel_tools: true,
            vision: false,
            thinking: ThinkingSupport::None,
            effort: false,
            prompt_caching: false,
            structured_output: false,
            strict_tools: false,
            cost: None,
            extensions: BTreeMap::new(),
        }
    }
}
