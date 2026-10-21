//! Canonical model types shared by every provider adapter.
//!
//! Adapters translate between these types and a wire protocol. Nothing here
//! is specific to one provider; provider-only features travel in the
//! namespaced `extensions` map so orchestration code stays portable without
//! flattening every model to the lowest common denominator.

use std::fmt;

use serde::{Deserialize, Serialize};

mod capabilities;
mod error;
mod messages;
mod request;
mod stream;
mod usage;

pub use capabilities::{CapabilityProfile, SERVER_COMPACTION_KEY, ThinkingSupport};
pub use error::{ModelError, ModelErrorClass, provider_exhaustion};
pub use messages::{ContentBlock, Message, MessageRole, ToolResultBlock, text_of};
pub use request::{
    Effort, ModelRequest, PromptCacheTtl, SystemBlock, ThinkingMode, ToolChoice, ToolSpec,
};
pub use stream::{
    AccumulateError, BlockStart, ModelResponse, StopReason, StreamAccumulator, StreamEvent,
};
pub use usage::{CostBasis, CostGap, ModelCost, ModelCostTier, Usage};

/// A provider-qualified model name, rendered as `provider/model`.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
pub struct ModelRef {
    pub provider: String,
    pub model: String,
}

impl ModelRef {
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
        }
    }

    /// Parse `provider/model`. The model part may itself contain slashes.
    pub fn parse(text: &str) -> Option<Self> {
        let (provider, model) = text.split_once('/')?;
        if provider.is_empty() || model.is_empty() {
            return None;
        }
        Some(Self::new(provider, model))
    }
}

impl fmt::Display for ModelRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.provider, self.model)
    }
}

/// An ordered list of models to try for one turn: the primary, then each
/// fallback. Rendered as `provider/model,provider/model`.
///
/// Non-empty by construction, so "which model does this agent use" always has
/// an answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelChain {
    pub primary: ModelRef,
    #[serde(default)]
    pub fallbacks: Vec<ModelRef>,
}

impl ModelChain {
    pub fn new(primary: ModelRef, fallbacks: Vec<ModelRef>) -> Self {
        Self { primary, fallbacks }
    }

    pub fn single(primary: ModelRef) -> Self {
        Self {
            primary,
            fallbacks: Vec::new(),
        }
    }

    /// Parse `provider/model[,provider/model]...`. Every link must parse, and
    /// blank links are rejected rather than dropped: a typo in a chain is a
    /// mistake, not a shorter chain.
    pub fn parse(text: &str) -> Option<Self> {
        let mut links = text.split(',').map(str::trim).map(ModelRef::parse);
        let primary = links.next()??;
        let fallbacks = links.collect::<Option<Vec<_>>>()?;
        Some(Self { primary, fallbacks })
    }

    /// The primary first, then each fallback in order.
    pub fn links(&self) -> impl Iterator<Item = &ModelRef> {
        std::iter::once(&self.primary).chain(self.fallbacks.iter())
    }

    /// Not `len`/`is_empty`: a chain is non-empty by construction, so an
    /// `is_empty` that can only return `false` would just mislead a reader.
    pub fn link_count(&self) -> usize {
        1 + self.fallbacks.len()
    }

    pub fn has_fallbacks(&self) -> bool {
        !self.fallbacks.is_empty()
    }
}

impl fmt::Display for ModelChain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let rendered: Vec<String> = self.links().map(ToString::to_string).collect();
        f.write_str(&rendered.join(","))
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "model/tests_cache_ttl.rs"]
mod tests_cache_ttl;
