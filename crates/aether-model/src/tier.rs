//! Capability bands that let callers pick a model without naming one.

use serde::{Deserialize, Serialize};

/// A capability band within one provider. The vocabulary is shared with
/// Archon's catalog tiers: keep the three names stable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelTier {
    Fast,
    Balanced,
    Strongest,
}

impl ModelTier {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Balanced => "balanced",
            Self::Strongest => "strongest",
        }
    }
}

/// Output price at or above this share of the provider's dearest tiered model
/// ranks `strongest`; below it but at or above [`BALANCED_SHARE`], `balanced`.
/// Chosen so the built-in families land in their declared bands (Opus 0.4,
/// Sonnet 0.2, Haiku 0.01 of Fable's $50): price gaps alone cannot separate them.
const STRONGEST_SHARE: f64 = 0.35;
const BALANCED_SHARE: f64 = 0.1;

/// Rank a model by its output price relative to `reference`, the highest
/// output price among tiered models of the same provider. `None` when either
/// price is not a positive finite number: an unpriced model stays unranked.
pub fn tier_by_price(output_per_mtok: f64, reference: f64) -> Option<ModelTier> {
    let usable = |price: f64| price.is_finite() && price > 0.0;
    if !usable(output_per_mtok) || !usable(reference) {
        return None;
    }
    let share = output_per_mtok / reference;
    Some(if share >= STRONGEST_SHARE {
        ModelTier::Strongest
    } else if share >= BALANCED_SHARE {
        ModelTier::Balanced
    } else {
        ModelTier::Fast
    })
}

#[cfg(test)]
mod tests;
